// SPDX-License-Identifier: AGPL-3.0-or-later
//! A FAT12 volume reader, written independently of the builder: it decodes
//! the BPB, follows on-media cluster chains through the first FAT and walks
//! directories. Microsoft FAT Specification, Boot Sector and BPB, FAT Data
//! Structure (FAT12 entries), FAT Directory Structure, and Long File Name
//! Implementation for the long names other tools write.
//!
//! The builder's tests read every image back through it, and `upcli image
//! export --files` unpacks the disks of a set with [`Volume::tree`]
//! (backlog/docs/design/recipes.md §3.10, decision-37). An image is
//! untrusted input: every offset is checked against the image, every
//! cluster number against the data area; a chain that loops, leaves the
//! data area or reaches a free, reserved or bad cluster is refused, and so
//! is a cluster that two files or folders claim. So reading never panics,
//! never loops and never returns more file bytes than the image holds.
//! Anything that is not a FAT12 volume this reader understands is a
//! [`ReadError`], never a guess.
//!
//! Not read: FAT16 and FAT32 (a volume of 4085 clusters or more), an image
//! without the 55h AAh signature or with a BPB that does not describe the
//! image's own length, and partitioned disks.

use super::DosTime;
use std::fmt;

/// Why a volume, or a part of it, cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError(String);

impl ReadError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReadError {}

/// The attribute bits of a directory entry (FAT Directory Structure).
pub mod attribute {
    pub const READ_ONLY: u8 = 0x01;
    pub const HIDDEN: u8 = 0x02;
    pub const SYSTEM: u8 = 0x04;
    pub const VOLUME_ID: u8 = 0x08;
    pub const DIRECTORY: u8 = 0x10;
    pub const ARCHIVE: u8 = 0x20;
    /// The four low bits together: a long-name entry.
    pub const LONG_NAME: u8 = 0x0F;
}

/// The largest FAT12 volume: 4084 clusters (FAT Type Determination).
const MAX_CLUSTERS: u64 = 4084;

/// The longest name read, in bytes of UTF-8: a long name holds at most 255
/// characters (Long File Name Implementation), and the hosts' file systems
/// take at most 255 bytes.
pub const MAX_NAME: usize = 255;

/// The longest folder path of short names, `\A\B` with the leading
/// backslash: the 64 characters DOS keeps for a current directory, which
/// the builder keeps too ([`super::MAX_FOLDER_PATH`]). DOS cannot enter a
/// deeper folder, so a volume with one is no DOS disk, and the bound keeps
/// the walk of a crafted image short: at most 32 levels.
pub const MAX_FOLDER_PATH: usize = super::MAX_FOLDER_PATH;

/// The longest path of host names, `\A\B\FILE` with the leading backslash,
/// in characters: MAX_PATH, the 260 characters the FAT specification lets a
/// path of long names hold, its drive and terminating NUL included, so no
/// path of a volume Windows wrote is longer.
pub const MAX_PATH: usize = 260;

/// One 32-byte short directory entry, with the long name before it when one
/// is there and its checksum matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `STEM.EXT`, or `STEM` without an extension, or `.` and `..`: the
    /// short name as stored, trimmed of its padding. A byte that is not
    /// ASCII is shown as U+FFFD; [`Volume::tree`] refuses such a name.
    pub name: String,
    /// The long name of the long-name entries right before this one, when
    /// they are complete, in sequence and carry this entry's checksum.
    pub long_name: Option<String>,
    /// Attribute byte (offset 11).
    pub attributes: u8,
    /// First cluster (offset 26); 0 for an empty file or the root.
    pub first_cluster: u16,
    /// File length (offset 28).
    pub size: u32,
    /// The raw entry.
    pub raw: [u8; 32],
    /// The short name in the case byte 12 asks for (08h: the stem in lower
    /// case, 10h: the extension), as Windows NT, Linux and mtools write a
    /// name that is all lower case in one part instead of a long name.
    cased: String,
}

impl Entry {
    /// The directory attribute bit (10h) is set.
    pub fn is_dir(&self) -> bool {
        self.attributes & attribute::DIRECTORY != 0
    }

    /// The last-write time (offsets 22 and 24), the time DOS shows; `None`
    /// when the entry holds no valid time.
    pub fn time(&self) -> Option<DosTime> {
        let word = |at: usize| u16::from_le_bytes([self.raw[at], self.raw[at + 1]]);
        DosTime::from_words(word(24), word(22))
    }

    /// The name a host gives the file: the long name, else the short name
    /// with the case byte 12 asks for.
    pub fn host_name(&self) -> &str {
        self.long_name.as_deref().unwrap_or(&self.cased)
    }

    fn decode(raw: &[u8; 32], long_name: Option<String>) -> Self {
        let mut stored = [0; 11];
        stored.copy_from_slice(&raw[..11]);
        // A first byte of 05h stands for E5h, which marks a deleted entry.
        if stored[0] == 0x05 {
            stored[0] = 0xE5;
        }
        let stem = String::from_utf8_lossy(stored[..8].trim_ascii_end()).into_owned();
        let extension = String::from_utf8_lossy(stored[8..11].trim_ascii_end()).into_owned();
        let join = |stem: &str, extension: &str| {
            if extension.is_empty() {
                stem.to_owned()
            } else {
                format!("{stem}.{extension}")
            }
        };
        let lower = |part: &str, bit: u8| {
            if raw[12] & bit != 0 {
                part.to_ascii_lowercase()
            } else {
                part.to_owned()
            }
        };
        Self {
            name: join(&stem, &extension),
            cased: join(&lower(&stem, 0x08), &lower(&extension, 0x10)),
            long_name,
            attributes: raw[11],
            first_cluster: u16::from_le_bytes([raw[26], raw[27]]),
            size: u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]),
            raw: *raw,
        }
    }

    fn is_label(&self) -> bool {
        self.attributes & attribute::VOLUME_ID != 0
    }

    fn is_dot(&self) -> bool {
        self.raw[..11] == *b".          " || self.raw[..11] == *b"..         "
    }
}

/// One file or folder of a volume's tree ([`Volume::tree`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// The path from the root, the parts separated by `/`: each part the
    /// entry's long name, else its short name as stored (`LEVEL1/GAME.COM`).
    pub path: String,
    /// The short name as stored, `STEM.EXT`.
    pub short_name: String,
    /// Attribute byte; the directory bit tells a folder.
    pub attributes: u8,
    /// The last-write time, `None` when the entry holds none.
    pub time: Option<DosTime>,
    /// A file's bytes, exactly its stated length; `None` for a folder.
    pub contents: Option<Vec<u8>>,
}

/// A FAT12 volume in memory.
pub struct Volume<'a> {
    image: &'a [u8],
    cluster_bytes: usize,
    fat_start: usize,
    fat_bytes: usize,
    fats: usize,
    root_start: usize,
    root_entries: usize,
    data_start: usize,
    clusters: usize,
}

impl<'a> Volume<'a> {
    /// Decode the BPB. Refuses an image without the boot sector signature,
    /// a BPB whose values are impossible or do not describe exactly this
    /// image (a truncated image among them), and a volume too large for
    /// FAT12.
    pub fn open(image: &'a [u8]) -> Result<Self, ReadError> {
        if image.len() < 512 {
            return Err(ReadError::new(format!(
                "the image is {} bytes, shorter than a boot sector",
                image.len()
            )));
        }
        if image[510..512] != [0x55, 0xAA] {
            return Err(ReadError::new("no boot sector signature"));
        }
        let word = |at: usize| u64::from(u16::from_le_bytes([image[at], image[at + 1]]));
        let sector = word(11);
        if !matches!(sector, 512 | 1024 | 2048 | 4096) {
            return Err(ReadError::new(format!(
                "the BPB's sector size {sector} is not 512, 1024, 2048 or 4096"
            )));
        }
        let cluster_sectors = u64::from(image[13]);
        if !cluster_sectors.is_power_of_two() {
            return Err(ReadError::new(format!(
                "the BPB's {cluster_sectors} sectors per cluster is not a power of two"
            )));
        }
        let reserved = word(14);
        let fats = u64::from(image[16]);
        let root_entries = word(17);
        let fat_sectors = word(22);
        if reserved == 0 || fats == 0 || root_entries == 0 || fat_sectors == 0 {
            return Err(ReadError::new(
                "the BPB has no reserved sector, FAT or root directory",
            ));
        }
        let sectors = match word(19) {
            0 => u64::from(u32::from_le_bytes([
                image[32], image[33], image[34], image[35],
            ])),
            small => small,
        };
        let total = sectors * sector;
        let length = image.len() as u64;
        if total > length {
            return Err(ReadError::new(format!(
                "the image is truncated: its BPB describes {total} bytes, the image has {length}"
            )));
        }
        if total < length {
            return Err(ReadError::new(format!(
                "the BPB describes {total} bytes, the image has {length}"
            )));
        }
        let cluster_bytes = cluster_sectors * sector;
        let fat_start = reserved * sector;
        let fat_bytes = fat_sectors * sector;
        let root_start = fat_start + fats * fat_bytes;
        let data_start = root_start + (root_entries * 32).div_ceil(sector) * sector;
        if data_start >= total {
            return Err(ReadError::new("the BPB leaves no data area"));
        }
        let clusters = (total - data_start) / cluster_bytes;
        if clusters == 0 {
            return Err(ReadError::new("the data area holds no cluster"));
        }
        if clusters > MAX_CLUSTERS {
            return Err(ReadError::new(format!(
                "{clusters} clusters: a FAT16 or FAT32 volume, not FAT12"
            )));
        }
        if (clusters + 2) * 3 / 2 + 1 > fat_bytes {
            return Err(ReadError::new(
                "the FAT is too small for the data area's clusters",
            ));
        }
        // Every value is at most the image's length now.
        let size = |value: u64| value as usize;
        Ok(Self {
            image,
            cluster_bytes: size(cluster_bytes),
            fat_start: size(fat_start),
            fat_bytes: size(fat_bytes),
            fats: size(fats),
            root_start: size(root_start),
            root_entries: size(root_entries),
            data_start: size(data_start),
            clusters: size(clusters),
        })
    }

    /// Bytes per cluster.
    pub fn cluster_bytes(&self) -> usize {
        self.cluster_bytes
    }

    /// The number of clusters in the data area; they are numbered from 2.
    pub fn clusters(&self) -> usize {
        self.clusters
    }

    /// The 12-bit entry of `cluster` in the first FAT; FF7h (a bad cluster)
    /// for a number past the data area.
    pub fn fat_entry(&self, cluster: u16) -> u16 {
        if usize::from(cluster) >= self.clusters + 2 {
            return 0xFF7;
        }
        // `open` checked that the FAT holds every entry of the data area.
        let at = self.fat_start + usize::from(cluster) + usize::from(cluster) / 2;
        let pair = u16::from_le_bytes([self.image[at], self.image[at + 1]]);
        if cluster.is_multiple_of(2) {
            pair & 0xFFF
        } else {
            pair >> 4
        }
    }

    /// The root's volume-label entry, trimmed.
    pub fn label(&self) -> Option<String> {
        self.root_raw()
            .into_iter()
            .find(Entry::is_label)
            .map(|entry| String::from_utf8_lossy(entry.raw[..11].trim_ascii_end()).into_owned())
    }

    /// The volume serial of the extended BPB (offset 39), when the boot
    /// sector has one (signature 29h at offset 38).
    pub fn serial(&self) -> Option<u32> {
        (self.image[38] == 0x29).then(|| {
            u32::from_le_bytes([
                self.image[39],
                self.image[40],
                self.image[41],
                self.image[42],
            ])
        })
    }

    /// Data-area bytes in free clusters, as DOS DIR reports them.
    pub fn free_bytes(&self) -> usize {
        (2..self.clusters + 2)
            .filter(|&cluster| self.fat_entry(cluster as u16) == 0)
            .count()
            * self.cluster_bytes
    }

    fn root_raw(&self) -> Vec<Entry> {
        let end = self.root_start + self.root_entries * 32;
        entries(&self.image[self.root_start..end])
    }

    /// The clusters of a chain, refusing a cluster outside the data area, a
    /// free, reserved or bad cluster, and a chain that loops.
    pub fn chain(&self, first: u16) -> Result<Vec<u16>, ReadError> {
        let mut seen = vec![false; self.clusters + 2];
        let mut chain = Vec::new();
        let mut cluster = first;
        loop {
            if !(2..self.clusters + 2).contains(&usize::from(cluster)) {
                return Err(ReadError::new(format!(
                    "the chain from cluster {first} reaches cluster {cluster}, outside the data area"
                )));
            }
            if std::mem::replace(&mut seen[usize::from(cluster)], true) {
                return Err(ReadError::new(format!(
                    "the chain from cluster {first} loops at cluster {cluster}"
                )));
            }
            chain.push(cluster);
            match self.fat_entry(cluster) {
                0xFF8..=0xFFF => return Ok(chain),
                0 => {
                    return Err(ReadError::new(format!(
                        "the chain from cluster {first} reaches the free cluster {cluster}"
                    )))
                }
                0xFF7 => {
                    return Err(ReadError::new(format!(
                        "the chain from cluster {first} reaches the bad cluster {cluster}"
                    )))
                }
                // FF0h to FF6h are reserved values, and cluster numbers of a
                // volume of 4079 clusters or more: the range check above
                // tells which.
                1 => {
                    return Err(ReadError::new(format!(
                        "the chain from cluster {first} reaches the reserved value 001h"
                    )))
                }
                next => cluster = next,
            }
        }
    }

    fn chain_bytes(&self, chain: &[u16]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(chain.len() * self.cluster_bytes);
        for &cluster in chain {
            // `chain` returns clusters of the data area only, which `open`
            // checked lies within the image.
            let at = self.data_start + (usize::from(cluster) - 2) * self.cluster_bytes;
            bytes.extend_from_slice(&self.image[at..at + self.cluster_bytes]);
        }
        bytes
    }

    /// A file's bytes and chain from its entry: a chain exactly as long as
    /// its size needs, the bytes cut to the size.
    fn file_bytes(&self, entry: &Entry, path: &str) -> Result<(Vec<u8>, Vec<u16>), ReadError> {
        let size = entry.size as usize;
        if size == 0 {
            return if entry.first_cluster == 0 {
                Ok((Vec::new(), Vec::new()))
            } else {
                Err(ReadError::new(format!(
                    "the empty file {path} owns a cluster"
                )))
            };
        }
        let chain = self
            .chain(entry.first_cluster)
            .map_err(|e| within(path, e))?;
        if chain.len() != size.div_ceil(self.cluster_bytes) {
            return Err(ReadError::new(format!(
                "{path}: a chain of {} clusters for {size} bytes",
                chain.len()
            )));
        }
        let mut bytes = self.chain_bytes(&chain);
        bytes.truncate(size);
        Ok((bytes, chain))
    }

    /// The entries and chain of a subdirectory.
    fn folder(&self, entry: &Entry, path: &str) -> Result<(Vec<Entry>, Vec<u16>), ReadError> {
        if entry.first_cluster == 0 {
            return Err(ReadError::new(format!("the folder {path} owns no cluster")));
        }
        let chain = self
            .chain(entry.first_cluster)
            .map_err(|e| within(path, e))?;
        Ok((entries(&self.chain_bytes(&chain)), chain))
    }

    /// Entries of a folder, `.` and `..` included for a subdirectory and the
    /// volume label left out. `path` is `""` for the root or
    /// backslash-separated folder names such as `GAME\DATA`; case is ignored.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, ReadError> {
        let mut entries: Vec<Entry> = self.root_raw();
        for part in path.split('\\').filter(|part| !part.is_empty()) {
            let folder = entries
                .iter()
                .find(|entry| {
                    entry.is_dir() && !entry.is_label() && entry.name.eq_ignore_ascii_case(part)
                })
                .ok_or_else(|| ReadError::new(format!("no folder {part} in {path}")))?;
            entries = self.folder(folder, path)?.0;
        }
        entries.retain(|entry| !entry.is_label());
        Ok(entries)
    }

    /// The contents of a file at a backslash-separated path.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, ReadError> {
        let (folder, name) = path.rsplit_once('\\').unwrap_or(("", path));
        let entry = self
            .list(folder)?
            .into_iter()
            .find(|entry| !entry.is_dir() && entry.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| ReadError::new(format!("no file {path}")))?;
        Ok(self.file_bytes(&entry, path)?.0)
    }

    /// Every file and folder below the root as (path, entry), the path of
    /// short names separated by backslashes, sorted by path.
    pub fn walk(&self) -> Result<Vec<(String, Entry)>, ReadError> {
        let mut found = Vec::new();
        self.visit(&mut |_, short, entry, _| {
            found.push((short.join("\\"), entry.clone()));
            Ok(())
        })?;
        found.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(found)
    }

    /// The volume's files and folders in the order they are stored: each
    /// folder's entries in directory order, a folder's own members right
    /// after it. A file's bytes are exactly its stated length.
    ///
    /// Refused: a short name that is not printable ASCII (a long name must
    /// be valid UTF-16 to be read at all), a name that holds `/` or is `.`
    /// or `..` or is longer than [`MAX_NAME`] bytes, a folder deeper than
    /// [`MAX_FOLDER_PATH`], a path longer than [`MAX_PATH`], a chain that is
    /// broken or loops, a file whose chain does not match its size, and a
    /// cluster that two files or folders claim. So the tree never holds
    /// more file bytes than the volume's data area, and its paths stay
    /// short.
    pub fn tree(&self) -> Result<Vec<Node>, ReadError> {
        let mut nodes = Vec::new();
        let mut owner = vec![false; self.clusters + 2];
        self.visit(&mut |path, _, entry, folder_chain| {
            let path = path.join("/");
            if entry.long_name.is_none() && !entry.name.bytes().all(|b| (0x20..0x7F).contains(&b)) {
                return Err(ReadError::new(format!(
                    "{path}: the short name is not ASCII"
                )));
            }
            let (contents, chain) = if entry.is_dir() {
                (None, folder_chain.to_vec())
            } else {
                let (bytes, chain) = self.file_bytes(entry, &path)?;
                (Some(bytes), chain)
            };
            for cluster in chain {
                if std::mem::replace(&mut owner[usize::from(cluster)], true) {
                    return Err(ReadError::new(format!(
                        "{path}: cluster {cluster} belongs to another file or folder too"
                    )));
                }
            }
            nodes.push(Node {
                path,
                short_name: entry.name.clone(),
                attributes: entry.attributes,
                time: entry.time(),
                contents,
            });
            Ok(())
        })?;
        Ok(nodes)
    }

    /// Visit every file and folder below the root in stored order, a
    /// folder's members right after it, with its path of host names, its
    /// path of short names and, for a folder, its chain. Each folder's
    /// clusters are read once: a folder met again, through a loop or a
    /// cross-linked entry, is refused, so the walk always ends.
    fn visit(&self, visitor: &mut Visitor<'_>) -> Result<(), ReadError> {
        let mut folders = vec![false; self.clusters + 2];
        let members = |entries: Vec<Entry>| {
            entries
                .into_iter()
                .filter(|entry| !entry.is_label() && !entry.is_dot())
                .collect::<Vec<_>>()
                .into_iter()
        };
        // One path of host names and one of short names, the folders the
        // walk is in, with their lengths as `\A\B` paths; each level of the
        // stack is the rest of one of those folders.
        let mut path: Vec<String> = Vec::new();
        let mut short: Vec<String> = Vec::new();
        let mut lengths: Vec<(usize, usize)> = Vec::new();
        let mut stack = vec![members(self.root_raw())];
        while let Some(rest) = stack.last_mut() {
            let Some(entry) = rest.next() else {
                stack.pop();
                path.pop();
                short.pop();
                lengths.pop();
                continue;
            };
            let name = entry.host_name();
            if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                return Err(ReadError::new(format!(
                    "{}: the name {name:?} cannot be a path part",
                    path.join("/")
                )));
            }
            if name.len() > MAX_NAME {
                return Err(ReadError::new(format!(
                    "{}: a name of {} bytes, longer than {MAX_NAME}",
                    path.join("/"),
                    name.len()
                )));
            }
            let length = lengths.last().map_or(0, |&(host, _)| host) + 1 + name.chars().count();
            let short_length = lengths.last().map_or(0, |&(_, short)| short) + 1 + entry.name.len();
            if length > MAX_PATH {
                return Err(ReadError::new(format!(
                    "{}/{name}: a path longer than {MAX_PATH} characters",
                    path.join("/")
                )));
            }
            path.push(name.to_owned());
            short.push(entry.name.clone());
            if !entry.is_dir() {
                let visited = visitor(&path, &short, &entry, &[]);
                path.pop();
                short.pop();
                visited?;
                continue;
            }
            if short_length > MAX_FOLDER_PATH {
                return Err(ReadError::new(format!(
                    "{}: a folder deeper than the {MAX_FOLDER_PATH} characters DOS keeps for a path",
                    path.join("/")
                )));
            }
            let (inner, chain) = self.folder(&entry, &path.join("/"))?;
            for &cluster in &chain {
                if std::mem::replace(&mut folders[usize::from(cluster)], true) {
                    return Err(ReadError::new(format!(
                        "{}: the folder's cluster {cluster} is read twice",
                        path.join("/")
                    )));
                }
            }
            visitor(&path, &short, &entry, &chain)?;
            lengths.push((length, short_length));
            stack.push(members(inner));
        }
        Ok(())
    }

    /// Structural check: identical FAT copies, reserved entries 0 and 1, every
    /// subdirectory starting with `.` (itself) and `..` (its parent, 0 for the
    /// root), chain lengths matching file sizes, no cluster in two chains and
    /// no allocated cluster outside every chain.
    pub fn check(&self) -> Result<(), ReadError> {
        let fat = |copy: usize| {
            let at = self.fat_start + copy * self.fat_bytes;
            &self.image[at..at + self.fat_bytes]
        };
        if (1..self.fats).any(|copy| fat(copy) != fat(0)) {
            return Err(ReadError::new("FAT copies differ"));
        }
        if self.fat_entry(0) != 0xF00 | u16::from(self.image[21]) || self.fat_entry(1) != 0xFFF {
            return Err(ReadError::new("reserved FAT entries are wrong"));
        }
        let mut owner = vec![false; self.clusters + 2];
        let mut claim = |chain: &[u16]| -> Result<(), ReadError> {
            for &cluster in chain {
                if std::mem::replace(&mut owner[usize::from(cluster)], true) {
                    return Err(ReadError::new(format!(
                        "cluster {cluster} is in two chains"
                    )));
                }
            }
            Ok(())
        };
        // Each folder with its own first cluster and its parent's; a folder
        // claims its clusters before it is read, so none is read twice.
        let mut pending = vec![(String::new(), self.root_raw(), 0u16, 0u16)];
        while let Some((folder, entries, cluster, parent)) = pending.pop() {
            let members = if folder.is_empty() {
                &entries[..]
            } else {
                let dots = entries
                    .get(..2)
                    .ok_or_else(|| ReadError::new(format!("{folder}: no dot entries")))?;
                if dots[0].raw[..11] != *b".          "
                    || dots[0].first_cluster != cluster
                    || dots[1].raw[..11] != *b"..         "
                    || dots[1].first_cluster != parent
                {
                    return Err(ReadError::new(format!("{folder}: wrong . or .. entry")));
                }
                &entries[2..]
            };
            for entry in members.iter().filter(|entry| !entry.is_label()) {
                let path = if folder.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{folder}\\{}", entry.name)
                };
                if entry.is_dir() {
                    if 1 + path.len() > MAX_FOLDER_PATH {
                        return Err(ReadError::new(format!(
                            "{path}: a folder deeper than the {MAX_FOLDER_PATH} characters DOS keeps for a path"
                        )));
                    }
                    let (inner, chain) = self.folder(entry, &path)?;
                    claim(&chain)?;
                    pending.push((path, inner, entry.first_cluster, cluster));
                } else {
                    claim(&self.file_bytes(entry, &path)?.1)?;
                }
            }
        }
        for (cluster, owned) in owner.iter().enumerate().skip(2) {
            if self.fat_entry(cluster as u16) != 0 && !owned {
                return Err(ReadError::new(format!(
                    "cluster {cluster} is allocated but unreachable"
                )));
            }
        }
        Ok(())
    }
}

/// What [`Volume::visit`] calls for each file and folder: its path of host
/// names, its path of short names, its entry and, for a folder, its chain.
type Visitor<'v> = dyn FnMut(&[String], &[String], &Entry, &[u16]) -> Result<(), ReadError> + 'v;

fn within(path: &str, error: ReadError) -> ReadError {
    ReadError::new(format!("{path}: {error}"))
}

/// Directory entries up to the end marker, deleted ones left out, each with
/// the long name of the long-name entries right before it when they form
/// one (Long File Name Implementation: sequence numbers from the last part
/// down to 1, the short name's checksum in each, 13 UTF-16 units a part).
fn entries(bytes: &[u8]) -> Vec<Entry> {
    let mut found = Vec::new();
    // Long-name parts seen since the last short entry, as stored.
    let mut parts: Vec<&[u8; 32]> = Vec::new();
    for raw in bytes.as_chunks::<32>().0 {
        match raw[0] {
            0x00 => break,
            0xE5 => parts.clear(),
            _ if raw[11] & 0x3F == attribute::LONG_NAME => parts.push(raw),
            _ => {
                let long_name = long_name(&parts, raw);
                parts.clear();
                found.push(Entry::decode(raw, long_name));
            }
        }
    }
    found
}

/// The long name `parts` spell for the short entry `short`, or `None` when
/// they are absent, out of sequence or carry another checksum.
fn long_name(parts: &[&[u8; 32]], short: &[u8; 32]) -> Option<String> {
    let first = parts.first()?;
    let count = usize::from(first[0] & 0x1F);
    if first[0] & 0x40 == 0 || count != parts.len() || count == 0 {
        return None;
    }
    let checksum = short[..11]
        .iter()
        .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte));
    let mut units = Vec::new();
    for (index, part) in parts.iter().rev().enumerate() {
        if usize::from(part[0] & 0x1F) != index + 1 || part[13] != checksum {
            return None;
        }
        for range in [1..11, 14..26, 28..32] {
            units.extend(
                part[range]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair)),
            );
        }
    }
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    let name = char::decode_utf16(units[..end].iter().copied())
        .collect::<Result<String, _>>()
        .ok()?;
    (!name.is_empty() && !name.chars().any(char::is_control)).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fat12::{plan_volumes, Entry as Put, FloppyFormat, Order, Volume as Keys};

    fn time(text: &str) -> DosTime {
        text.parse().unwrap()
    }

    type Read = (String, Option<DosTime>, Option<Vec<u8>>);

    /// The tree as (path, time, contents).
    fn read_back(image: &[u8]) -> Vec<Read> {
        Volume::open(image)
            .unwrap()
            .tree()
            .unwrap()
            .into_iter()
            .map(|node| (node.path, node.time, node.contents))
            .collect()
    }

    fn build(entries: Vec<Put>, keys: &Keys) -> Vec<Vec<u8>> {
        plan_volumes(entries, keys).unwrap().images().unwrap()
    }

    /// Every builder feature reads back: the listed order, each entry's
    /// time and the default time of the others, the label, the serial, and
    /// files, folders and an empty folder.
    #[test]
    fn the_tree_of_a_built_volume_is_what_was_put_on_it() {
        let mut keys = Keys::new(FloppyFormat::Dd720);
        keys.order = Order::Listed;
        keys.label = Some(*b"ROUND TRIP ");
        keys.serial = 0x1234_5678;
        keys.time = time("2001-02-03T04:05:06");
        let images = build(
            vec![
                Put::file("ZED.TXT", b"zed".to_vec()).at(time("1993-03-27T13:58:22")),
                Put::folder("EMPTY"),
                Put::file("DATA/B.DAT", vec![7; 3000]),
                Put::file("DATA/A.DAT", Vec::new()).at(time("2107-12-31T23:59:58")),
                Put::file("ALPHA.COM", vec![1; 1024]),
            ],
            &keys,
        );
        assert_eq!(images.len(), 1);
        let volume = Volume::open(&images[0]).unwrap();
        volume.check().unwrap();
        assert_eq!(volume.label().as_deref(), Some("ROUND TRIP"));
        assert_eq!(volume.serial(), Some(0x1234_5678));
        let default = Some(time("2001-02-03T04:05:06"));
        let expected: Vec<Read> = vec![
            (
                "ZED.TXT".into(),
                Some(time("1993-03-27T13:58:22")),
                Some(b"zed".to_vec()),
            ),
            ("EMPTY".into(), default, None),
            ("DATA".into(), default, None),
            ("DATA/B.DAT".into(), default, Some(vec![7; 3000])),
            (
                "DATA/A.DAT".into(),
                Some(time("2107-12-31T23:59:58")),
                Some(Vec::new()),
            ),
            ("ALPHA.COM".into(), default, Some(vec![1; 1024])),
        ];
        assert_eq!(read_back(&images[0]), expected);
        let nodes = volume.tree().unwrap();
        assert_eq!(nodes[0].attributes & attribute::DIRECTORY, 0);
        assert_eq!(
            nodes[1].attributes & attribute::DIRECTORY,
            attribute::DIRECTORY
        );
        assert_eq!(nodes[3].short_name, "B.DAT");
    }

    /// Name order reads back in the builder's order: in every folder its
    /// files, then its folders, each by the stored name.
    #[test]
    fn name_order_reads_back_files_then_folders() {
        let images = build(
            vec![
                Put::folder("B"),
                Put::file("Z.TXT", b"z".to_vec()),
                Put::file("A/X.TXT", b"x".to_vec()),
                Put::file("M.TXT", b"m".to_vec()),
            ],
            &Keys::new(FloppyFormat::Hd144),
        );
        let paths: Vec<String> = read_back(&images[0]).into_iter().map(|n| n.0).collect();
        assert_eq!(paths, ["M.TXT", "Z.TXT", "A", "A/X.TXT", "B"]);
    }

    /// The volumes of a split read back as the tree that was split: every
    /// file once, in order, its folder on each volume that holds part of it.
    #[test]
    fn the_volumes_of_a_split_together_hold_the_tree() {
        let mut keys = Keys::new(FloppyFormat::Dd360);
        keys.volumes = Some(3);
        keys.label = Some(*b"PARTS      ");
        let entries: Vec<Put> = (0..6u8)
            .map(|index| Put::file(format!("GAME/PART{index}.BIN"), vec![index; 150_000]))
            .collect();
        let images = build(entries.clone(), &keys);
        assert_eq!(images.len(), 3);
        let mut files = Vec::new();
        for (index, image) in images.iter().enumerate() {
            let volume = Volume::open(image).unwrap();
            volume.check().unwrap();
            assert_eq!(volume.label(), Some(format!("PARTS {}", index + 1)));
            for (path, _, contents) in read_back(image) {
                match contents {
                    Some(contents) => files.push((path, contents)),
                    None => assert_eq!(path, "GAME"),
                }
            }
        }
        let expected: Vec<(String, Vec<u8>)> = entries
            .into_iter()
            .map(|entry| (entry.path, entry.contents.unwrap()))
            .collect();
        assert_eq!(files, expected);
    }

    /// A long name before its short entry names the file when its checksum
    /// matches, and is ignored when it does not.
    #[test]
    fn a_long_name_with_the_right_checksum_names_the_file() {
        let mut image = build(
            vec![Put::file("FDAUTO.BAT", b"@echo off".to_vec())],
            &Keys::new(FloppyFormat::Hd144),
        )
        .remove(0);
        let root = Volume::open(&image).unwrap().root_start;
        // Move the short entry one slot down and put a long name before it.
        let short: [u8; 32] = image[root..root + 32].try_into().unwrap();
        image.copy_within(root..root + 32, root + 32);
        let checksum = short[..11]
            .iter()
            .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte));
        let mut part = [0xFFu8; 32];
        part[0] = 0x41;
        part[11] = attribute::LONG_NAME;
        part[12] = 0;
        part[13] = checksum;
        part[26..28].copy_from_slice(&[0, 0]);
        let name: Vec<u16> = "fdauto.bat".encode_utf16().chain([0]).collect();
        let slots = (1..11)
            .step_by(2)
            .chain((14..26).step_by(2))
            .chain((28..32).step_by(2));
        for (slot, unit) in slots.zip(name) {
            part[slot..slot + 2].copy_from_slice(&unit.to_le_bytes());
        }
        image[root..root + 32].copy_from_slice(&part);
        let nodes = Volume::open(&image).unwrap().tree().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].path, "fdauto.bat");
        assert_eq!(nodes[0].short_name, "FDAUTO.BAT");
        assert_eq!(nodes[0].contents.as_deref(), Some(&b"@echo off"[..]));
        image[root + 13] ^= 1;
        let nodes = Volume::open(&image).unwrap().tree().unwrap();
        assert_eq!(nodes[0].path, "FDAUTO.BAT");
    }

    /// A 1.44M volume with A.BIN (2000 bytes, four clusters) in the root and
    /// DIR\B.BIN (600 bytes) below it.
    fn two_files() -> Vec<u8> {
        build(
            vec![
                Put::file("A.BIN", vec![9; 2000]),
                Put::file("DIR/B.BIN", vec![8; 600]),
            ],
            &Keys::new(FloppyFormat::Hd144),
        )
        .remove(0)
    }

    /// Write the 12-bit entry of `cluster` in both FATs of a 1.44M volume.
    fn set_fat(image: &mut [u8], cluster: u16, value: u16) {
        for copy in 0..2 {
            let at = 512 + copy * 9 * 512 + usize::from(cluster) + usize::from(cluster) / 2;
            let pair = u16::from_le_bytes([image[at], image[at + 1]]);
            let pair = if cluster.is_multiple_of(2) {
                (pair & 0xF000) | value
            } else {
                (pair & 0x000F) | (value << 4)
            };
            image[at..at + 2].copy_from_slice(&pair.to_le_bytes());
        }
    }

    fn refused(image: &[u8]) -> String {
        match Volume::open(image) {
            Err(error) => error.to_string(),
            Ok(volume) => volume.tree().unwrap_err().to_string(),
        }
    }

    /// A hostile or damaged image is an error with its reason, never a
    /// panic, a hang or more bytes than it holds.
    #[test]
    fn a_damaged_volume_is_refused_with_a_reason() {
        let image = two_files();
        let volume = Volume::open(&image).unwrap();
        let root = volume.list("").unwrap();
        let (first, dir) = (root[0].first_cluster, root[1].first_cluster);
        assert_eq!(volume.chain(first).unwrap().len(), 4);
        let dir_at = volume.data_start + (usize::from(dir) - 2) * 512;

        let mut looping = image.clone();
        set_fat(&mut looping, first + 3, first);
        assert!(refused(&looping).contains("loops at cluster"));

        let mut outside = image.clone();
        set_fat(&mut outside, first + 1, 4000);
        assert!(refused(&outside).contains("outside the data area"));

        let mut free = image.clone();
        set_fat(&mut free, first + 1, 0);
        assert!(refused(&free).contains("free cluster"));

        let mut bad = image.clone();
        set_fat(&mut bad, first + 1, 0xFF7);
        assert!(refused(&bad).contains("bad cluster"));

        let mut short = image.clone();
        set_fat(&mut short, first + 1, 0xFFF);
        assert!(refused(&short).contains("a chain of 2 clusters for 2000 bytes"));

        // B.BIN's entry, the third of DIR, pointed at A.BIN's chain.
        let mut cross = image.clone();
        let at = dir_at + 64;
        cross[at + 26..at + 28].copy_from_slice(&first.to_le_bytes());
        cross[at + 28..at + 32].copy_from_slice(&2000u32.to_le_bytes());
        assert!(refused(&cross).contains("belongs to another file or folder"));

        // B.BIN's entry turned into a folder that is DIR itself.
        let mut folder_loop = image.clone();
        folder_loop[at..at + 11].copy_from_slice(b"SELF       ");
        folder_loop[at + 11] = attribute::DIRECTORY;
        folder_loop[at + 26..at + 28].copy_from_slice(&dir.to_le_bytes());
        assert!(refused(&folder_loop).contains("is read twice"));
        assert!(Volume::open(&folder_loop).unwrap().walk().is_err());
        assert!(Volume::open(&folder_loop).unwrap().check().is_err());

        let truncated = &image[..image.len() - 512];
        assert!(refused(truncated).contains("truncated"));
        assert!(refused(&image[..100]).contains("shorter than a boot sector"));

        let mut unsigned = image.clone();
        unsigned[510] = 0;
        assert!(refused(&unsigned).contains("signature"));

        let mut sector = image.clone();
        sector[11..13].copy_from_slice(&0u16.to_le_bytes());
        assert!(refused(&sector).contains("sector size"));

        let mut name = image.clone();
        name[volume.root_start] = 0xC4;
        assert!(refused(&name).contains("not ASCII"));
    }

    /// Every byte of the boot sector, the first FAT sector, the root's
    /// first sector and DIR's cluster set to 00h, FFh and its value with
    /// the top bit flipped, in turn: the reader answers without a panic.
    #[test]
    fn no_single_byte_change_makes_the_reader_panic() {
        let image = two_files();
        let volume = Volume::open(&image).unwrap();
        let dir = volume.list("").unwrap()[1].first_cluster;
        let dir_at = volume.data_start + (usize::from(dir) - 2) * 512;
        let mut changed = image.clone();
        for at in (0..1024).chain(9728..10240).chain(dir_at..dir_at + 512) {
            for value in [0x00, 0xFF, image[at] ^ 0x80] {
                changed[at] = value;
                if let Ok(volume) = Volume::open(&changed) {
                    let _ = volume.tree();
                    let _ = volume.walk();
                    let _ = volume.check();
                    let _ = volume.label();
                    let _ = volume.free_bytes();
                    let _ = volume.read("DIR\\B.BIN");
                }
            }
            changed[at] = image[at];
        }
        for length in [0, 1, 511, 512, 513, 4096, image.len() - 1] {
            assert!(Volume::open(&image[..length]).is_err(), "{length}");
        }
    }

    /// A volume of the largest FAT12 size, 4084 clusters of one sector, one
    /// FAT of 12 sectors and a root of 16 entries, with nothing on it.
    fn largest() -> Vec<u8> {
        let total = 1 + 12 + 1 + 4084;
        let mut image = vec![0u8; total * 512];
        image[11..13].copy_from_slice(&512u16.to_le_bytes());
        image[13] = 1;
        image[14..16].copy_from_slice(&1u16.to_le_bytes());
        image[16] = 1;
        image[17..19].copy_from_slice(&16u16.to_le_bytes());
        image[19..21].copy_from_slice(&(total as u16).to_le_bytes());
        image[21] = 0xF0;
        image[22..24].copy_from_slice(&12u16.to_le_bytes());
        image[510..512].copy_from_slice(&[0x55, 0xAA]);
        set_one_fat(&mut image, 0, 0xFF0);
        set_one_fat(&mut image, 1, 0xFFF);
        image
    }

    fn set_one_fat(image: &mut [u8], cluster: usize, value: u16) {
        let at = 512 + cluster + cluster / 2;
        let pair = u16::from_le_bytes([image[at], image[at + 1]]);
        let pair = if cluster.is_multiple_of(2) {
            (pair & 0xF000) | value
        } else {
            (pair & 0x000F) | (value << 4)
        };
        image[at..at + 2].copy_from_slice(&pair.to_le_bytes());
    }

    fn short_entry(name: &[u8; 11], attributes: u8, cluster: u16, size: u32) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[..11].copy_from_slice(name);
        entry[11] = attributes;
        entry[24..26].copy_from_slice(&((1u16 << 5) | 1).to_le_bytes());
        entry[26..28].copy_from_slice(&cluster.to_le_bytes());
        entry[28..32].copy_from_slice(&size.to_le_bytes());
        entry
    }

    /// The long-name entries of `parts` parts of `unit` for `short`.
    fn long_parts(short: &[u8; 11], parts: usize, unit: u16) -> Vec<[u8; 32]> {
        let checksum = short
            .iter()
            .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte));
        (1..=parts)
            .rev()
            .map(|sequence| {
                let mut part = [0u8; 32];
                part[0] = sequence as u8 | if sequence == parts { 0x40 } else { 0 };
                part[11] = attribute::LONG_NAME;
                part[13] = checksum;
                let slots = (1..11)
                    .step_by(2)
                    .chain((14..26).step_by(2))
                    .chain((28..32).step_by(2));
                for slot in slots {
                    part[slot..slot + 2].copy_from_slice(&unit.to_le_bytes());
                }
                part
            })
            .collect()
    }

    /// `depth` folders each inside the one before, every one named D, with
    /// a long name of `parts` parts of `unit` when `parts` is not 0.
    fn nested(depth: usize, parts: usize, unit: u16) -> Vec<u8> {
        let mut image = largest();
        let (root, data) = (13 * 512, 14 * 512);
        let name = b"D          ";
        let put = |image: &mut Vec<u8>, mut at: usize, child: u16| {
            if parts > 0 {
                for part in long_parts(name, parts, unit) {
                    image[at..at + 32].copy_from_slice(&part);
                    at += 32;
                }
            }
            image[at..at + 32].copy_from_slice(&short_entry(name, attribute::DIRECTORY, child, 0));
        };
        put(&mut image, root, 2);
        for level in 0..depth {
            let cluster = 2 + level;
            set_one_fat(&mut image, cluster, 0xFFF);
            let at = data + level * 512;
            let parent = if level == 0 { 0 } else { cluster as u16 - 1 };
            image[at..at + 32].copy_from_slice(&short_entry(
                b".          ",
                attribute::DIRECTORY,
                cluster as u16,
                0,
            ));
            image[at + 32..at + 64].copy_from_slice(&short_entry(
                b"..         ",
                attribute::DIRECTORY,
                parent,
                0,
            ));
            if level + 1 < depth {
                put(&mut image, at + 64, cluster as u16 + 1);
            }
        }
        image
    }

    /// A crafted image of thousands of nested folders is refused at the
    /// depth DOS allows, at once, without building paths for every level;
    /// so is a path of long names longer than MAX_PATH and a long name
    /// longer than 255 bytes.
    #[test]
    fn deep_folders_and_long_paths_are_refused_quickly() {
        let started = std::time::Instant::now();
        for parts in [0, 13] {
            let image = nested(4000, parts, u16::from(b'N'));
            let volume = Volume::open(&image).unwrap();
            let error = volume.tree().unwrap_err().to_string();
            let expected = if parts == 0 {
                "deeper than the 64 characters"
            } else {
                "a path longer than 260 characters"
            };
            assert!(error.contains(expected), "{error}");
            assert!(volume.walk().is_err());
            assert!(volume.check().is_err());
        }
        // 32 levels of one-letter folders are as deep as DOS goes.
        assert_eq!(
            Volume::open(&nested(32, 0, 0))
                .unwrap()
                .tree()
                .unwrap()
                .len(),
            32
        );
        assert!(Volume::open(&nested(33, 0, 0)).unwrap().tree().is_err());
        // A long name of 130 two-byte characters is 260 bytes of UTF-8.
        let error = Volume::open(&nested(1, 10, 0x00E9))
            .unwrap()
            .tree()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("a name of 260 bytes, longer than 255"),
            "{error}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    /// The case bits of byte 12 name a file whose short name is all lower
    /// case in its stem or its extension, as mtools and Windows NT write it.
    #[test]
    fn the_case_bits_of_a_short_entry_give_its_host_name() {
        let mut image = build(
            vec![Put::file("README.TXT", b"read".to_vec())],
            &Keys::new(FloppyFormat::Hd144),
        )
        .remove(0);
        let root = Volume::open(&image).unwrap().root_start;
        for (bits, name) in [
            (0x00, "README.TXT"),
            (0x08, "readme.TXT"),
            (0x10, "README.txt"),
            (0x18, "readme.txt"),
        ] {
            image[root + 12] = bits;
            let nodes = Volume::open(&image).unwrap().tree().unwrap();
            assert_eq!(
                (nodes[0].path.as_str(), nodes[0].short_name.as_str()),
                (name, "README.TXT")
            );
        }
    }

    /// In a volume of more than 4078 clusters, FF0h to FF5h are cluster
    /// numbers, not reserved values.
    #[test]
    fn cluster_numbers_up_to_ff5h_are_followed() {
        let mut image = largest();
        set_one_fat(&mut image, 0xFEF, 0xFF0);
        set_one_fat(&mut image, 0xFF0, 0xFF5);
        set_one_fat(&mut image, 0xFF5, 0xFFF);
        let at = 13 * 512;
        image[at..at + 32].copy_from_slice(&short_entry(b"END     BIN", 0x20, 0xFEF, 1536));
        let volume = Volume::open(&image).unwrap();
        assert_eq!(volume.chain(0xFEF).unwrap(), [0xFEF, 0xFF0, 0xFF5]);
        assert_eq!(
            volume.tree().unwrap()[0].contents.as_ref().unwrap().len(),
            1536
        );
        // FF6h is past the data area of this volume.
        set_one_fat(&mut image, 0xFF5, 0xFF6);
        assert!(refused(&image).contains("outside the data area"));
    }

    #[test]
    fn a_volume_of_fat16_size_is_not_read_as_fat12() {
        let mut image = vec![0u8; 8 * 1024 * 1024];
        image[11..13].copy_from_slice(&512u16.to_le_bytes());
        image[13] = 1;
        image[14..16].copy_from_slice(&1u16.to_le_bytes());
        image[16] = 2;
        image[17..19].copy_from_slice(&512u16.to_le_bytes());
        image[19..21].copy_from_slice(&16384u16.to_le_bytes());
        image[22..24].copy_from_slice(&64u16.to_le_bytes());
        image[510..512].copy_from_slice(&[0x55, 0xAA]);
        assert!(refused(&image).contains("not FAT12"), "{}", refused(&image));
    }
}
