// SPDX-License-Identifier: AGPL-3.0-or-later
//! From a file to a machine: hashing, choosing the core, checking its
//! config, and creating the machine (core-contract.md §13.3, steps 2 to 5
//! of "The path, step by step"), and the same start for a machine whose
//! content and storage a platform's own code gathered (§14.2.5).
//!
//! [`prepare`] does everything that may take time and touches no machine:
//! it reads the whole file once, asks the cores, and checks the config the
//! answering core wrote. It runs on a worker thread, so its result,
//! [`Prepared`], is `Send`. [`start`] then builds the machine on the thread
//! that will drive it, because a machine need not be `Send`.
//!
//! A platform whose machines start from more than one file, such as a
//! stored PC image with its firmware, disks and journals, makes its
//! [`Prepared`] with [`Prepared::from_parts`] instead: the config, a
//! [`ContentPlan`] and a [`StoragePlan`], each made on the worker and opened
//! on the machine's thread at every start. [`start_with`] lets that
//! platform's code build its own machine type from the same request.

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

/// The content of one start, made on a worker thread and opened on the
/// thread that drives the machine. It is opened again at every start of
/// the same [`Prepared`] (a power cycle starts the machine anew), so
/// `open` must give an equal source each time.
pub trait ContentPlan: Send {
    /// The source the machine's `create` reads its content from.
    fn open(&self) -> Result<Box<dyn ContentSource>, Error>;
}

/// The machine's storage, made and opened as a [`ContentPlan`] is: on a
/// worker, then on the machine's thread at every start, each time over the
/// same records.
pub trait StoragePlan: Send {
    /// The storage the machine keeps for its life.
    fn open(&self) -> Result<Rc<dyn Storage>, Error>;
}

/// Everything a platform's own code decided for one start, for
/// [`Prepared::from_parts`].
pub struct Parts {
    /// The core that runs the machine.
    pub core: &'static dyn Core,
    /// The platform id, in its current spelling. When the core does not
    /// describe it, its name for people is the id, and `create` refuses it.
    pub platform: &'static str,
    /// The machine's name for people, for the window's title.
    pub title: String,
    /// What a person should know before the machine starts.
    pub notes: Vec<Message>,
    /// The config `create` reads.
    pub config: ConfigDocumentBuf,
    /// The content `create` reads, by digest and size.
    pub content: Box<dyn ContentPlan>,
    /// The machine's records.
    pub storage: Box<dyn StoragePlan>,
    /// Run-only options, by the keys the platform describes.
    pub options: Vec<(String, String)>,
}

/// Everything decided about one start, ready to start: a file [`prepare`]
/// checked, or the [`Parts`] a platform's code gathered. `Send`: it is made
/// on a worker thread.
pub struct Prepared {
    core: &'static dyn Core,
    platform: &'static str,
    platform_name: &'static str,
    title: String,
    notes: Vec<Message>,
    config: ConfigDocumentBuf,
    options: Vec<(String, String)>,
    source: Source,
}

/// Where a start's content and records come from.
enum Source {
    /// One file, from [`prepare`].
    File(Opened),
    /// Plans, from [`Prepared::from_parts`].
    Parts {
        content: Box<dyn ContentPlan>,
        storage: Box<dyn StoragePlan>,
    },
}

/// What [`prepare`] learnt about the file it opened.
struct Opened {
    kind: &'static str,
    file: String,
    sha256: [u8; 32],
    content: Arc<FileContent>,
    saves: PathBuf,
    states: PathBuf,
}

// A prepared start crosses from the worker that made it to the thread that
// starts it. A field that is not `Send` fails the build here.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Prepared>();
    assert_send::<Parts>();
    assert_send::<Outcome>();
    assert_send::<LaunchError>();
};

impl Prepared {
    /// A start from the parts a platform's code gathered. Nothing is
    /// checked here: the core's `create` checks the config and asks the
    /// content and the records it needs.
    pub fn from_parts(parts: Parts) -> Prepared {
        let platform_name = parts
            .core
            .description()
            .platforms
            .iter()
            .find(|platform| platform.id == parts.platform)
            .map_or(parts.platform, |platform| platform.name);
        Prepared {
            core: parts.core,
            platform: parts.platform,
            platform_name,
            title: parts.title,
            notes: parts.notes,
            config: parts.config,
            options: parts.options,
            source: Source::Parts {
                content: parts.content,
                storage: parts.storage,
            },
        }
    }

    /// Run-only options the application adds before the start, after those
    /// it has, by the keys the platform describes. The core refuses a key it
    /// does not describe when the machine is created. Nothing here merges or
    /// refuses a key given twice: `create` sees both, in this order, and the
    /// core decides what a repeated key means.
    pub fn with_options(mut self, options: Vec<(String, String)>) -> Prepared {
        self.options.extend(options);
        self
    }

    /// The core that runs the machine.
    pub fn core(&self) -> &'static dyn Core {
        self.core
    }

    /// The platform id the core answered with, or the one of the parts.
    pub fn platform(&self) -> &'static str {
        self.platform
    }

    /// The platform's name for people, for the window's title.
    pub fn platform_name(&self) -> &'static str {
        self.platform_name
    }

    /// The id of the content kind the core answered with; `None` for a
    /// start from parts, which opens no file.
    pub fn kind(&self) -> Option<&'static str> {
        self.opened().map(|opened| opened.kind)
    }

    /// The file's name, without its directory; `None` for a start from
    /// parts.
    pub fn file_name(&self) -> Option<&str> {
        self.opened().map(|opened| opened.file.as_str())
    }

    /// The content's own title, or the file's name when it carries none;
    /// for a start from parts, the title of the parts.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// What a person should know before the machine starts.
    pub fn notes(&self) -> &[Message] {
        &self.notes
    }

    /// The config the core wrote for the file, or the one of the parts.
    pub fn config(&self) -> &ConfigDocumentBuf {
        &self.config
    }

    /// The SHA-256 of the whole file; `None` for a start from parts.
    pub fn sha256(&self) -> Option<&[u8; 32]> {
        self.opened().map(|opened| &opened.sha256)
    }

    /// The file's length in bytes; `None` for a start from parts.
    pub fn size(&self) -> Option<u64> {
        self.opened().map(|opened| opened.content.size())
    }

    /// The directory of the machine's records: `saves/CORE/aa/DIGEST`;
    /// `None` for a start from parts, whose records are its storage plan's.
    pub fn saves_dir(&self) -> Option<&Path> {
        self.opened().map(|opened| opened.saves.as_path())
    }

    /// The directory of the machine's saved states:
    /// `states/CORE/aa/DIGEST`; `None` for a start from parts.
    pub fn states_dir(&self) -> Option<&Path> {
        self.opened().map(|opened| opened.states.as_path())
    }

    /// The file this start opened, if it is one.
    fn opened(&self) -> Option<&Opened> {
        match &self.source {
            Source::File(opened) => Some(opened),
            Source::Parts { .. } => None,
        }
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
                    .and_then(|prepared| prepared.file_name())
                    .map(str::to_owned)
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
        title: identified.title.unwrap_or_else(|| file.file.to_owned()),
        notes: identified.notes,
        config: identified.config,
        options: Vec::new(),
        source: Source::File(Opened {
            kind: identified.kind,
            file: file.file.to_owned(),
            sha256: file.sha256,
            content: Arc::clone(file.content),
            saves: HostDirs::of(&file.dirs.saves, description.id, file.digest),
            states: HostDirs::of(&file.dirs.states, description.id, file.digest),
        }),
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
        match self.0.opened() {
            Some(opened) if *sha256 == opened.sha256 && size == opened.content.size() => Ok(
                Content::Reader(Box::new(Shared(Arc::clone(&opened.content)))),
            ),
            _ => Err(Error::new(
                ErrorKind::Content,
                "a machine opened from a file has that one file and nothing else",
            )),
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
/// be `Send` (§3.5). This is [`start_with`] with the core's own `create`.
pub fn start(prepared: &Prepared) -> Result<Box<dyn Machine>, Error> {
    start_with(prepared, |request| prepared.core.create(request))
}

/// [`start`], with the caller making the machine from the request, so that
/// a platform's own code can keep its machine's type: `create` is that
/// platform's typed form of `Core::create`. On the thread that will drive
/// the machine.
///
/// The request names the platform, the config and the run-only options of
/// `prepared`, with its content and storage:
///
/// - For a file, a content source that answers the file's digest with the
///   verified file and nothing else, and a storage over the files of
///   [`Prepared::saves_dir`], created when missing. The error of `create`
///   comes back as it is, with one exception: when the storage refused to
///   open a record because another machine holds it, here or in another
///   program, and `create` failed with that [`ErrorKind::Usage`], the error
///   is `Usage` with the sentence the application shows. Any other `Usage`,
///   such as a record name the storage does not take or a read past a
///   record's end, is the core's and comes back unchanged.
/// - For parts, the source and the storage their plans open now, each
///   opened again at every start. A plan's failure and the error of
///   `create` come back as they are: the plans are the platform's, and so
///   is the sentence for a record another machine holds.
pub fn start_with<M>(
    prepared: &Prepared,
    create: impl FnOnce(&CreateRequest<'_>) -> Result<M, Error>,
) -> Result<M, Error> {
    let options: Vec<(&str, &str)> = prepared
        .options
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    match &prepared.source {
        Source::File(opened) => {
            let storage = Rc::new(Watched {
                files: FileStorage::open(&opened.saves)?,
                held: Cell::new(false),
            });
            let source = OneFile(prepared);
            let shared = Rc::clone(&storage) as Rc<dyn Storage>;
            create(&request(prepared, &options, &source, shared)).map_err(|error| {
                if error.kind == ErrorKind::Usage && storage.held.get() {
                    Error::new(
                        ErrorKind::Usage,
                        format!(
                            "{} is already running in another machine, here or in another program, and its saved data is in use. Stop that machine first.",
                            opened.file
                        ),
                    )
                } else {
                    error
                }
            })
        }
        Source::Parts { content, storage } => {
            let source = content.open()?;
            let storage = storage.open()?;
            create(&request(prepared, &options, source.as_ref(), storage))
        }
    }
}

/// The request of one start of `prepared`.
fn request<'a>(
    prepared: &'a Prepared,
    options: &'a [(&'a str, &'a str)],
    content: &'a dyn ContentSource,
    storage: Rc<dyn Storage>,
) -> CreateRequest<'a> {
    CreateRequest::new(
        prepared.platform,
        prepared.config.as_document(),
        content,
        storage,
    )
    .with_options(options)
}

#[cfg(test)]
mod tests;
