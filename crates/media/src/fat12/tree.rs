// SPDX-License-Identifier: AGPL-3.0-or-later
//! The host tree a request names, scanned without following symbolic links,
//! ordered by the bytes of host names and given short names folder by folder.
//! Only names, kinds and lengths are read here; host timestamps never are.

use super::{names, BuildError, Rename, Request, ShortName, SkipReason, Skipped};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

/// One file or folder. Nodes are in pre-order: a folder precedes its members,
/// and members follow in host-name order.
#[derive(Clone, Debug)]
pub(super) struct Node {
    pub parent: Option<usize>,
    /// `None` for the top folder, which has no host counterpart.
    pub host: Option<PathBuf>,
    pub name: ShortName,
    /// A file's length; `None` for a folder.
    pub size: Option<u64>,
    pub dos_path: String,
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
const MAX_FOLDER_PATH: usize = 64;

struct Candidate {
    host: PathBuf,
    name: OsString,
    size: Option<u64>,
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
    let mut seen: HashMap<(u64, u64), &PathBuf> = HashMap::new();
    let mut sources = Vec::new();
    for path in &request.sources {
        let name = path
            .file_name()
            .ok_or_else(|| BuildError::Unnamed { path: path.clone() })?;
        let metadata = fs::symlink_metadata(path).map_err(io_error(path))?;
        let size = kind(name, &metadata, &outputs).map_err(|reason| BuildError::Refused {
            path: path.clone(),
            reason,
        })?;
        if let Some(first) = identity(&metadata).and_then(|id| seen.insert(id, path)) {
            return Err(BuildError::SameSource {
                first: first.clone(),
                second: path.clone(),
            });
        }
        sources.push(Candidate {
            host: path.clone(),
            name: name.to_owned(),
            size,
        });
    }
    // Two sources may share a final name; their full paths break the tie.
    sources.sort_by(|a, b| {
        (
            a.name.as_encoded_bytes(),
            a.host.as_os_str().as_encoded_bytes(),
        )
            .cmp(&(
                b.name.as_encoded_bytes(),
                b.host.as_os_str().as_encoded_bytes(),
            ))
    });
    let mut tree = Tree::default();
    let (parent, prefix) = match request.top {
        Some(top) => {
            tree.nodes.push(Node {
                parent: None,
                host: None,
                name: top,
                size: None,
                dos_path: top.to_string(),
            });
            (Some(0), top.to_string())
        }
        None => (None, String::new()),
    };
    add(&mut tree, parent, &prefix, sources, &outputs)?;
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

/// A folder's members in host-name order; the ones that cannot be copied are
/// recorded as skipped.
fn members(
    folder: &Path,
    outputs: &HashSet<(u64, u64)>,
    skipped: &mut Vec<Skipped>,
) -> Result<Vec<Candidate>, BuildError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(folder).map_err(io_error(folder))? {
        let entry = entry.map_err(io_error(folder))?;
        entries.push((entry.file_name(), entry.path()));
    }
    entries.sort_by(|a, b| a.0.as_encoded_bytes().cmp(b.0.as_encoded_bytes()));
    let mut candidates = Vec::new();
    for (name, host) in entries {
        let metadata = fs::symlink_metadata(&host).map_err(io_error(&host))?;
        match kind(&name, &metadata, outputs) {
            Ok(size) => candidates.push(Candidate { host, name, size }),
            Err(reason) => skipped.push(Skipped { host, reason }),
        }
    }
    Ok(candidates)
}

fn add(
    tree: &mut Tree,
    parent: Option<usize>,
    prefix: &str,
    candidates: Vec<Candidate>,
    outputs: &HashSet<(u64, u64)>,
) -> Result<(), BuildError> {
    let hosts: Vec<&OsStr> = candidates.iter().map(|c| c.name.as_os_str()).collect();
    let names = names::assign(&hosts).map_err(|index| BuildError::NoShortName {
        path: candidates[index].host.clone(),
    })?;
    for (candidate, (name, renamed)) in candidates.into_iter().zip(names) {
        let dos_path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}\\{name}")
        };
        if candidate.size.is_none() && 1 + dos_path.len() > MAX_FOLDER_PATH {
            return Err(BuildError::PathTooLong {
                host: candidate.host,
                length: 1 + dos_path.len(),
                dos_path: format!("\\{dos_path}"),
            });
        }
        if renamed {
            tree.renames.push(Rename {
                host: candidate.host.clone(),
                dos_path: dos_path.clone(),
            });
        }
        let index = tree.nodes.len();
        tree.nodes.push(Node {
            parent,
            host: Some(candidate.host.clone()),
            name,
            size: candidate.size,
            dos_path: dos_path.clone(),
        });
        if candidate.size.is_none() {
            let members = members(&candidate.host, outputs, &mut tree.skipped)?;
            add(tree, Some(index), &dos_path, members, outputs)?;
        }
    }
    Ok(())
}
