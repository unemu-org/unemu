// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-memory test media; Microsoft FAT Specification, FAT12 BPB and directories.

/// Microsoft FAT Specification, BPB, FAT12 entries and FAT Directory Structure.
/// Follow the on-media cluster chain so DOS's cached directory cannot stand in
/// for the root entry and file contents actually written to the drive.
pub fn file(image: &[u8], name: &[u8; 11]) -> Option<Vec<u8>> {
    let word = |at| usize::from(u16::from_le_bytes([image[at], image[at + 1]]));
    let sector_bytes = word(11);
    let cluster_bytes = usize::from(image[13]) * sector_bytes;
    let fat_start = word(14) * sector_bytes;
    let fat_bytes = word(22) * sector_bytes;
    let root_start = fat_start + usize::from(image[16]) * fat_bytes;
    let root_bytes = word(17) * 32;
    let data_start = root_start + root_bytes.div_ceil(sector_bytes) * sector_bytes;
    let entry = image[root_start..root_start + root_bytes]
        .as_chunks::<32>()
        .0
        .iter()
        .take_while(|entry| entry[0] != 0)
        .find(|entry| entry[0] != 0xE5 && entry[11] & 0x18 == 0 && &entry[..11] == name)?;
    let size = u32::from_le_bytes(entry[28..32].try_into().unwrap()) as usize;
    assert!(size <= image.len(), "file size exceeds media size");
    let mut cluster = usize::from(u16::from_le_bytes([entry[26], entry[27]]));
    let fat = &image[fat_start..fat_start + fat_bytes];
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        assert!((2..0xFF0).contains(&cluster), "invalid FAT12 data cluster");
        let start = data_start + (cluster - 2) * cluster_bytes;
        let count = cluster_bytes.min(size - bytes.len());
        assert!(count > 0, "empty FAT12 cluster");
        bytes.extend_from_slice(&image[start..start + count]);
        let at = cluster + cluster / 2;
        let packed = u16::from_le_bytes([fat[at], fat[at + 1]]);
        cluster = usize::from(if cluster & 1 == 0 {
            packed & 0xFFF
        } else {
            packed >> 4
        });
    }
    assert!(
        size == 0 || cluster >= 0xFF8,
        "file chain must end after its data"
    );
    Some(bytes)
}

/// Insert a new root file into a disposable fixture, updating every FAT copy.
/// Existing vendor files stay intact; callers only add absent DOS 8.3 names.
pub fn add_file(image: &mut [u8], name: &[u8; 11], contents: &[u8]) {
    assert!(file(image, name).is_none(), "fixture file already exists");
    let word = |at| usize::from(u16::from_le_bytes([image[at], image[at + 1]]));
    let sector = word(11);
    let cluster_bytes = usize::from(image[13]) * sector;
    let fat_start = word(14) * sector;
    let fat_bytes = word(22) * sector;
    let fats = usize::from(image[16]);
    let root = fat_start + fats * fat_bytes;
    let root_bytes = word(17) * 32;
    let data = root + root_bytes.div_ceil(sector) * sector;
    let slot = (root..root + root_bytes)
        .step_by(32)
        .find(|&at| matches!(image[at], 0 | 0xE5))
        .expect("root directory full");
    let clusters: Vec<_> = (2..2 + (image.len() - data) / cluster_bytes)
        .filter(|&cluster| {
            let at = fat_start + cluster + cluster / 2;
            let packed = u16::from_le_bytes([image[at], image[at + 1]]);
            (if cluster & 1 == 0 {
                packed & 0xFFF
            } else {
                packed >> 4
            }) == 0
        })
        .take(contents.len().div_ceil(cluster_bytes))
        .collect();
    assert_eq!(
        clusters.len(),
        contents.len().div_ceil(cluster_bytes),
        "fixture disk full"
    );
    for (index, &cluster) in clusters.iter().enumerate() {
        let next = clusters.get(index + 1).copied().unwrap_or(0xFFF) as u16;
        for copy in 0..fats {
            let at = fat_start + copy * fat_bytes + cluster + cluster / 2;
            let old = u16::from_le_bytes([image[at], image[at + 1]]);
            let packed = if cluster & 1 == 0 {
                (old & 0xF000) | next
            } else {
                (old & 0x000F) | (next << 4)
            };
            image[at..at + 2].copy_from_slice(&packed.to_le_bytes());
        }
        let start = data + (cluster - 2) * cluster_bytes;
        let from = index * cluster_bytes;
        let count = cluster_bytes.min(contents.len() - from);
        image[start..start + cluster_bytes].fill(0);
        image[start..start + count].copy_from_slice(&contents[from..from + count]);
    }
    image[slot..slot + 32].fill(0);
    image[slot..slot + 11].copy_from_slice(name);
    image[slot + 11] = 0x20;
    image[slot + 26..slot + 28]
        .copy_from_slice(&(clusters.first().copied().unwrap_or(0) as u16).to_le_bytes());
    image[slot + 28..slot + 32].copy_from_slice(&(contents.len() as u32).to_le_bytes());
    assert_eq!(file(image, name).unwrap(), contents);
}
