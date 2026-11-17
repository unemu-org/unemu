// SPDX-License-Identifier: AGPL-3.0-or-later
//! The host tree a request names, scanned without following symbolic links,
//! given short names folder by folder in host-name order, then ordered files
//! first, then folders, each by the bytes of the short names.
//! Only names, kinds and lengths are read here; host timestamps never are.

use super::{names, BuildError, DosTime, Rename, Request, ShortName, SkipReason, Skipped};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

/// One file or folder. Nodes are in pre-order: a folder precedes its members,
/// and members follow in name order: files first, then folders, each by the
/// bytes of their 8.3 names.
#[derive(Clone, Debug)]
pub(super) struct Node {
    pub parent: Option<usize>,
    /// `None` for the top folder, which has no host counterpart.
    pub host: Option<PathBuf>,
    pub name: ShortName,
    /// A file's length; `None` for a folder.
    pub size: Option<u64>,
    pub dos_path: String,
    /// A file's bytes when they were given in memory; host files are read
    /// only when the image is built.
    pub contents: Option<Vec<u8>>,
    /// The entry's time; a folder's `.` and `..` take it too.
    pub time: DosTime,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Tree {
    pub nodes: Vec<Node>,
    pub renames: Vec<Rename>,
    pub skipped: Vec<Skipped>,
}

/// MS-DOS keeps each drive's current directory in a 67-byte field of its
/// current directory structure: the drive letter and colon, the path from its
/// leading backslash, and a terminating NUL. A folder deeper than that could
/// not be entered with CD, and XCOPY could not create it.
pub(super) const MAX_FOLDER_PATH: usize = 64;

/// A host file or folder as it was read for a disk: its name and identity,
/// and its length or, for a folder, its members in host-name order with the
/// entries left out. Only names, kinds and lengths are read; contents and
/// timestamps never are, so planning from it reads the host no more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostItem {
    host: PathBuf,
    name: OsString,
    identity: Option<(u64, u64)>,
    kind: HostKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostKind {
    File(u64),
    Folder {
        members: Vec<HostItem>,
        skipped: Vec<Skipped>,
    },
}

impl HostItem {
    /// Read a file or folder named as a source. One that cannot be copied
    /// (a symbolic link, host metadata, a device) is refused, as
    /// [`Request::sources`] refuses it; inside a folder such entries are
    /// left out and listed.
    pub fn read(path: &Path) -> Result<Self, BuildError> {
        let outputs = HashSet::new();
        let (name, metadata, size) = named(path, &outputs)?;
        read_entry(path.to_owned(), name, &metadata, size, &outputs)
    }

    /// The host path it was read from.
    pub fn host(&self) -> &Path {
        &self.host
    }

    pub fn is_folder(&self) -> bool {
        matches!(self.kind, HostKind::Folder { .. })
    }

    /// Files it holds: one for a file, every file of its tree for a folder.
    pub fn files(&self) -> usize {
        match &self.kind {
            HostKind::File(_) => 1,
            HostKind::Folder { members, .. } => members.iter().map(Self::files).sum(),
        }
    }

    /// Folders inside it, at any depth; a folder does not count itself.
    pub fn folders(&self) -> usize {
        match &self.kind {
            HostKind::File(_) => 0,
            HostKind::Folder { members, .. } => members
                .iter()
                .map(|member| usize::from(member.is_folder()) + member.folders())
                .sum(),
        }
    }

    /// Length of every file it holds.
    pub fn bytes(&self) -> u64 {
        match &self.kind {
            HostKind::File(bytes) => *bytes,
            HostKind::Folder { members, .. } => members.iter().map(Self::bytes).sum(),
        }
    }

    /// A folder's members and the entries left out of it, not deeper.
    pub(super) fn contents(&self) -> Option<(&[HostItem], &[Skipped])> {
        match &self.kind {
            HostKind::File(_) => None,
            HostKind::Folder { members, skipped } => Some((members, skipped)),
        }
    }

    /// Entries left out anywhere inside it.
    pub fn skipped(&self) -> usize {
        match &self.kind {
            HostKind::File(_) => 0,
            HostKind::Folder { members, skipped } => {
                skipped.len() + members.iter().map(Self::skipped).sum::<usize>()
            }
        }
    }
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> BuildError + '_ {
    move |source| BuildError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Two paths that reach one host file or folder share its identity, whatever
/// their spelling: `./Game` and `game` on a case-insensitive file system, or
/// hard links. Only Unix exposes it.
#[cfg(unix)]
fn identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// A named source's final name, metadata and length (`None` for a folder),
/// or why it is refused.
fn named(
    path: &Path,
    outputs: &HashSet<(u64, u64)>,
) -> Result<(OsString, fs::Metadata, Option<u64>), BuildError> {
    let name = path
        .file_name()
        .ok_or_else(|| BuildError::Unnamed {
            path: path.to_owned(),
        })?
        .to_owned();
    let metadata = fs::symlink_metadata(path).map_err(io_error(path))?;
    let size = kind(&name, &metadata, outputs).map_err(|reason| BuildError::Refused {
        path: path.to_owned(),
        reason,
    })?;
    Ok((name, metadata, size))
}

/// A checked entry with its tree, when it is a folder.
fn read_entry(
    host: PathBuf,
    name: OsString,
    metadata: &fs::Metadata,
    size: Option<u64>,
    outputs: &HashSet<(u64, u64)>,
) -> Result<HostItem, BuildError> {
    let kind = match size {
        Some(bytes) => HostKind::File(bytes),
        None => {
            let mut skipped = Vec::new();
            let members = members(&host, outputs, &mut skipped)?;
            HostKind::Folder { members, skipped }
        }
    };
    Ok(HostItem {
        identity: identity(metadata),
        host,
        name,
        kind,
    })
}

pub(super) fn scan(request: &Request) -> Result<Tree, BuildError> {
    if request.sources.is_empty() {
        return Err(BuildError::NothingToAdd);
    }
    let outputs: HashSet<(u64, u64)> = request
        .exclude
        .iter()
        .filter_map(|path| fs::symlink_metadata(path).ok())
        .filter_map(|metadata| identity(&metadata))
        .collect();
    // Every source is checked before any folder is read.
    let mut checked = Vec::new();
    for path in &request.sources {
        checked.push((path, named(path, &outputs)?));
    }
    let items = checked
        .into_iter()
        .map(|(path, (name, metadata, size))| {
            read_entry(path.clone(), name, &metadata, size, &outputs)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let items: Vec<&HostItem> = items.iter().collect();
    distinct(&items)?;
    build(&items, request.top, Vec::new())
}

/// Refuse two items that name the same host file or folder.
pub(super) fn distinct(items: &[&HostItem]) -> Result<(), BuildError> {
    let mut seen: HashMap<(u64, u64), &Path> = HashMap::new();
    for item in items {
        if let Some(first) = item.identity.and_then(|id| seen.insert(id, &item.host)) {
            return Err(BuildError::SameSource {
                first: first.to_owned(),
                second: item.host.clone(),
            });
        }
    }
    Ok(())
}

/// Name `items` and their trees, under `top` when given. `skipped` comes
/// first in the tree's list: entries left out of a folder whose members are
/// the items.
pub(super) fn build(
    items: &[&HostItem],
    top: Option<ShortName>,
    skipped: Vec<Skipped>,
) -> Result<Tree, BuildError> {
    let mut items = items.to_vec();
    // Two sources may share a final name; their full paths break the tie.
    items.sort_by(|a, b| {
        (
            a.name.as_encoded_bytes(),
            a.host.as_os_str().as_encoded_bytes(),
        )
            .cmp(&(
                b.name.as_encoded_bytes(),
                b.host.as_os_str().as_encoded_bytes(),
            ))
    });
    let mut tree = Tree {
        skipped,
        ..Tree::default()
    };
    let (parent, prefix) = match top {
        Some(top) => {
            tree.nodes.push(Node {
                parent: None,
                host: None,
                name: top,
                size: None,
                dos_path: top.to_string(),
                contents: None,
                time: DosTime::EPOCH,
            });
            (Some(0), top.to_string())
        }
        None => (None, String::new()),
    };
    add(&mut tree, parent, &prefix, &items)?;
    Ok(tree)
}

/// Desktop metadata that host file managers and archivers scatter through
/// folders.
fn junk(name: &OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    bytes.starts_with(b"._")
        || [&b".DS_Store"[..], b"__MACOSX", b"Thumbs.db", b"desktop.ini"]
            .iter()
            .any(|junk| bytes.eq_ignore_ascii_case(junk))
}

/// A file's length, `None` for a folder, or why the entry cannot be copied.
fn kind(
    name: &OsStr,
    metadata: &fs::Metadata,
    outputs: &HashSet<(u64, u64)>,
) -> Result<Option<u64>, SkipReason> {
    let kind = metadata.file_type();
    if junk(name) {
        Err(SkipReason::HostJunk)
    } else if kind.is_symlink() {
        Err(SkipReason::SymbolicLink)
    } else if identity(metadata).is_some_and(|id| outputs.contains(&id)) {
        Err(SkipReason::Output)
    } else if kind.is_file() {
        Ok(Some(metadata.len()))
    } else if kind.is_dir() {
        Ok(None)
    } else {
        Err(SkipReason::NotRegular)
    }
}

/// A folder's members in host-name order, each read with its tree; the ones
/// that cannot be copied are recorded as skipped.
fn members(
    folder: &Path,
    outputs: &HashSet<(u64, u64)>,
    skipped: &mut Vec<Skipped>,
) -> Result<Vec<HostItem>, BuildError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(folder).map_err(io_error(folder))? {
        let entry = entry.map_err(io_error(folder))?;
        entries.push((entry.file_name(), entry.path()));
    }
    entries.sort_by(|a, b| a.0.as_encoded_bytes().cmp(b.0.as_encoded_bytes()));
    let mut checked = Vec::new();
    for (name, host) in entries {
        let metadata = fs::symlink_metadata(&host).map_err(io_error(&host))?;
        match kind(&name, &metadata, outputs) {
            Ok(size) => checked.push((host, name, metadata, size)),
            Err(reason) => skipped.push(Skipped { host, reason }),
        }
    }
    checked
        .into_iter()
        .map(|(host, name, metadata, size)| read_entry(host, name, &metadata, size, outputs))
        .collect()
}

fn add(
    tree: &mut Tree,
    parent: Option<usize>,
    prefix: &str,
    items: &[&HostItem],
) -> Result<(), BuildError> {
    let hosts: Vec<&OsStr> = items.iter().map(|item| item.name.as_os_str()).collect();
    let names = names::assign(&hosts).map_err(|index| BuildError::NoShortName {
        path: items[index].host.clone(),
    })?;
    // Names are given in host-name order, which decides numeric tails; the
    // folder is then written files first, then folders, each by the bytes of
    // the stored names, as XCOPY /S writes a tree.
    let mut named: Vec<_> = items.iter().zip(names).collect();
    named.sort_by_key(|(item, (name, _))| (item.is_folder(), *name.as_bytes()));
    for (item, (name, renamed)) in named {
        let dos_path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}\\{name}")
        };
        if item.is_folder() && 1 + dos_path.len() > MAX_FOLDER_PATH {
            return Err(BuildError::PathTooLong {
                host: item.host.clone(),
                length: 1 + dos_path.len(),
                dos_path: format!("\\{dos_path}"),
            });
        }
        if renamed {
            tree.renames.push(Rename {
                host: item.host.clone(),
                dos_path: dos_path.clone(),
            });
        }
        let index = tree.nodes.len();
        tree.nodes.push(Node {
            parent,
            host: Some(item.host.clone()),
            name,
            size: match item.kind {
                HostKind::File(bytes) => Some(bytes),
                HostKind::Folder { .. } => None,
            },
            dos_path: dos_path.clone(),
            contents: None,
            time: DosTime::EPOCH,
        });
        if let HostKind::Folder { members, skipped } = &item.kind {
            tree.skipped.extend(skipped.iter().cloned());
            let members: Vec<&HostItem> = members.iter().collect();
            add(tree, Some(index), &dos_path, &members)?;
        }
    }
    Ok(())
}
