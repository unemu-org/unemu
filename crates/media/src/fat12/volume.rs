// SPDX-License-Identifier: AGPL-3.0-or-later
//! Volumes from files given in memory, the input of a recipe's `fat` step
//! (backlog/docs/design/recipes.md §6.9). Nothing here reads the host.
//! Names are checked, never converted: every part of a path must already be
//! a DOS 8.3 name as stored, upper case (Microsoft FAT Specification, FAT
//! Directory Structure, short name field), and every folder fit the 64
//! characters DOS keeps for a current directory. In name order the volumes
//! are byte for byte those [`super::build`] makes from host files with the
//! same names after their conversion.

use super::split;
use super::tree::{Node, Tree, MAX_FOLDER_PATH};
use super::{BuildError, DosTime, FloppyFormat, Plan, ShortName, DEFAULT_SERIAL};
use std::path::PathBuf;

/// The order of the entries in each folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    /// Files first, then folders, each by the bytes of their 8.3 names as
    /// stored, as XCOPY /S writes a tree and as host files are ordered.
    #[default]
    Name,
    /// As listed: each folder's members in the order their paths first
    /// appear.
    Listed,
}

/// A file or folder of a volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The path from the volume's root, parts separated by `/`:
    /// `LEVEL1/GAME.COM`. Each part must be an 8.3 name exactly as it is
    /// stored, in upper case.
    pub path: String,
    /// A file's bytes; `None` for a folder. Folders above a file need no
    /// entry of their own; one is given for an empty folder or for a time.
    pub contents: Option<Vec<u8>>,
    /// The entry's time, else [`Volume::time`].
    pub time: Option<DosTime>,
}

impl Entry {
    /// A file without a time of its own.
    pub fn file(path: impl Into<String>, contents: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            contents: Some(contents.into()),
            time: None,
        }
    }

    /// A folder without a time of its own.
    pub fn folder(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            contents: None,
            time: None,
        }
    }

    /// The same entry with its own time.
    pub fn at(self, time: DosTime) -> Self {
        Self {
            time: Some(time),
            ..self
        }
    }
}

/// What the volumes are: the keys of a recipe's `fat` step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// The diskette format.
    pub format: FloppyFormat,
    /// The volume label, for the boot sector and the root's label entry; a
    /// split numbers it per volume as [`super::plan_split`] does.
    pub label: Option<[u8; 11]>,
    /// The serial in the boot sector.
    pub serial: u32,
    /// The order of every folder's entries.
    pub order: Order,
    /// The time of entries that state none, of folders and of the label.
    pub time: DosTime,
    /// Split over exactly this many volumes, from 2, in folder order (a
    /// folder's own files before its subfolders); `None` for one volume.
    pub volumes: Option<usize>,
}

impl Volume {
    /// One unlabelled volume of `format` in name order, with the default
    /// serial and every time at the FAT epoch: the bytes of
    /// [`super::build`].
    pub fn new(format: FloppyFormat) -> Self {
        Self {
            format,
            label: None,
            serial: DEFAULT_SERIAL,
            order: Order::Name,
            time: DosTime::EPOCH,
            volumes: None,
        }
    }
}

/// Plan the volumes of `entries`: one, or exactly [`Volume::volumes`].
/// A bad name, a path named twice, a folder too deep, a misfit or another
/// number of volumes than the split needs is an error, and the error of a
/// split names the number it needs. [`Plan::images`] then builds the images
/// without reading the host.
pub fn plan_volumes(entries: Vec<Entry>, volume: &Volume) -> Result<Plan, BuildError> {
    if let Some(requested @ 0..=1) = volume.volumes {
        return Err(BuildError::VolumeCount { requested });
    }
    let tree = tree(entries, volume)?;
    let mut plan = match volume.volumes {
        None => split::single(volume.format, volume.label, tree)?,
        Some(requested) => {
            let plan = split::greedy(volume.format, volume.label, tree)?;
            if plan.disks.len() != requested {
                return Err(BuildError::WrongVolumes {
                    format: volume.format,
                    requested,
                    needed: plan.disks.len(),
                });
            }
            plan
        }
    };
    plan.serial = volume.serial;
    plan.time = volume.time;
    Ok(plan)
}

/// A file or folder while the tree is gathered.
struct Pending {
    name: ShortName,
    /// The path as given up to this part.
    path: String,
    contents: Option<Vec<u8>>,
    folder: bool,
    time: Option<DosTime>,
    /// A folder named by an entry of its own, not only above a file.
    listed: bool,
    members: Vec<Pending>,
}

fn tree(entries: Vec<Entry>, volume: &Volume) -> Result<Tree, BuildError> {
    let mut root: Vec<Pending> = Vec::new();
    for entry in entries {
        let parts = entry
            .path
            .split('/')
            .map(|part| {
                ShortName::strict(part)
                    .map(|name| (part, name))
                    .map_err(|fault| BuildError::BadName {
                        path: entry.path.clone(),
                        name: part.to_owned(),
                        fault,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let clash = || BuildError::NameClash {
            path: entry.path.clone(),
        };
        let mut level = &mut root;
        let mut path = String::new();
        let last = parts.len() - 1;
        for (position, (given, name)) in parts.into_iter().enumerate() {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(given);
            let found = level.iter().position(|pending| pending.name == name);
            let at = match found {
                Some(at) => at,
                None => {
                    level.push(Pending {
                        name,
                        path: path.clone(),
                        contents: None,
                        folder: true,
                        time: None,
                        listed: false,
                        members: Vec::new(),
                    });
                    level.len() - 1
                }
            };
            if position < last {
                if !level[at].folder {
                    return Err(clash());
                }
                level = &mut level[at].members;
                continue;
            }
            let pending = &mut level[at];
            // The entry's own part: new, or a folder seen only above files.
            let fresh = found.is_none();
            match &entry.contents {
                Some(_) if !fresh => return Err(clash()),
                None if !fresh && (!pending.folder || pending.listed) => return Err(clash()),
                _ => {}
            }
            pending.folder = entry.contents.is_none();
            pending.listed = true;
            pending.time = entry.time;
            pending.contents = entry.contents;
            break;
        }
    }
    let mut tree = Tree::default();
    add(&mut tree, None, "", root, volume)?;
    Ok(tree)
}

/// Flatten `level` into the tree in pre-order, as the host scan does.
fn add(
    tree: &mut Tree,
    parent: Option<usize>,
    prefix: &str,
    mut level: Vec<Pending>,
    volume: &Volume,
) -> Result<(), BuildError> {
    if volume.order == Order::Name {
        level.sort_by_key(|pending| (pending.folder, *pending.name.as_bytes()));
    }
    for pending in level {
        let dos_path = if prefix.is_empty() {
            pending.name.to_string()
        } else {
            format!("{prefix}\\{}", pending.name)
        };
        if pending.folder && 1 + dos_path.len() > MAX_FOLDER_PATH {
            return Err(BuildError::PathTooLong {
                host: PathBuf::from(&pending.path),
                length: 1 + dos_path.len(),
                dos_path: format!("\\{dos_path}"),
            });
        }
        let index = tree.nodes.len();
        tree.nodes.push(Node {
            parent,
            host: Some(PathBuf::from(&pending.path)),
            name: pending.name,
            size: (!pending.folder).then(|| {
                pending
                    .contents
                    .as_ref()
                    .map_or(0, |bytes| bytes.len() as u64)
            }),
            dos_path: dos_path.clone(),
            contents: pending.contents,
            time: pending.time.unwrap_or(volume.time),
        });
        if pending.folder {
            add(tree, Some(index), &dos_path, pending.members, volume)?;
        }
    }
    Ok(())
}
