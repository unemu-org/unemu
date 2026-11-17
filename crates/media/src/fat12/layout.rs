// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cluster allocation and directory entries for one disk of a plan;
//! Microsoft FAT Specification, FAT Data Structure (FAT12 entries) and FAT
//! Directory Structure (32-byte entries, `.` and `..` in every subdirectory).

use super::tree::Tree;
use super::{metadata, BuildError, DosTime, FloppyFormat, ENTRY};
use std::fs;
use std::io::Read;
use std::path::Path;

const ATTRIBUTE_DIRECTORY: u8 = 0x10;
/// FAT12 end-of-chain mark.
const END_OF_CHAIN: u16 = 0xFFF;

/// Clusters a folder with `entries` entries needs, its `.` and `..` included.
/// A folder always owns at least one cluster.
pub(super) fn folder_clusters(format: FloppyFormat, entries: usize) -> u64 {
    (entries as u64 * ENTRY as u64)
        .div_ceil(format.cluster_bytes())
        .max(1)
}

/// Clusters a file of `bytes` needs; an empty file owns none.
pub(super) fn file_clusters(format: FloppyFormat, bytes: u64) -> u64 {
    bytes.div_ceil(format.cluster_bytes())
}

/// Members of each folder and of the root, in tree order.
pub(super) fn members(tree: &Tree, member: &[bool]) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut folders = vec![Vec::new(); tree.nodes.len()];
    let mut root = Vec::new();
    for (index, node) in tree.nodes.iter().enumerate() {
        if member[index] {
            match node.parent {
                Some(parent) => folders[parent].push(index),
                None => root.push(index),
            }
        }
    }
    (folders, root)
}

/// The volume-wide values of one disk: the label, the serial and the time
/// the label entry carries.
#[derive(Clone, Copy, Debug)]
pub(super) struct Stamp {
    pub label: Option<[u8; 11]>,
    pub serial: u32,
    pub time: DosTime,
}

/// The whole image of one disk. The plan has already checked that the
/// members fit, so allocation never runs past the data area.
pub(super) fn image(
    format: FloppyFormat,
    tree: &Tree,
    member: &[bool],
    stamp: Stamp,
) -> Result<Vec<u8>, BuildError> {
    let label = stamp.label;
    let mut image = metadata(format, stamp);
    image.resize(format.image_bytes() as usize, 0);
    let (folders, root) = members(tree, member);
    let cluster_bytes = format.cluster_bytes() as usize;
    let fat = format.fat_start()..format.fat_start() + format.fat_bytes();

    // Contiguous chains in tree order: a folder's clusters precede its members'.
    let mut first = vec![0u16; tree.nodes.len()];
    let mut next = 2u16;
    for (index, node) in tree.nodes.iter().enumerate() {
        if !member[index] {
            continue;
        }
        let count = match node.size {
            Some(bytes) => file_clusters(format, bytes),
            None => folder_clusters(format, folders[index].len() + 2),
        } as u16;
        if count == 0 {
            continue;
        }
        first[index] = next;
        for cluster in next..next + count {
            let link = if cluster + 1 < next + count {
                cluster + 1
            } else {
                END_OF_CHAIN
            };
            set_fat12(&mut image[fat.clone()], cluster, link);
        }
        next += count;
    }

    let data = |cluster: u16| format.data_start() + usize::from(cluster - 2) * cluster_bytes;
    let entry_of = |index: usize| {
        let node = &tree.nodes[index];
        let attributes = if node.size.is_none() {
            ATTRIBUTE_DIRECTORY
        } else {
            0
        };
        (
            *node.name.as_bytes(),
            attributes,
            first[index],
            node.size.unwrap_or(0) as u32,
            node.time,
        )
    };
    let root_slots = format.root_start() + usize::from(label.is_some()) * ENTRY;
    for (slot, &index) in root.iter().enumerate() {
        let at = root_slots + slot * ENTRY;
        write_entry(&mut image[at..at + ENTRY], entry_of(index));
    }
    for (index, node) in tree.nodes.iter().enumerate() {
        if !member[index] {
            continue;
        }
        match (node.size, &node.host) {
            (None, _) => {
                // `..` of a first-level folder points at cluster 0, the root.
                let parent = node.parent.map_or(0, |parent| first[parent]);
                let dots = [
                    (
                        *b".          ",
                        ATTRIBUTE_DIRECTORY,
                        first[index],
                        0,
                        node.time,
                    ),
                    (*b"..         ", ATTRIBUTE_DIRECTORY, parent, 0, node.time),
                ];
                let entries = dots
                    .into_iter()
                    .chain(folders[index].iter().map(|&member| entry_of(member)));
                for (slot, entry) in entries.enumerate() {
                    let at = data(first[index]) + slot * ENTRY;
                    write_entry(&mut image[at..at + ENTRY], entry);
                }
            }
            (Some(1..), _) if node.contents.is_some() => {
                let contents = node.contents.as_deref().unwrap_or_default();
                let at = data(first[index]);
                image[at..at + contents.len()].copy_from_slice(contents);
            }
            (Some(bytes @ 1..), Some(host)) => {
                let contents = read_host(host, bytes)?;
                let at = data(first[index]);
                image[at..at + contents.len()].copy_from_slice(&contents);
            }
            _ => {}
        }
    }
    image.copy_within(fat.clone(), fat.end);
    Ok(image)
}

fn set_fat12(fat: &mut [u8], cluster: u16, value: u16) {
    let at = usize::from(cluster) + usize::from(cluster) / 2;
    let [low, high] = value.to_le_bytes();
    if cluster.is_multiple_of(2) {
        fat[at] = low;
        fat[at + 1] = (fat[at + 1] & 0xF0) | (high & 0x0F);
    } else {
        fat[at] = (fat[at] & 0x0F) | (low << 4);
        fat[at + 1] = (value >> 4) as u8;
    }
}

/// One directory entry: name, attributes, first cluster, length and time.
type EntryFields = ([u8; 11], u8, u16, u32, DosTime);

/// A 32-byte short entry with no attributes but the directory bit where it
/// applies.
fn write_entry(entry: &mut [u8], (name, attributes, cluster, bytes, time): EntryFields) {
    entry.fill(0);
    entry[..11].copy_from_slice(&name);
    entry[11] = attributes;
    stamp_entry(entry, time);
    entry[26..28].copy_from_slice(&cluster.to_le_bytes());
    entry[28..32].copy_from_slice(&bytes.to_le_bytes());
}

/// Write `time` as the entry's creation, last-access and last-write time
/// (FAT Directory Structure: DIR_CrtTime, DIR_CrtDate, DIR_LstAccDate,
/// DIR_WrtTime, DIR_WrtDate). DOS before 4.0 reads only the write time and
/// date; the others repeat it so every reader sees one time. The tenths of
/// creation time stay zero, as the seconds are even.
pub(super) fn stamp_entry(entry: &mut [u8], time: DosTime) {
    for (offset, word) in [
        (14, time.time()),
        (16, time.date()),
        (18, time.date()),
        (22, time.time()),
        (24, time.date()),
    ] {
        entry[offset..offset + 2].copy_from_slice(&word.to_le_bytes());
    }
}

/// Read exactly the planned length without following a symbolic link that
/// replaced the file since it was scanned. A pipe or device swapped in is
/// opened without blocking and then refused; the non-blocking flag does not
/// change reads of a regular file.
fn read_host(path: &Path, bytes: u64) -> Result<Vec<u8>, BuildError> {
    let io_error = |source| BuildError::Io {
        path: path.to_owned(),
        source,
    };
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(io_error)?;
    let changed = || BuildError::Changed {
        path: path.to_owned(),
    };
    if !file.metadata().map_err(io_error)?.is_file() {
        return Err(changed());
    }
    let mut contents = Vec::with_capacity(bytes as usize);
    file.take(bytes + 1)
        .read_to_end(&mut contents)
        .map_err(io_error)?;
    if contents.len() as u64 != bytes {
        return Err(changed());
    }
    Ok(contents)
}
