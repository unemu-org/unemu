// SPDX-License-Identifier: AGPL-3.0-or-later
//! A FAT12 volume reader for tests, independent of the writer: it decodes the
//! BPB, follows on-media cluster chains through the first FAT and walks
//! directories by path. Microsoft FAT Specification, Boot Sector and BPB, FAT
//! Data Structure (FAT12 entries) and FAT Directory Structure.

/// One 32-byte short directory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `STEM.EXT`, or `STEM` without an extension, or `.` and `..`.
    pub name: String,
    /// Attribute byte (offset 11).
    pub attributes: u8,
    /// First cluster (offset 26); 0 for an empty file or the root.
    pub first_cluster: u16,
    /// File length (offset 28).
    pub size: u32,
    /// The raw entry.
    pub raw: [u8; 32],
}

impl Entry {
    /// The directory attribute bit (10h) is set.
    pub fn is_dir(&self) -> bool {
        self.attributes & 0x10 != 0
    }

    fn decode(bytes: &[u8; 32]) -> Self {
        let bytes = *bytes;
        let stem = String::from_utf8_lossy(bytes[..8].trim_ascii_end()).into_owned();
        let extension = String::from_utf8_lossy(bytes[8..11].trim_ascii_end()).into_owned();
        Self {
            name: if extension.is_empty() {
                stem
            } else {
                format!("{stem}.{extension}")
            },
            attributes: bytes[11],
            first_cluster: u16::from_le_bytes([bytes[26], bytes[27]]),
            size: u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
            raw: bytes,
        }
    }
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
    /// Decode the BPB; refuses geometry that does not fit the image.
    pub fn open(image: &'a [u8]) -> Result<Self, String> {
        if image.len() < 512 || image[510..512] != [0x55, 0xAA] {
            return Err("no boot sector signature".into());
        }
        let word = |at: usize| usize::from(u16::from_le_bytes([image[at], image[at + 1]]));
        let sector = word(11);
        let cluster_bytes = usize::from(image[13]) * sector;
        let fat_start = word(14) * sector;
        let fats = usize::from(image[16]);
        let root_entries = word(17);
        let total = word(19) * sector;
        let fat_bytes = word(22) * sector;
        let root_start = fat_start + fats * fat_bytes;
        let data_start = root_start + (root_entries * 32).div_ceil(sector) * sector;
        if sector == 0 || cluster_bytes == 0 || total != image.len() || data_start > total {
            return Err("BPB does not describe this image".into());
        }
        Ok(Self {
            image,
            cluster_bytes,
            fat_start,
            fat_bytes,
            fats,
            root_start,
            root_entries,
            data_start,
            clusters: (total - data_start) / cluster_bytes,
        })
    }

    /// Bytes per cluster.
    pub fn cluster_bytes(&self) -> usize {
        self.cluster_bytes
    }

    /// The 12-bit entry of `cluster` in the first FAT.
    pub fn fat_entry(&self, cluster: u16) -> u16 {
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
        self.raw_root()
            .find(|entry| entry.attributes & 0x08 != 0 && entry.attributes != 0x0F)
            .map(|entry| String::from_utf8_lossy(entry.raw[..11].trim_ascii_end()).into_owned())
    }

    /// Data-area bytes in free clusters, as DOS DIR reports them.
    pub fn free_bytes(&self) -> usize {
        (2..self.clusters + 2)
            .filter(|&cluster| self.fat_entry(cluster as u16) == 0)
            .count()
            * self.cluster_bytes
    }

    fn raw_root(&self) -> impl Iterator<Item = Entry> + '_ {
        entries(&self.image[self.root_start..self.root_start + self.root_entries * 32])
    }

    /// The clusters of a chain, refusing free, reserved or looping links.
    pub fn chain(&self, first: u16) -> Result<Vec<u16>, String> {
        let mut chain = Vec::new();
        let mut cluster = first;
        loop {
            if !(2..self.clusters as u16 + 2).contains(&cluster) {
                return Err(format!("chain from {first} reaches cluster {cluster}"));
            }
            if chain.len() > self.clusters {
                return Err(format!("chain from {first} loops"));
            }
            chain.push(cluster);
            match self.fat_entry(cluster) {
                0xFF8..=0xFFF => return Ok(chain),
                next => cluster = next,
            }
        }
    }

    fn chain_bytes(&self, first: u16) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        for cluster in self.chain(first)? {
            let at = self.data_start + (usize::from(cluster) - 2) * self.cluster_bytes;
            bytes.extend_from_slice(&self.image[at..at + self.cluster_bytes]);
        }
        Ok(bytes)
    }

    /// Entries of a folder, `.` and `..` included for a subdirectory and the
    /// volume label excluded for the root. `path` is `""` for the root or
    /// backslash-separated folder names such as `GAME\DATA`; case is ignored.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, String> {
        let mut entries: Vec<Entry> = self
            .raw_root()
            .filter(|entry| entry.attributes & 0x08 == 0)
            .collect();
        for part in path.split('\\').filter(|part| !part.is_empty()) {
            let folder = entries
                .iter()
                .find(|entry| entry.is_dir() && entry.name.eq_ignore_ascii_case(part))
                .ok_or_else(|| format!("no folder {part} in {path}"))?;
            let bytes = self.chain_bytes(folder.first_cluster)?;
            entries = self::entries(&bytes).collect();
        }
        Ok(entries)
    }

    /// The contents of a file at a backslash-separated path.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        let (folder, name) = path.rsplit_once('\\').unwrap_or(("", path));
        let entry = self
            .list(folder)?
            .into_iter()
            .find(|entry| !entry.is_dir() && entry.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no file {path}"))?;
        let size = entry.size as usize;
        if size == 0 {
            return if entry.first_cluster == 0 {
                Ok(Vec::new())
            } else {
                Err(format!("empty file {path} owns a cluster"))
            };
        }
        let chain = self.chain(entry.first_cluster)?;
        if chain.len() != size.div_ceil(self.cluster_bytes) {
            return Err(format!("{path}: chain length does not match size"));
        }
        let mut bytes = self.chain_bytes(entry.first_cluster)?;
        bytes.truncate(size);
        Ok(bytes)
    }

    /// Every file and folder below the root as (path, entry), depth first.
    pub fn walk(&self) -> Result<Vec<(String, Entry)>, String> {
        let mut found = Vec::new();
        let mut pending = vec![String::new()];
        while let Some(folder) = pending.pop() {
            for entry in self.list(&folder)? {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                let path = if folder.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{folder}\\{}", entry.name)
                };
                if entry.is_dir() {
                    pending.push(path.clone());
                }
                found.push((path, entry));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(found)
    }

    /// Structural check: identical FAT copies, reserved entries 0 and 1, every
    /// subdirectory starting with `.` (itself) and `..` (its parent, 0 for the
    /// root), chain lengths matching file sizes, no cluster in two chains and
    /// no allocated cluster outside every chain.
    pub fn check(&self) -> Result<(), String> {
        let fat = |copy: usize| {
            let at = self.fat_start + copy * self.fat_bytes;
            &self.image[at..at + self.fat_bytes]
        };
        if (1..self.fats).any(|copy| fat(copy) != fat(0)) {
            return Err("FAT copies differ".into());
        }
        if self.fat_entry(0) != 0xF00 | u16::from(self.image[21]) || self.fat_entry(1) != 0xFFF {
            return Err("reserved FAT entries are wrong".into());
        }
        let mut owner = vec![false; self.clusters + 2];
        let mut claim = |chain: &[u16]| -> Result<(), String> {
            for &cluster in chain {
                if std::mem::replace(&mut owner[usize::from(cluster)], true) {
                    return Err(format!("cluster {cluster} is in two chains"));
                }
            }
            Ok(())
        };
        let mut pending = vec![(String::new(), 0u16)];
        while let Some((folder, cluster)) = pending.pop() {
            let entries = self.list(&folder)?;
            let members = if folder.is_empty() {
                &entries[..]
            } else {
                let dots = entries.get(..2).ok_or("folder without dot entries")?;
                let parent = pending_parent(&folder, self)?;
                if dots[0].raw[..11] != *b".          "
                    || dots[0].first_cluster != cluster
                    || dots[1].raw[..11] != *b"..         "
                    || dots[1].first_cluster != parent
                {
                    return Err(format!("{folder}: wrong . or .. entry"));
                }
                claim(&self.chain(cluster)?)?;
                &entries[2..]
            };
            for entry in members {
                let path = if folder.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{folder}\\{}", entry.name)
                };
                if entry.is_dir() {
                    pending.push((path, entry.first_cluster));
                } else if entry.size > 0 {
                    self.read(&path)?;
                    claim(&self.chain(entry.first_cluster)?)?;
                } else if entry.first_cluster != 0 {
                    return Err(format!("empty file {path} owns a cluster"));
                }
            }
        }
        for (cluster, owned) in owner.iter().enumerate().skip(2) {
            if self.fat_entry(cluster as u16) != 0 && !owned {
                return Err(format!("cluster {cluster} is allocated but unreachable"));
            }
        }
        Ok(())
    }
}

/// Directory entries up to the end marker, deleted ones left out.
fn entries(bytes: &[u8]) -> impl Iterator<Item = Entry> + '_ {
    bytes
        .as_chunks::<32>()
        .0
        .iter()
        .take_while(|raw| raw[0] != 0)
        .filter(|raw| raw[0] != 0xE5)
        .map(Entry::decode)
}

/// First cluster of a folder's parent: 0 when the parent is the root.
fn pending_parent(folder: &str, volume: &Volume<'_>) -> Result<u16, String> {
    let Some((parent, _)) = folder.rsplit_once('\\') else {
        return Ok(0);
    };
    let (grand, name) = parent.rsplit_once('\\').unwrap_or(("", parent));
    volume
        .list(grand)?
        .into_iter()
        .find(|entry| entry.is_dir() && entry.name.eq_ignore_ascii_case(name))
        .map(|entry| entry.first_cluster)
        .ok_or_else(|| format!("no folder {parent}"))
}
