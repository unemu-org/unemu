// SPDX-License-Identifier: AGPL-3.0-or-later
//! What host items, read once, come to on a disk: their DOS names, the
//! entries left out, the space DOS counts for them, and the plan of one disk
//! or of a split, for any top folder, format and label, without reading the
//! host again. Space counts as [`super::plan`] counts it (Microsoft FAT
//! Specification, FAT Data Structure and FAT Directory Structure): whole
//! clusters per file and per folder, `.` and `..` included, and the fixed
//! root directory's entries, a volume label taking one. A survey's plans and
//! images equal [`super::build`]'s for the same sources and top folder.

use super::layout::{file_clusters, folder_clusters, members};
use super::tree::{self, HostItem, Tree};
use super::{split, BuildError, FloppyFormat, Plan, Rename, ShortName, Skipped};

/// The layout of read host items on one disk format.
#[derive(Clone, Debug)]
pub struct Survey {
    format: FloppyFormat,
    label: Option<[u8; 11]>,
    tree: Tree,
    landing: Vec<String>,
    files: usize,
    folders: usize,
    file_bytes: u64,
    file_space: u64,
    folder_space: u64,
    root_entries: usize,
}

impl Survey {
    /// Lay `items` out under the folder `top`, or at the root without one.
    /// A folder added alone puts its members straight under `top`: it is the
    /// top folder, so its tree is not nested once more. Otherwise each item
    /// goes under `top`. Two items that reach the same host entry are
    /// refused; the order of `items` does not matter.
    pub fn new(
        items: &[HostItem],
        top: Option<ShortName>,
        format: FloppyFormat,
        label: Option<[u8; 11]>,
    ) -> Result<Self, BuildError> {
        if items.is_empty() {
            return Err(BuildError::NothingToAdd);
        }
        let all: Vec<&HostItem> = items.iter().collect();
        tree::distinct(&all)?;
        let alone = match items {
            [folder] => folder.contents(),
            _ => None,
        };
        let tree = match alone {
            Some((members, skipped)) => {
                let members: Vec<&HostItem> = members.iter().collect();
                tree::build(&members, top, skipped.to_vec())?
            }
            None => tree::build(&all, top, Vec::new())?,
        };
        let parent = top.map(|_| 0);
        let landing = match alone {
            Some(_) => vec![top.map_or_else(|| "\\".to_owned(), |top| format!("\\{top}\\"))],
            None => items
                .iter()
                .map(|item| {
                    tree.nodes
                        .iter()
                        .find(|node| {
                            node.parent == parent && node.host.as_deref() == Some(item.host())
                        })
                        .map_or_else(String::new, |node| match node.size {
                            Some(_) => format!("\\{}", node.dos_path),
                            None => format!("\\{}\\", node.dos_path),
                        })
                })
                .collect(),
        };
        let every = vec![true; tree.nodes.len()];
        let (folder_members, root) = members(&tree, &every);
        let (mut files, mut folders, mut file_bytes, mut file_space, mut folder_space) =
            (0, 0, 0, 0, 0);
        for (index, node) in tree.nodes.iter().enumerate() {
            match node.size {
                Some(bytes) => {
                    files += 1;
                    file_bytes += bytes;
                    file_space += file_clusters(format, bytes) * format.cluster_bytes();
                }
                None => {
                    folders += 1;
                    folder_space += folder_clusters(format, folder_members[index].len() + 2)
                        * format.cluster_bytes();
                }
            }
        }
        Ok(Self {
            format,
            label,
            root_entries: root.len() + usize::from(label.is_some()),
            tree,
            landing,
            files,
            folders,
            file_bytes,
            file_space,
            folder_space,
        })
    }

    /// Where each item lands, in the order given: its DOS path from the root,
    /// a folder's with a trailing backslash (`\NIGHTFAL\`, `\TQ\TQ.EXE`).
    /// A folder added alone lands at the top folder, or `\` without one.
    pub fn landing(&self) -> &[String] {
        &self.landing
    }

    /// Every host name that changed, in tree order.
    pub fn renames(&self) -> &[Rename] {
        &self.tree.renames
    }

    /// Every host entry left out, in tree order.
    pub fn skipped(&self) -> &[Skipped] {
        &self.tree.skipped
    }

    /// Files on the disk.
    pub fn files(&self) -> usize {
        self.files
    }

    /// Folders on the disk, the top folder included.
    pub fn folders(&self) -> usize {
        self.folders
    }

    /// The files' own length.
    pub fn file_bytes(&self) -> u64 {
        self.file_bytes
    }

    /// Data-area bytes the files take, in whole clusters.
    pub fn file_space(&self) -> u64 {
        self.file_space
    }

    /// Data-area bytes the folders take, in whole clusters.
    pub fn folder_space(&self) -> u64 {
        self.folder_space
    }

    /// Data-area bytes everything takes on one disk.
    pub fn needed(&self) -> u64 {
        self.file_space + self.folder_space
    }

    /// Entries of the fixed root directory in use, the label's included.
    pub fn root_entries(&self) -> usize {
        self.root_entries
    }

    /// Everything on one disk, as [`super::plan`] plans it.
    pub fn plan(&self) -> Result<Plan, BuildError> {
        split::single(self.format, self.label, self.tree.clone())
    }

    /// As many disks as needed, as [`super::plan_split`] plans them.
    pub fn plan_split(&self) -> Result<Plan, BuildError> {
        split::greedy(self.format, self.label, self.tree.clone())
    }
}
