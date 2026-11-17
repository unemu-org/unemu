// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fitting a scanned tree on one disk, or spreading it over several of one
//! format. Costs count whole clusters for files and folders (a folder grows a
//! cluster at a time as entries are added) and entries of the fixed root.
//! Items are planned in folder order: a folder's own files first, then its
//! subfolders, each by name, so the loose files beside a program's
//! folders (its EXE, SETUP and README) go on the first disk.

use super::layout::{file_clusters, folder_clusters, members};
use super::tree::Tree;
use super::{BuildError, DiskPlan, DosTime, FloppyFormat, PlacedFile, Plan, DEFAULT_SERIAL};

/// One disk being filled.
struct Disk {
    member: Vec<bool>,
    /// Members of each folder on this disk, without `.` and `..`.
    entries: Vec<usize>,
    root: usize,
    clusters: u64,
}

impl Disk {
    fn new(nodes: usize) -> Self {
        Self {
            member: vec![false; nodes],
            entries: vec![0; nodes],
            root: 0,
            clusters: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.root == 0
    }
}

/// What placing one item adds to a disk.
struct Cost {
    clusters: u64,
    root: usize,
}

/// An item is a file or a folder without members: the units a split places.
/// `chain` is every folder from the root down to the item's own folder (the
/// item itself when it is a folder).
struct Item {
    chain: Vec<usize>,
    file: Option<usize>,
}

/// Files and folders without members in folder order: at every level the
/// files first, then the folders, each in tree order.
fn order(tree: &Tree) -> Vec<usize> {
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); tree.nodes.len()];
    let mut roots = Vec::new();
    for (index, node) in tree.nodes.iter().enumerate() {
        match node.parent {
            Some(parent) => members[parent].push(index),
            None => roots.push(index),
        }
    }
    fn visit(tree: &Tree, members: &[Vec<usize>], level: &[usize], out: &mut Vec<usize>) {
        out.extend(level.iter().filter(|&&i| tree.nodes[i].size.is_some()));
        for &folder in level.iter().filter(|&&i| tree.nodes[i].size.is_none()) {
            if members[folder].is_empty() {
                out.push(folder);
            } else {
                visit(tree, members, &members[folder], out);
            }
        }
    }
    let mut out = Vec::new();
    visit(tree, &members, &roots, &mut out);
    out
}

fn items(tree: &Tree) -> Vec<Item> {
    let ancestors = |mut index: Option<usize>| {
        let mut chain = Vec::new();
        while let Some(at) = index {
            chain.push(at);
            index = tree.nodes[at].parent;
        }
        chain.reverse();
        chain
    };
    order(tree)
        .into_iter()
        .map(|index| match tree.nodes[index].size {
            Some(_) => Item {
                chain: ancestors(tree.nodes[index].parent),
                file: Some(index),
            },
            None => Item {
                chain: ancestors(Some(index)),
                file: None,
            },
        })
        .collect()
}

fn cost(format: FloppyFormat, tree: &Tree, disk: &Disk, item: &Item) -> Cost {
    // A new entry at chain position p lives in chain[p - 1], or in the root
    // when p is 0; the file sits one position below the last folder.
    let mut added = vec![0; item.chain.len()];
    let mut root = 0;
    let new_entries = item
        .chain
        .iter()
        .enumerate()
        .filter(|(_, &folder)| !disk.member[folder])
        .map(|(position, _)| position)
        .chain(item.file.map(|_| item.chain.len()));
    for position in new_entries {
        match position.checked_sub(1) {
            None => root += 1,
            Some(parent) => added[parent] += 1,
        }
    }
    let mut clusters = item.file.map_or(0, |file| {
        file_clusters(format, tree.nodes[file].size.unwrap_or(0))
    });
    for (position, &folder) in item.chain.iter().enumerate() {
        let before = if disk.member[folder] {
            folder_clusters(format, disk.entries[folder] + 2)
        } else {
            0
        };
        clusters += folder_clusters(format, disk.entries[folder] + added[position] + 2) - before;
    }
    Cost { clusters, root }
}

fn place(disk: &mut Disk, item: &Item, cost: Cost) {
    for (position, &folder) in item.chain.iter().enumerate() {
        if !disk.member[folder] {
            disk.member[folder] = true;
            if position > 0 {
                disk.entries[item.chain[position - 1]] += 1;
            }
        }
    }
    if let Some(file) = item.file {
        disk.member[file] = true;
        if let Some(&folder) = item.chain.last() {
            disk.entries[folder] += 1;
        }
    }
    disk.root += cost.root;
    disk.clusters += cost.clusters;
}

fn root_capacity(format: FloppyFormat, label: Option<[u8; 11]>) -> usize {
    format.root_entries() - usize::from(label.is_some())
}

fn fits(format: FloppyFormat, label: Option<[u8; 11]>, disk: &Disk, cost: &Cost) -> bool {
    disk.clusters + cost.clusters <= format.data_clusters()
        && disk.root + cost.root <= root_capacity(format, label)
}

fn too_large(format: FloppyFormat, tree: &Tree, item: &Item, cost: &Cost) -> BuildError {
    // In practice only a file outgrows an empty disk; a folder chain would
    // have to be thousands of levels deep. Every item has a file or a chain.
    let index = item.file.or(item.chain.last().copied()).unwrap_or(0);
    let node = &tree.nodes[index];
    let bytes = node.size.unwrap_or(0);
    let folders = cost.clusters - file_clusters(format, bytes);
    BuildError::FileTooLarge {
        host: node.host.clone().unwrap_or_default(),
        dos_path: node.dos_path.clone(),
        bytes,
        room: format.data_clusters().saturating_sub(folders) * format.cluster_bytes(),
        format,
    }
}

/// Every source on one disk.
pub(super) fn single(
    format: FloppyFormat,
    label: Option<[u8; 11]>,
    tree: Tree,
) -> Result<Plan, BuildError> {
    let empty = Disk::new(tree.nodes.len());
    for item in items(&tree) {
        let cost = cost(format, &tree, &empty, &item);
        if cost.clusters > format.data_clusters() {
            return Err(too_large(format, &tree, &item, &cost));
        }
    }
    let member = vec![true; tree.nodes.len()];
    let (clusters, root) = usage(format, &tree, &member);
    if root > root_capacity(format, label) {
        return Err(BuildError::RootFull {
            format,
            needed: root + usize::from(label.is_some()),
            capacity: format.root_entries(),
        });
    }
    if clusters > format.data_clusters() {
        let needed = clusters * format.cluster_bytes();
        return Err(BuildError::DoesNotFit {
            format,
            needed,
            capacity: format.data_bytes(),
            shortfall: needed - format.data_bytes(),
        });
    }
    Ok(finish(format, tree, vec![member], label))
}

/// Greedy in folder order: each item goes on the current disk if it fits, else
/// it starts the next disk.
pub(super) fn greedy(
    format: FloppyFormat,
    label: Option<[u8; 11]>,
    tree: Tree,
) -> Result<Plan, BuildError> {
    let mut disks = vec![Disk::new(tree.nodes.len())];
    for item in items(&tree) {
        let mut current = disks.len() - 1;
        let mut placed = cost(format, &tree, &disks[current], &item);
        if !fits(format, label, &disks[current], &placed) {
            let fresh = Disk::new(tree.nodes.len());
            let alone = cost(format, &tree, &fresh, &item);
            if !fits(format, label, &fresh, &alone) {
                return Err(too_large(format, &tree, &item, &alone));
            }
            if !disks[current].is_empty() {
                disks.push(fresh);
                current += 1;
            }
            placed = alone;
        }
        place(&mut disks[current], &item, placed);
    }
    Ok(finish(
        format,
        tree,
        disks.into_iter().map(|disk| disk.member).collect(),
        label,
    ))
}

/// Data clusters and root entries a set of members uses.
fn usage(format: FloppyFormat, tree: &Tree, member: &[bool]) -> (u64, usize) {
    let (folders, root) = members(tree, member);
    let clusters = tree
        .nodes
        .iter()
        .enumerate()
        .filter(|(index, _)| member[*index])
        .map(|(index, node)| match node.size {
            Some(bytes) => file_clusters(format, bytes),
            None => folder_clusters(format, folders[index].len() + 2),
        })
        .sum();
    (clusters, root.len())
}

/// `<LABEL> k`, the label shortened so the number fits in eleven characters.
pub(super) fn numbered(label: [u8; 11], disk: usize) -> [u8; 11] {
    let suffix = format!(" {disk}");
    let text = label.trim_ascii_end();
    let keep = text.len().min(11usize.saturating_sub(suffix.len()));
    let mut numbered = [b' '; 11];
    for (out, byte) in numbered.iter_mut().zip(
        text[..keep]
            .trim_ascii_end()
            .iter()
            .chain(suffix.as_bytes()),
    ) {
        *out = *byte;
    }
    numbered
}

fn finish(
    format: FloppyFormat,
    mut tree: Tree,
    members: Vec<Vec<bool>>,
    label: Option<[u8; 11]>,
) -> Plan {
    let renames = std::mem::take(&mut tree.renames);
    let skipped = std::mem::take(&mut tree.skipped);
    let count = members.len();
    let planned = order(&tree);
    let disks = members
        .iter()
        .enumerate()
        .map(|(index, member)| {
            let (clusters, _) = usage(format, &tree, member);
            DiskPlan {
                label: label.map(|label| {
                    if count > 1 {
                        numbered(label, index + 1)
                    } else {
                        label
                    }
                }),
                files: planned
                    .iter()
                    .map(|&index| (index, &tree.nodes[index]))
                    .filter(|(index, node)| member[*index] && node.size.is_some())
                    .map(|(_, node)| PlacedFile {
                        host: node.host.clone().unwrap_or_default(),
                        dos_path: node.dos_path.clone(),
                        bytes: node.size.unwrap_or(0),
                    })
                    .collect(),
                used_bytes: clusters * format.cluster_bytes(),
                free_bytes: (format.data_clusters() - clusters) * format.cluster_bytes(),
            }
        })
        .collect();
    Plan {
        format,
        disks,
        renames,
        skipped,
        tree,
        members,
        serial: DEFAULT_SERIAL,
        time: DosTime::EPOCH,
    }
}
