// SPDX-License-Identifier: AGPL-3.0-or-later
//! From a file to a machine: hashing, choosing the core, checking its
//! config, and creating the machine (core-contract.md §13.3, steps 2 to 5
//! of "The path, step by step").
//!
//! [`prepare`] does everything that may take time and touches no machine:
//! it reads the whole file once, asks the cores, and checks the config the
//! answering core wrote. It runs on a worker thread, so its result,
//! [`Prepared`], is `Send`. [`start`] then builds the machine on the thread
//! that will drive it, because a machine need not be `Send`.

use crate::{kind_lines, valid_kinds, Cores, LaunchError};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use unemu_core_api::{
    ConfigDocumentBuf, Content, ContentReader, ContentSource, Core, CreateRequest, Error,
    ErrorKind, Identified, Machine, Message, OfferedContent, Record, Storage,
};
use unemu_core_files::{file_name, FileContent, FileStorage};

/// How much of the file one read takes while it is hashed.
const CHUNK: usize = 1 << 20;

/// Where the front end keeps what machines started from files leave behind.
/// The application chooses the directories; this crate names no host
/// location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDirs {
    /// Machines' records: `saves/CORE/aa/DIGEST/`.
    pub saves: PathBuf,
    /// Saved states: `states/CORE/aa/DIGEST/`.
    pub states: PathBuf,
}

impl HostDirs {
    /// `root/CORE/aa/DIGEST`: the content's digest in lower-case hex and
    /// its first two digits, so that what a machine leaves behind follows
    /// the content, not the file's name or place, and no directory holds
    /// more than a few hundred entries.
    fn of(root: &Path, core: &str, digest: &str) -> PathBuf {
        root.join(core).join(&digest[..2]).join(digest)
    }
}

/// Everything decided about one file, ready to start. `Send`: it is made on
/// a worker thread.
pub struct Prepared {
    core: &'static dyn Core,
    platform: &'static str,
    platform_name: &'static str,
    kind: &'static str,
    file: String,
    title: String,
    notes: Vec<Message>,
    config: ConfigDocumentBuf,
    sha256: [u8; 32],
    content: Arc<FileContent>,
    saves: PathBuf,
    states: PathBuf,
}

// A prepared file crosses from the worker that hashed it to the thread that
// starts it. A field that is not `Send` fails the build here.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Prepared>();
    assert_send::<Outcome>();
    assert_send::<LaunchError>();
};

impl Prepared {
    /// The core that runs the file.
    pub fn core(&self) -> &'static dyn Core {
        self.core
    }

    /// The platform id the core answered with.
    pub fn platform(&self) -> &'static str {
        self.platform
    }

    /// The platform's name for people, for the window's title.
    pub fn platform_name(&self) -> &'static str {
        self.platform_name
    }

    /// The id of the content kind the core answered with.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The file's name, without its directory.
    pub fn file_name(&self) -> &str {
        &self.file
    }

    /// The content's own title, or the file's name when it carries none.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// What a person should know before the machine starts.
    pub fn notes(&self) -> &[Message] {
        &self.notes
    }

    /// The config the core wrote for the file.
    pub fn config(&self) -> &ConfigDocumentBuf {
        &self.config
    }

    /// The SHA-256 of the whole file.
    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// The file's length in bytes.
    pub fn size(&self) -> u64 {
        self.content.size()
    }

    /// The directory of the machine's records: `saves/CORE/aa/DIGEST`.
    pub fn saves_dir(&self) -> &Path {
        &self.saves
    }

    /// The directory of the machine's saved states:
    /// `states/CORE/aa/DIGEST`.
    pub fn states_dir(&self) -> &Path {
        &self.states
    }
}

/// What [`prepare`] decided.
// One outcome is made per opened file and moved a few times, so the size of
// `Ready` costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
pub enum Outcome {
    /// One core runs the file.
    Ready(Prepared),
    /// More than one core runs the file, and nothing chose between them. Each
    /// is checked as a `Ready` one is, in the order the cores were asked.
    Choose(Vec<Prepared>),
}

impl Outcome {
    /// The prepared file, or for [`Outcome::Choose`] the refusal that names
    /// the cores and `--core`. A dialog that asks comes when two
    /// compiled-in cores open one kind; until then an application takes
    /// this.
    pub fn into_ready(self) -> Result<Prepared, LaunchError> {
        match self {
            Self::Ready(prepared) => Ok(prepared),
            Self::Choose(choices) => Err(LaunchError::Several {
                file: choices
                    .first()
                    .map(|prepared| prepared.file.clone())
                    .unwrap_or_default(),
                cores: choices
                    .iter()
                    .map(|prepared| {
                        let description = prepared.core.description();
                        (description.name.to_owned(), description.id.to_owned())
                    })
                    .collect(),
            }),
        }
    }
}

/// Open `path`, hash all of it, and ask the cores. `wanted` is a core id the
/// person named.
///
/// The steps of core-contract.md §13.3:
///
/// - The file is opened once and read whole, whatever its size, to compute
///   its SHA-256. Every read goes through the [`FileContent`] the cores get
///   later, which fails with [`ErrorKind::Content`] when the file's length
///   or modification time changes, so a file written while it is hashed is
///   refused as changed.
/// - With `wanted`, only that core is asked. Without, the cores whose
///   content kinds list the file's extension (compared in lower case) are
///   asked first, in the order of the list, then the others: the extension
///   is a hint, and `identify` decides.
/// - Each answer is checked: the platform and the content kind are the
///   core's own, the core's id can name a directory, and `validate` on the
///   config the core wrote lists the file's digest and size and nothing
///   else. Content that needs more is refused with
///   [`LaunchError::NeedsMore`] and the labels of what is missing.
/// - A core that fails, in `identify` (other than with `Content` or
///   `Storage`, which are the file's and end the open) or in the check, is
///   left out. One core left: [`Outcome::Ready`]. Several:
///   [`Outcome::Choose`]. None: the first failed check, else
///   [`LaunchError::Unsupported`] with the message of the first core that
///   recognised the file and cannot run it, else the first core's failure
///   in `identify`, else [`LaunchError::NoCore`] with the kinds the cores
///   open.
///
/// Nothing is written: the directories of [`HostDirs`] are only named here.
pub fn prepare(
    path: &Path,
    cores: &Cores,
    wanted: Option<&str>,
    dirs: &HostDirs,
) -> Result<Outcome, LaunchError> {
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    // A core the program does not have is refused before the file is read.
    let asked = asked(path, cores, wanted)?;
    let (content, sha256) = hash(path, &file)?;

    let offered = OfferedContent::new(&file, sha256, content.size(), content.as_ref());
    let mut answers = Vec::new();
    let mut unsupported = None;
    // A core that fails is left out, so that one broken core does not keep
    // another from opening the file; its failure is told only when no core
    // is left to run it.
    let mut failed = None;
    for core in &asked {
        match core.identify(&offered) {
            Ok(None) => {}
            Ok(Some(identified)) => answers.push((*core, identified)),
            Err(error) => match error.kind {
                ErrorKind::Unsupported => {
                    unsupported.get_or_insert((*core, error.message));
                }
                // The file itself failed: no other core can do better.
                ErrorKind::Content => return Err(LaunchError::Changed { file }),
                ErrorKind::Storage => {
                    return Err(LaunchError::Read {
                        file,
                        reason: error.message,
                    })
                }
                _ => {
                    failed.get_or_insert_with(|| core_failed(*core, &file, error.message));
                }
            },
        }
    }

    let digest = hex(&sha256);
    let mut prepared = Vec::new();
    let mut refused = None;
    for (core, identified) in answers {
        let checked = Checked {
            file: &file,
            sha256,
            content: &content,
            digest: &digest,
            dirs,
        };
        match check(core, identified, checked) {
            Ok(one) => prepared.push(one),
            Err(error) => {
                refused.get_or_insert(error);
            }
        }
    }

    match prepared.len() {
        // A core that ran the file and refused its own answer speaks first,
        // then one that recognised it, then one that failed on it.
        0 => Err(refused
            .or_else(|| {
                unsupported.map(|(core, reason)| LaunchError::Unsupported {
                    file: file.clone(),
                    core: core.description().name.to_owned(),
                    reason,
                })
            })
            .or(failed)
            .unwrap_or_else(|| match wanted {
                Some(_) => LaunchError::NotOpenedBy {
                    kinds: kind_lines(asked.iter().copied()),
                    core: asked
                        .first()
                        .map(|core| core.description().name.to_owned())
                        .unwrap_or_default(),
                    file,
                },
                None => LaunchError::NoCore {
                    kinds: kind_lines(cores.usable()),
                    file,
                },
            })),
        1 => Ok(Outcome::Ready(prepared.remove(0))),
        _ => Ok(Outcome::Choose(prepared)),
    }
}

/// Open the file once, wrap it as verified content and hash all of it
/// through that wrapper.
fn hash(path: &Path, file: &str) -> Result<(Arc<FileContent>, [u8; 32]), LaunchError> {
    let read = |reason: String| LaunchError::Read {
        file: file.to_owned(),
        reason,
    };
    let opened = File::open(path).map_err(|error| read(error.to_string()))?;
    let metadata = opened.metadata().map_err(|error| read(error.to_string()))?;
    if !metadata.is_file() {
        return Err(read("it is not a file".to_owned()));
    }
    let content = FileContent::from_file(opened, metadata.len())
        .map_err(|error| reading_failed(error, file))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; CHUNK];
    let mut offset = 0;
    while offset < content.size() {
        let length = usize::try_from(content.size() - offset).map_or(CHUNK, |left| left.min(CHUNK));
        let chunk = &mut buffer[..length];
        content
            .read_at(offset, chunk)
            .map_err(|error| reading_failed(error, file))?;
        hasher.update(&*chunk);
        offset += length as u64;
    }
    Ok((Arc::new(content), hasher.finalize().into()))
}

/// A failed read of the content: `Content` when it changed, the host's
/// failure otherwise.
fn reading_failed(error: Error, file: &str) -> LaunchError {
    match error.kind {
        ErrorKind::Content => LaunchError::Changed {
            file: file.to_owned(),
        },
        _ => LaunchError::Read {
            file: file.to_owned(),
            reason: error.message,
        },
    }
}

/// The cores to ask, in order: `wanted` alone, or those that list the
/// file's extension first and then the others.
fn asked(
    path: &Path,
    cores: &Cores,
    wanted: Option<&str>,
) -> Result<Vec<&'static dyn Core>, LaunchError> {
    if let Some(id) = wanted {
        if let Some((_, reason)) = cores
            .refused()
            .iter()
            .find(|(description, _)| description.id == id)
        {
            return Err(LaunchError::Unusable {
                id: id.to_owned(),
                reason: reason.clone(),
            });
        }
        return match cores.by_id(id) {
            Some(core) => Ok(vec![core]),
            None => Err(LaunchError::UnknownCore {
                id: id.to_owned(),
                known: cores
                    .usable()
                    .map(|core| core.description().id.to_owned())
                    .collect(),
            }),
        };
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let lists = |core: &&'static dyn Core| {
        extension.as_deref().is_some_and(|extension| {
            core.description().platforms.iter().any(|platform| {
                platform
                    .content
                    .iter()
                    .filter(|kind| kind.is_valid())
                    .any(|kind| kind.extensions.contains(&extension))
            })
        })
    };
    let (first, rest): (Vec<_>, Vec<_>) = cores.usable().partition(lists);
    Ok(first.into_iter().chain(rest).collect())
}

/// What [`check`] needs of the file besides the answer.
struct Checked<'a> {
    file: &'a str,
    sha256: [u8; 32],
    content: &'a Arc<FileContent>,
    digest: &'a str,
    dirs: &'a HostDirs,
}

/// Check one answer: its platform is the core's, and `validate` on its
/// config lists the file and nothing else.
fn check(
    core: &'static dyn Core,
    identified: Identified,
    file: Checked<'_>,
) -> Result<Prepared, LaunchError> {
    let description = core.description();
    let platform = description
        .platforms
        .iter()
        .find(|platform| platform.id == identified.platform)
        .ok_or_else(|| {
            core_failed(
                core,
                file.file,
                format!(
                    "it answered with the platform {}, which it does not describe",
                    identified.platform
                ),
            )
        })?;
    if !valid_kinds(platform.content).any(|kind| kind.id == identified.kind) {
        return Err(core_failed(
            core,
            file.file,
            format!(
                "it answered with the content kind {}, which its platform {} does not list",
                identified.kind, platform.id
            ),
        ));
    }
    if !is_directory_name(description.id) {
        return Err(core_failed(
            core,
            file.file,
            format!(
                "its id {} cannot name a directory for its records",
                description.id
            ),
        ));
    }
    let validated = core
        .validate(platform.id, identified.config.as_document())
        .map_err(|error| {
            core_failed(
                core,
                file.file,
                format!("it refused the config it wrote: {}", error.message),
            )
        })?;
    let size = file.content.size();
    let (own, missing): (Vec<_>, Vec<_>) = validated
        .content
        .into_iter()
        .partition(|piece| piece.sha256 == file.sha256 && piece.size == size);
    if !missing.is_empty() {
        return Err(LaunchError::NeedsMore {
            file: file.file.to_owned(),
            missing: missing.into_iter().map(|piece| piece.label).collect(),
        });
    }
    if own.is_empty() {
        return Err(core_failed(
            core,
            file.file,
            "the config it wrote does not name the file".to_owned(),
        ));
    }
    Ok(Prepared {
        core,
        platform: platform.id,
        platform_name: platform.name,
        kind: identified.kind,
        file: file.file.to_owned(),
        title: identified.title.unwrap_or_else(|| file.file.to_owned()),
        notes: identified.notes,
        config: identified.config,
        sha256: file.sha256,
        content: Arc::clone(file.content),
        saves: HostDirs::of(&file.dirs.saves, description.id, file.digest),
        states: HostDirs::of(&file.dirs.states, description.id, file.digest),
    })
}

/// Whether a core id may become one directory under the save roots: 1 to
/// 64 bytes of ASCII letters, digits, `-`, `_` and `.`, not starting with a
/// dot, so it is one path component on every host and never `.` or `..`.
fn is_directory_name(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn core_failed(core: &dyn Core, file: &str, reason: String) -> LaunchError {
    LaunchError::Core {
        file: file.to_owned(),
        core: core.description().name.to_owned(),
        reason,
    }
}

/// Lower-case hex digits of a digest.
fn hex(digest: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    text
}

/// The one piece of content a machine started from a file may ask for.
struct OneFile<'a>(&'a Prepared);

impl ContentSource for OneFile<'_> {
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error> {
        if *sha256 == self.0.sha256 && size == self.0.size() {
            Ok(Content::Reader(Box::new(Shared(Arc::clone(
                &self.0.content,
            )))))
        } else {
            Err(Error::new(
                ErrorKind::Content,
                "a machine opened from a file has that one file and nothing else",
            ))
        }
    }
}

/// The verified file, shared by every reader a machine asks for.
struct Shared(Arc<FileContent>);

impl ContentReader for Shared {
    fn size(&self) -> u64 {
        self.0.size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.0.read_at(offset, buf)
    }
}

/// The machine's storage, watched for the one refusal a person can act on:
/// a record that another machine holds.
struct Watched {
    files: FileStorage,
    /// Set when the storage refused a valid record name: its lock is held,
    /// here or in another program.
    held: Cell<bool>,
}

impl Storage for Watched {
    fn open(&self, name: &str) -> Result<Box<dyn Record>, Error> {
        self.files.open(name).inspect_err(|error| {
            if error.kind == ErrorKind::Usage && file_name(name).is_some() {
                self.held.set(true);
            }
        })
    }
}

/// Build the machine. On the thread that will drive it: a machine need not
/// be `Send` (§3.5).
///
/// The request names the platform and the config the core wrote, a content
/// source that answers the file's digest with the verified file and nothing
/// else, and a storage over the files of [`Prepared::saves_dir`], created
/// when missing. The core's error comes back as it is, with one exception:
/// when the storage refused to open a record because another machine holds
/// it, here or in another program, and `create` failed with that
/// [`ErrorKind::Usage`], the error is `Usage` with the sentence the
/// application shows. Any other `Usage`, such as a record name the storage
/// does not take or a read past a record's end, is the core's and comes
/// back unchanged.
pub fn start(prepared: &Prepared) -> Result<Box<dyn Machine>, Error> {
    let storage = Rc::new(Watched {
        files: FileStorage::open(&prepared.saves)?,
        held: Cell::new(false),
    });
    let source = OneFile(prepared);
    let request = CreateRequest::new(
        prepared.platform,
        prepared.config.as_document(),
        &source,
        Rc::clone(&storage) as Rc<dyn Storage>,
    );
    prepared.core.create(&request).map_err(|error| {
        if error.kind == ErrorKind::Usage && storage.held.get() {
            Error::new(
                ErrorKind::Usage,
                format!(
                    "{} is already running in another machine, here or in another program, and its saved data is in use. Stop that machine first.",
                    prepared.file
                ),
            )
        } else {
            error
        }
    })
}

#[cfg(test)]
mod tests;
