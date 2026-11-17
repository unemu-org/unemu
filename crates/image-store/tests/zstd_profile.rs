// SPDX-License-Identifier: AGPL-3.0-or-later
//! Test vectors of Zstandard encoder profile 1, the encoder behind every
//! `+zstd` payload of the version-1 media types; machine-images §2 and the
//! `codec` module documentation.
//!
//! Each vector is a payload of one compressed kind, built here by fixed rules
//! from no fixture, with the length and SHA-256 of the frame profile 1 writes
//! for it, and the frame itself when it is short. A pinned recipe's lock names
//! these stored digests, so a vector that fails means the encoder in use no
//! longer writes profile 1. Re-pinning never fixes that: an encoder that
//! writes other bytes is a new profile beside this one.

use image_store::{codec, media_type as mt, Descriptor, Digest, PayloadType, Store};
use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

/// The disk payloads the PC provider declares compressible. The encoder is the
/// same for every kind, so these limits only need to admit the vectors.
const DISK: &[PayloadType] = &[
    PayloadType {
        media_type: "application/vnd.unemu.disk.base.v1+zstd",
        max_size: 1 << 40,
    },
    PayloadType {
        media_type: "application/vnd.unemu.disk.sectors.v1+zstd",
        max_size: 1 << 40,
    },
];

/// Level 3's largest window, 2 MiB: a larger payload gets a windowed frame.
const WINDOW: u64 = 1 << 21;

struct Vector {
    /// What the input imitates.
    name: &'static str,
    kind: &'static str,
    codecs: &'static [PayloadType],
    input: fn() -> Vec<u8>,
    decoded: u64,
    /// Length and SHA-256 of the frame.
    stored: u64,
    digest: &'static str,
    /// The whole frame in hexadecimal, for the short ones.
    frame: Option<&'static str>,
}

/// One vector per compressed payload kind. Their sizes cover each row of
/// libzstd's level-3 parameter table (up to 16 KiB, 128 KiB, 256 KiB and
/// beyond) and both frame shapes: single segment up to 2 MiB, windowed above.
const VECTORS: [Vector; 7] = [
    Vector {
        name: "floppy: boot sector, FATs and root directory of a 1.44M disk",
        kind: mt::FLOPPY,
        codecs: codec::MEDIA_PAYLOAD_TYPES,
        input: floppy,
        decoded: 10240,
        stored: 244,
        digest: "sha256:104253b3c914aef855e5953249a55b69d863ac387afc518c80f8d6ce9a7af25a",
        frame: Some(concat!(
            "28b52ffd640027350700140ceb3c90554e454d55312e30000201010002e00040",
            "0bf0090012000200291711262050524f46494c45312020204641543132202020",
            "fa31c08ed88ed0bc007cfbbe787cac08c07409b40ebb0700cd10ebf231c0cd16",
            "cd190053796e746865746963207465737420766f6c756d653a206e6f20737973",
            "74656d20686572652e0d0a0055aaf0ffff03400005600007800009f0ff0bf0ff",
            "000860715d524541444d4520205458542060715d0200a00f0000444154412020",
            "202042494e2060715d0a00e8030b009f409920321013c0b33faa39ec528de88f",
            "01393da81ea067d1026a59503730c803ed7e2537",
        )),
    },
    Vector {
        name: "tar: a firmware source archive of two text members",
        kind: "application/x-tar",
        codecs: codec::FIRMWARE_PAYLOAD_TYPES,
        input: tar,
        decoded: 10752,
        stored: 2261,
        digest: "sha256:be78feb3ef2a2cab713263efd69a8948d120301e096641090c870362615fdb29",
        frame: None,
    },
    Vector {
        name: "firmware: a 64 KiB system ROM",
        kind: mt::FIRMWARE,
        codecs: codec::FIRMWARE_PAYLOAD_TYPES,
        input: rom,
        decoded: 65536,
        stored: 12183,
        digest: "sha256:d48d1fbee3ac4032c60b8c2b98d36562b0fce846ff66f962563a7822f68a1993",
        frame: None,
    },
    Vector {
        name: "optical: the first 24 sectors of an ISO 9660 image",
        kind: mt::OPTICAL,
        codecs: codec::MEDIA_PAYLOAD_TYPES,
        input: optical,
        decoded: 49152,
        stored: 2963,
        digest: "sha256:a892c454a79127407f2cd801465a1a8339b88e4c97a2ccd1f4f19b4c60f13e06",
        frame: None,
    },
    Vector {
        name: "sector pack: 300 records of a disk session",
        kind: mt::DISK_SECTORS,
        codecs: DISK,
        input: sector_pack,
        decoded: 156000,
        stored: 48427,
        digest: "sha256:5de34d841574441bcbac826163ebd2b1bcee2e12f16e7b3d903571e5bf72b73f",
        frame: None,
    },
    Vector {
        name: "extra: a 300 KiB text file",
        kind: mt::EXTRA,
        codecs: codec::MEDIA_PAYLOAD_TYPES,
        input: extra,
        decoded: 307200,
        stored: 75774,
        digest: "sha256:fc80bc6e7c0fc4fe837d120f2f748480ae4b0101cca3983da7421fc6dd7e52ee",
        frame: None,
    },
    Vector {
        name: "disk base: a 3 MiB FAT16 hard disk",
        kind: mt::DISK_BASE,
        codecs: DISK,
        input: disk_base,
        decoded: 3 << 20,
        stored: 218128,
        digest: "sha256:f4392492c81b88e68a057f13d5b438de43c3204f93f0476f094c35a6395ade57",
        frame: None,
    },
];

/// A fixed xorshift32 stream, so every input is the same on every host.
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }
    fn fill(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            *byte = self.next() as u8;
        }
    }
}

const WORDS: [&str; 32] = [
    "disk", "sector", "track", "head", "drive", "boot", "image", "frame", "window", "level",
    "block", "match", "literal", "offset", "length", "copy", "table", "entry", "file", "volume",
    "label", "cluster", "chain", "byte", "word", "page", "line", "text", "field", "value",
    "profile", "vector",
];

/// Numbered lines of words, as in a text file, cut to exactly `length` bytes.
fn prose(seed: u32, length: usize) -> Vec<u8> {
    let mut noise = Noise(seed);
    let mut text = String::with_capacity(length + 80);
    let mut line = 0;
    while text.len() < length {
        line += 1;
        write!(text, "{line:05}").unwrap();
        for _ in 0..3 + noise.below(8) {
            write!(text, " {}", WORDS[noise.below(WORDS.len())]).unwrap();
        }
        text.push_str("\r\n");
    }
    text.truncate(length);
    text.into_bytes()
}

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// A FAT volume's shape, as its BIOS parameter block records it.
struct Volume {
    sectors: u32,
    media: u8,
    per_cluster: u8,
    root_entries: u16,
    fat_sectors: u16,
    per_track: u16,
    heads: u16,
    hidden: u32,
    drive: u8,
    fs_type: &'static [u8; 8],
}

/// A FAT boot sector: BIOS parameter block, a loader stub that prints a
/// message and reboots on a key, and the boot signature.
fn boot_sector(sector: &mut [u8], volume: &Volume) {
    sector[..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    sector[3..11].copy_from_slice(b"UNEMU1.0");
    put16(sector, 11, 512);
    sector[13] = volume.per_cluster;
    put16(sector, 14, 1);
    sector[16] = 2;
    put16(sector, 17, volume.root_entries);
    match u16::try_from(volume.sectors) {
        Ok(sectors) => put16(sector, 19, sectors),
        Err(_) => put32(sector, 32, volume.sectors),
    }
    sector[21] = volume.media;
    put16(sector, 22, volume.fat_sectors);
    put16(sector, 24, volume.per_track);
    put16(sector, 26, volume.heads);
    put32(sector, 28, volume.hidden);
    sector[36] = volume.drive;
    sector[38] = 0x29;
    put32(sector, 39, 0x2026_1117);
    sector[43..54].copy_from_slice(b"PROFILE1   ");
    sector[54..62].copy_from_slice(volume.fs_type);
    let stub = [
        0xfa, 0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xd0, 0xbc, 0x00, 0x7c, 0xfb, 0xbe, 0x78, 0x7c, 0xac,
        0x08, 0xc0, 0x74, 0x09, 0xb4, 0x0e, 0xbb, 0x07, 0x00, 0xcd, 0x10, 0xeb, 0xf2, 0x31, 0xc0,
        0xcd, 0x16, 0xcd, 0x19,
    ];
    sector[62..62 + stub.len()].copy_from_slice(&stub);
    let message = b"Synthetic test volume: no system here.\r\n\0";
    sector[0x78..0x78 + message.len()].copy_from_slice(message);
    sector[510..512].copy_from_slice(&[0x55, 0xaa]);
}

fn fat12(fat: &mut [u8], cluster: usize, value: u16) {
    let at = cluster * 3 / 2;
    if cluster.is_multiple_of(2) {
        fat[at] = value as u8;
        fat[at + 1] = (fat[at + 1] & 0xf0) | (value >> 8) as u8;
    } else {
        fat[at] = (fat[at] & 0x0f) | (value << 4) as u8;
        fat[at + 1] = (value >> 4) as u8;
    }
}

/// One 32-byte directory entry dated 2026-11-17 12:00.
fn entry(slot: &mut [u8], name: &[u8], attributes: u8, cluster: u16, size: u32) {
    slot[..11].copy_from_slice(name);
    slot[11] = attributes;
    put16(slot, 22, 12 << 11);
    put16(slot, 24, (46 << 9) | (11 << 5) | 17);
    put16(slot, 26, cluster);
    put32(slot, 28, size);
}

/// The first 20 sectors of a 1.44M FAT12 floppy: boot sector, both FATs and
/// the first root-directory sector.
fn floppy() -> Vec<u8> {
    let mut image = vec![0; 20 * 512];
    let volume = Volume {
        sectors: 2880,
        media: 0xf0,
        per_cluster: 1,
        root_entries: 224,
        fat_sectors: 9,
        per_track: 18,
        heads: 2,
        hidden: 0,
        drive: 0,
        fs_type: b"FAT12   ",
    };
    boot_sector(&mut image[..512], &volume);
    for first in [1, 10] {
        let fat = &mut image[first * 512..(first + 9) * 512];
        fat[..3].copy_from_slice(&[0xf0, 0xff, 0xff]);
        // README.TXT in clusters 2 to 9, DATA.BIN in 10 and 11.
        for cluster in 2..12 {
            let next = if cluster == 9 || cluster == 11 {
                0xfff
            } else {
                cluster as u16 + 1
            };
            fat12(fat, cluster, next);
        }
    }
    let root = &mut image[19 * 512..];
    entry(&mut root[..32], b"PROFILE1   ", 0x08, 0, 0);
    entry(&mut root[32..64], b"README  TXT", 0x20, 2, 4000);
    entry(&mut root[64..96], b"DATA    BIN", 0x20, 10, 1000);
    image
}

/// A ustar archive of a README and an assembly listing, with zero times and
/// owners as a reproducible source archive has them.
fn tar() -> Vec<u8> {
    let mut noise = Noise(0x7a12_0001);
    let mut listing = String::new();
    for line in 0..160 {
        let port = noise.below(256);
        let value = noise.next() & 0xffff;
        writeln!(
            listing,
            "l{line:03}:\tmov\tax, 0x{value:04x}\n\tout\t0x{port:02x}, al"
        )
        .unwrap();
    }
    let members = [
        ("source/README", prose(0x7a12_0002, 3000)),
        ("source/rom.asm", listing.into_bytes()),
    ];
    let mut archive = Vec::new();
    for (name, body) in members {
        let mut header = [0_u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        archive.extend_from_slice(&header);
        archive.extend_from_slice(&body);
        archive.resize(archive.len().next_multiple_of(512), 0);
    }
    archive.resize(archive.len() + 1024, 0);
    archive
}

/// A 64 KiB system ROM: unused space, code made of helpers of which some
/// appear twice, a message table, the reset vector, a date, a model byte
/// and the checksum byte.
fn rom() -> Vec<u8> {
    let mut rom = vec![0xff; 0x10000];
    let mut noise = Noise(0x0b10_5001);
    let mut code = Vec::new();
    while code.len() < 0x6000 {
        let mut helper = vec![0; 16 + noise.below(112)];
        noise.fill(&mut helper);
        // Real code is far from uniform: favour a few opcode values.
        for byte in &mut helper {
            if *byte & 3 != 0 {
                *byte = [0x8b, 0xe8, 0x89, 0x74, 0xb0, 0xe6, 0x50, 0x58][usize::from(*byte >> 5)];
            }
        }
        helper.push(0xc3);
        code.extend_from_slice(&helper);
        if noise.below(3) == 0 {
            code.extend_from_slice(&helper);
        }
    }
    code.truncate(0x6000);
    rom[0x8000..0xe000].copy_from_slice(&code);
    let mut table = Vec::new();
    for (index, word) in WORDS.iter().enumerate() {
        table.extend_from_slice(format!("{word} test {index} failed\r\n\0").as_bytes());
    }
    rom[0xe000..0xe000 + table.len()].copy_from_slice(&table);
    rom[0xfff0..0xfff5].copy_from_slice(&[0xea, 0x5b, 0xe0, 0x00, 0xf0]);
    rom[0xfff5..0xfffd].copy_from_slice(b"11/17/26");
    rom[0xfffe] = 0xfc;
    let sum = rom[..0xffff]
        .iter()
        .fold(0_u8, |sum, &b| sum.wrapping_add(b));
    rom[0xffff] = sum.wrapping_neg();
    rom
}

/// ISO 9660 numbers are recorded in both byte orders.
fn both16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    bytes[at + 2..at + 4].copy_from_slice(&value.to_be_bytes());
}
fn both32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    bytes[at + 4..at + 8].copy_from_slice(&value.to_be_bytes());
}

/// An ISO 9660 directory record; returns its length.
fn record(at: &mut [u8], extent: u32, size: u32, flags: u8, name: &[u8]) -> usize {
    let length = (33 + name.len()).next_multiple_of(2);
    at[0] = length as u8;
    both32(at, 2, extent);
    both32(at, 10, size);
    at[18..25].copy_from_slice(&[126, 11, 17, 12, 0, 0, 0]);
    at[25] = flags;
    both16(at, 28, 1);
    at[32] = name.len() as u8;
    at[33..33 + name.len()].copy_from_slice(name);
    length
}

/// The first 24 sectors of an ISO 9660 image: the empty system area, a
/// primary volume descriptor, the set terminator, the root directory and one
/// text file.
fn optical() -> Vec<u8> {
    const SECTOR: usize = 2048;
    let mut image = vec![0; 24 * SECTOR];
    let pvd = &mut image[16 * SECTOR..17 * SECTOR];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[8..72].fill(b' ');
    pvd[40..48].copy_from_slice(b"PROFILE1");
    both32(pvd, 80, 24);
    both16(pvd, 120, 1);
    both16(pvd, 124, 1);
    both16(pvd, 128, SECTOR as u16);
    record(&mut pvd[156..190], 18, SECTOR as u32, 0x02, &[0]);
    pvd[190..813].fill(b' ');
    for date in 0..4 {
        let at = 813 + date * 17;
        pvd[at..at + 16].copy_from_slice(b"2026111712000000");
    }
    pvd[881] = 1;
    let terminator = &mut image[17 * SECTOR..18 * SECTOR];
    terminator[0] = 255;
    terminator[1..6].copy_from_slice(b"CD001");
    terminator[6] = 1;
    let text = prose(0x0c0d_0001, 5 * SECTOR - 300);
    let root = &mut image[18 * SECTOR..19 * SECTOR];
    let mut at = record(root, 18, SECTOR as u32, 0x02, &[0]);
    at += record(&mut root[at..], 18, SECTOR as u32, 0x02, &[1]);
    record(&mut root[at..], 19, text.len() as u32, 0, b"README.TXT;1");
    image[19 * SECTOR..19 * SECTOR + text.len()].copy_from_slice(&text);
    image
}

/// A sector pack of 300 records, `u64le LBA + 512 data bytes` in ascending
/// LBA order: directory sectors, text, near copies, noise and zeros, as a
/// session's writes leave them.
fn sector_pack() -> Vec<u8> {
    let mut noise = Noise(0x5ec7_0001);
    let text = prose(0x5ec7_0002, 300 * 512);
    let mut pack = Vec::with_capacity(300 * 520);
    let mut lba = 144_u64;
    let mut previous = [0_u8; 512];
    for index in 0..300 {
        lba += 1 + noise.below(3) as u64;
        let mut data = [0_u8; 512];
        match index % 5 {
            0 => {
                for (slot, chunk) in data.chunks_mut(32).enumerate() {
                    let name = format!("F{:03}{slot:02}  DAT", index % 1000);
                    entry(
                        chunk,
                        name.as_bytes(),
                        0x20,
                        lba as u16,
                        (index * slot) as u32,
                    );
                }
            }
            1 => data.copy_from_slice(&text[index * 512..(index + 1) * 512]),
            2 => {
                data = previous;
                data[noise.below(512)] ^= 0x5a;
            }
            3 => noise.fill(&mut data),
            _ => {}
        }
        pack.extend_from_slice(&lba.to_le_bytes());
        pack.extend_from_slice(&data);
        previous = data;
    }
    pack
}

/// A 300 KiB text file, as an installation set's documentation extra.
fn extra() -> Vec<u8> {
    prose(0xe7a0_0001, 300 * 1024)
}

/// A 3 MiB FAT16 hard disk: partition table, boot sector, both FATs, the
/// root directory, then files of text, noise and a repeated record, free
/// space, and a copy of the text's start 2.5 MiB after it, beyond the 2 MiB
/// window, so a larger window would change the frame.
fn disk_base() -> Vec<u8> {
    const SECTOR: usize = 512;
    const TOTAL: usize = 6144;
    const START: usize = 63;
    const DATA: usize = 144;
    let mut disk = vec![0; TOTAL * SECTOR];
    let mbr = &mut disk[..SECTOR];
    mbr[0x1be..0x1c2].copy_from_slice(&[0x80, 1, 1, 0]);
    mbr[0x1c2..0x1c6].copy_from_slice(&[0x06, 15, 0x3f, 5]);
    put32(mbr, 0x1c6, START as u32);
    put32(mbr, 0x1ca, (TOTAL - START) as u32);
    mbr[510..512].copy_from_slice(&[0x55, 0xaa]);
    let volume = Volume {
        sectors: (TOTAL - START) as u32,
        media: 0xf8,
        per_cluster: 1,
        root_entries: 512,
        fat_sectors: 24,
        per_track: 63,
        heads: 16,
        hidden: START as u32,
        drive: 0x80,
        fs_type: b"FAT16   ",
    };
    boot_sector(&mut disk[START * SECTOR..(START + 1) * SECTOR], &volume);
    let text = prose(0xd15c_0001, 256 * 1024);
    let mut random = vec![0; 128 * 1024];
    Noise(0xd15c_0002).fill(&mut random);
    let mut table = Vec::with_capacity(512 * 1024);
    for index in 0..128_u32 {
        let mut record = [0x20_u8; 4096];
        record[..4].copy_from_slice(&index.to_le_bytes());
        record[4..36].copy_from_slice(b"profile 1 vector: a table record");
        table.extend_from_slice(&record);
    }
    // Files by first sector, each one contiguous.
    let files: [(&[u8; 11], usize, &[u8]); 4] = [
        (b"MANUAL  TXT", DATA, &text),
        (b"NOISE   BIN", DATA + 512, &random),
        (b"TABLE   DAT", DATA + 768, &table),
        (b"COPY    TXT", 5280, &text[..64 * 1024]),
    ];
    let root = (START + 1 + 2 * 24) * SECTOR;
    entry(&mut disk[root..root + 32], b"PROFILE1   ", 0x08, 0, 0);
    for (index, (name, first, bytes)) in files.iter().enumerate() {
        let cluster = first - DATA + 2;
        let count = bytes.len().div_ceil(SECTOR);
        let slot = root + 32 * (index + 1);
        entry(
            &mut disk[slot..slot + 32],
            *name,
            0x20,
            cluster as u16,
            bytes.len() as u32,
        );
        disk[first * SECTOR..first * SECTOR + bytes.len()].copy_from_slice(bytes);
        for fat in [START + 1, START + 1 + 24] {
            for n in cluster..cluster + count {
                let next = if n + 1 == cluster + count {
                    0xffff
                } else {
                    n as u16 + 1
                };
                put16(&mut disk[fat * SECTOR..], n * 2, next);
            }
            put16(&mut disk[fat * SECTOR..], 0, 0xfff8);
            put16(&mut disk[fat * SECTOR..], 2, 0xffff);
        }
    }
    // The last sectors carry only their own number, as a formatter's test
    // pattern leaves them.
    for lba in 5600..TOTAL {
        let at = lba * SECTOR;
        disk[at..at + 16].copy_from_slice(format!("sector {lba:09}").as_bytes());
    }
    disk
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, b| {
        write!(text, "{b:02x}").unwrap();
        text
    })
}

struct Temp(PathBuf);
impl Temp {
    fn store() -> (Self, Store) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temp = Self(std::env::temp_dir().join(format!(
            "zstd-profile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let store = Store::open(&temp.0).unwrap();
        (temp, store)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The descriptor and stored bytes of `input` through both production paths,
/// the descriptor computed without writing and the import, which must agree.
fn store_payload(
    store: &Store,
    input: &[u8],
    kind: &str,
    codecs: &[PayloadType],
) -> (Descriptor, Vec<u8>) {
    let size = input.len() as u64;
    let described = codec::describe(input, kind, size, codecs).unwrap();
    let imported = store
        .import_payload(input, kind, size, Some(&Digest::of(input)), codecs)
        .unwrap();
    assert_eq!(described, imported);
    let path = store
        .blob_file_path(&imported.digest, imported.size)
        .unwrap();
    (imported, fs::read(path).unwrap())
}

/// The frame the store writes for `vector`, through the import path.
fn stored_frame(store: &Store, vector: &Vector, input: &[u8]) -> Vec<u8> {
    let (descriptor, frame) = store_payload(store, input, vector.kind, vector.codecs);
    assert_eq!(descriptor.media_type, codec::compressed_type(vector.kind));
    frame
}

#[test]
fn profile_1_writes_the_pinned_frame_of_every_vector() {
    let (_temp, store) = Temp::store();
    let mut report = String::new();
    let mut failed = false;
    for vector in &VECTORS {
        let input = (vector.input)();
        let size = input.len() as u64;
        // The descriptor computed without writing and the stored blob agree.
        let described =
            codec::describe(input.as_slice(), vector.kind, size, vector.codecs).unwrap();
        let frame = stored_frame(&store, vector, &input);
        assert_eq!(described.digest, Digest::of(&frame), "{}", vector.name);
        assert_eq!(described.size, frame.len() as u64, "{}", vector.name);
        assert_eq!(
            described.decoded_digest().unwrap(),
            Digest::of(&input),
            "{}",
            vector.name
        );
        assert_eq!(
            zstd::stream::decode_all(frame.as_slice()).unwrap(),
            input,
            "{}",
            vector.name
        );
        let matches = size == vector.decoded
            && frame.len() as u64 == vector.stored
            && described.digest.to_string() == vector.digest
            && vector.frame.is_none_or(|pinned| pinned == hex(&frame));
        failed |= !matches;
        writeln!(
            report,
            "{} {}: decoded {size}, stored {}, {}{}",
            if matches { "ok  " } else { "DIFF" },
            vector.name,
            frame.len(),
            described.digest,
            if frame.len() <= 256 {
                format!("\n      {}", hex(&frame))
            } else {
                String::new()
            }
        )
        .unwrap();
    }
    assert!(
        !failed,
        "the encoder no longer writes Zstandard profile 1; an encoder that \
         writes other bytes is a new profile, never a re-pin:\n{report}"
    );
}

/// The frame of the first 4096 bytes of the floppy vector: a payload exactly
/// at the threshold.
const THRESHOLD_FRAME: &str = concat!(
    "28b52ffd64000f2505005409eb3c90554e454d55312e30000201010002e00040",
    "0bf0090012000200291711262050524f46494c45312020204641543132202020",
    "fa31c08ed88ed0bc007cfbbe787cac08c07409b40ebb0700cd10ebf231c0cd16",
    "cd190053796e746865746963207465737420766f6c756d653a206e6f20737973",
    "74656d20686572652e0d0a0055aaf0ffff03400005600007800009f0ff0bf0ff",
    "00041000d5ebb368a965eb06ce032728f15e",
);
/// Length and SHA-256 of the frame of the floppy vector padded with zeros to
/// 2880 KiB, exactly the media-set floppy limit.
const LIMIT_FRAME: (u64, &str) = (
    348,
    "sha256:5720006f5d6a5106c96347032c09ebf7250c8176ef20483e44ec03dd00cf1c13",
);

#[test]
fn the_threshold_and_the_size_limits_decide_which_payloads_become_zstd() {
    // Which payloads are encoded decides stored digests as much as the
    // encoder does, so the threshold and the limits are part of profile 1.
    let (_temp, store) = Temp::store();
    let mut image = floppy();
    image.resize((2880 << 10) + 1, 0);
    let limit = 2880 << 10;
    let encode =
        |input: &[u8]| store_payload(&store, input, mt::FLOPPY, codec::MEDIA_PAYLOAD_TYPES);
    let mut report = String::new();
    for (case, input) in [
        ("one byte under the threshold", &image[..4095]),
        ("one byte over the floppy limit", &image[..]),
    ] {
        let (descriptor, stored) = encode(input);
        assert_eq!(descriptor.media_type, mt::FLOPPY, "{case} stays raw");
        assert!(descriptor.annotations.is_empty(), "{case}");
        assert_eq!(stored, input, "{case}");
    }
    let (at_threshold, frame) = encode(&image[..4096]);
    assert_eq!(
        at_threshold.media_type,
        codec::compressed_type(mt::FLOPPY),
        "a payload at the threshold is encoded"
    );
    if hex(&frame) != THRESHOLD_FRAME {
        writeln!(report, "threshold: {} {}", frame.len(), hex(&frame)).unwrap();
    }
    let (at_limit, frame) = encode(&image[..limit]);
    assert_eq!(
        at_limit.media_type,
        codec::compressed_type(mt::FLOPPY),
        "a floppy at the limit is encoded"
    );
    if (frame.len() as u64, at_limit.digest.to_string().as_str()) != LIMIT_FRAME {
        writeln!(report, "limit: {} {}", frame.len(), at_limit.digest).unwrap();
    }
    assert!(
        report.is_empty(),
        "the encoder no longer writes Zstandard profile 1; an encoder that \
         writes other bytes is a new profile, never a re-pin:\n{report}"
    );
    // The behaviour above comes from these values; they are profile 1 too.
    assert_eq!(codec::MIN_COMPRESS_SIZE, 4096);
    let table = |types: &[PayloadType]| -> Vec<(String, u64)> {
        types
            .iter()
            .map(|t| (t.media_type.to_owned(), t.max_size))
            .collect()
    };
    let expected = |rows: &[(&str, u64)]| -> Vec<(String, u64)> {
        rows.iter().map(|(t, n)| (t.to_string(), *n)).collect()
    };
    assert_eq!(
        table(codec::MEDIA_PAYLOAD_TYPES),
        expected(&[
            ("application/vnd.unemu.media.floppy.v1+zstd", 2880 << 10),
            ("application/vnd.unemu.media.optical.v1+zstd", 900 << 20),
            ("application/vnd.unemu.media.extra.v1+zstd", 2 << 30),
        ])
    );
    assert_eq!(
        table(codec::FIRMWARE_PAYLOAD_TYPES),
        expected(&[
            ("application/vnd.unemu.firmware.v1+zstd", 16 << 20),
            ("application/vnd.unemu.media.extra.v1+zstd", 2 << 30),
            ("application/x-tar+zstd", 2 << 30),
        ])
    );
}

/// The fields of a Zstandard frame header, RFC 8878 §3.1.1.1.
#[derive(Debug, PartialEq, Eq)]
struct Header {
    single_segment: bool,
    checksum: bool,
    reserved: bool,
    dictionary_id_flag: u8,
    window_descriptor: Option<u8>,
    content_size: Option<u64>,
}

fn header(frame: &[u8]) -> Header {
    assert_eq!(frame[..4], [0x28, 0xb5, 0x2f, 0xfd], "magic number");
    let descriptor = frame[4];
    let single_segment = descriptor & 0x20 != 0;
    let mut at = 5;
    let window_descriptor = (!single_segment).then(|| {
        at += 1;
        frame[5]
    });
    let dictionary_id_flag = descriptor & 3;
    at += [0, 1, 2, 4][usize::from(dictionary_id_flag)];
    let width = match descriptor >> 6 {
        0 => usize::from(single_segment),
        flag => 1 << flag,
    };
    let content_size = (width > 0).then(|| {
        let mut bytes = [0; 8];
        bytes[..width].copy_from_slice(&frame[at..at + width]);
        u64::from_le_bytes(bytes) + if width == 2 { 256 } else { 0 }
    });
    Header {
        single_segment,
        checksum: descriptor & 4 != 0,
        reserved: descriptor & 8 != 0,
        dictionary_id_flag,
        window_descriptor,
        content_size,
    }
}

#[test]
fn profile_1_frames_carry_the_frozen_frame_options() {
    let (_temp, store) = Temp::store();
    for vector in &VECTORS {
        let input = (vector.input)();
        let frame = stored_frame(&store, vector, &input);
        let size = input.len() as u64;
        // A window that holds the whole payload makes a single-segment frame;
        // a larger payload records level 3's 2 MiB window: exponent 11
        // (window log 21), mantissa 0.
        let windowed = size > WINDOW;
        assert_eq!(
            header(&frame),
            Header {
                single_segment: !windowed,
                checksum: true,
                reserved: false,
                dictionary_id_flag: 0,
                window_descriptor: windowed.then_some(11 << 3),
                content_size: Some(size),
            },
            "{}",
            vector.name
        );
        // One frame and nothing after it.
        assert_eq!(
            zstd::zstd_safe::find_frame_compressed_size(&frame),
            Ok(frame.len()),
            "{}",
            vector.name
        );
        // The content checksum is checked: one changed checksum byte fails.
        let mut damaged = frame.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert!(
            zstd::stream::decode_all(damaged.as_slice()).is_err(),
            "{}",
            vector.name
        );
    }
}

/// The encoder settings the profile fixes; the test perturbs one at a time.
#[derive(Clone, Copy, Debug)]
struct Settings {
    level: i32,
    checksum: bool,
    content_size: bool,
    pledged: bool,
    flush_after: Option<usize>,
    window_log: Option<u32>,
    long_distance: bool,
    dictionary: bool,
}

/// Profile 1 as the `codec` documentation states it.
const PROFILE_1: Settings = Settings {
    level: 3,
    checksum: true,
    content_size: true,
    pledged: true,
    flush_after: None,
    window_log: None,
    long_distance: false,
    dictionary: false,
};

/// The same streaming encoder the store uses, with `settings` applied.
fn encode(input: &[u8], settings: Settings) -> Vec<u8> {
    let text = prose(0xd1c7_0001, 16 * 1024);
    let dictionary: &[u8] = if settings.dictionary { &text } else { &[] };
    let mut encoder =
        zstd::stream::write::Encoder::with_dictionary(Vec::new(), settings.level, dictionary)
            .unwrap();
    encoder.include_checksum(settings.checksum).unwrap();
    encoder.include_contentsize(settings.content_size).unwrap();
    if settings.pledged {
        encoder
            .set_pledged_src_size(Some(input.len() as u64))
            .unwrap();
    }
    if let Some(log) = settings.window_log {
        encoder.window_log(log).unwrap();
    }
    if settings.long_distance {
        encoder.long_distance_matching(true).unwrap();
    }
    let split = settings.flush_after.unwrap_or(0).min(input.len());
    encoder.write_all(&input[..split]).unwrap();
    if settings.flush_after.is_some() {
        encoder.flush().unwrap();
    }
    encoder.write_all(&input[split..]).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn a_change_to_any_profile_setting_misses_the_pinned_vectors() {
    let inputs: Vec<Vec<u8>> = VECTORS.iter().map(|v| (v.input)()).collect();
    let digests = |settings: Settings| -> Vec<String> {
        inputs
            .iter()
            .map(|input| Digest::of(&encode(input, settings)).to_string())
            .collect()
    };
    let pinned: Vec<String> = VECTORS.iter().map(|v| v.digest.to_owned()).collect();
    // The stand-in encoder with profile 1's settings is the store's encoder.
    assert_eq!(digests(PROFILE_1), pinned);
    assert_eq!(codec::LEVEL, PROFILE_1.level);
    let with = |change: fn(&mut Settings)| {
        let mut settings = PROFILE_1;
        change(&mut settings);
        settings
    };
    let perturbed = [
        ("level 2", with(|s| s.level = 2)),
        ("level 4", with(|s| s.level = 4)),
        ("no checksum", with(|s| s.checksum = false)),
        ("no content size", with(|s| s.content_size = false)),
        ("size not pledged", with(|s| s.pledged = false)),
        ("a flush after 4 KiB", with(|s| s.flush_after = Some(4096))),
        ("window log 20", with(|s| s.window_log = Some(20))),
        ("window log 22", with(|s| s.window_log = Some(22))),
        ("long-distance matching", with(|s| s.long_distance = true)),
        ("a dictionary", with(|s| s.dictionary = true)),
    ];
    // Each change moves at least one pinned digest, so the vector test would
    // fail. A window change shows only in the payload larger than the window,
    // because libzstd shrinks a smaller payload's window to fit it anyway.
    for (change, settings) in perturbed {
        let caught = digests(settings)
            .iter()
            .zip(&pinned)
            .filter(|(actual, pinned)| actual != pinned)
            .count();
        assert!(caught > 0, "no vector notices {change}");
    }
}

/// One Cargo.lock entry: name, version, source and checksum as written, and
/// its dependencies by crate name. Cargo adds a version, and a source, to a
/// dependency only while several versions of that crate are locked, so the
/// suffix is dropped.
#[derive(Debug, PartialEq, Eq)]
struct Locked {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
    dependencies: Vec<String>,
}

fn locked(lock: &str) -> Vec<Locked> {
    lock.split("[[package]]")
        .skip(1)
        .map(|entry| {
            let field = |key: &str| {
                entry.lines().find_map(|line| {
                    let value = line.strip_prefix(key)?.strip_prefix(" = \"")?;
                    Some(value.strip_suffix('"')?.to_owned())
                })
            };
            let dependencies = entry
                .split_once("dependencies = [")
                .and_then(|(_, list)| list.split_once(']'))
                .map(|(list, _)| {
                    list.lines()
                        .filter_map(|line| {
                            let name = line.trim().trim_end_matches(',').trim_matches('"');
                            name.split_whitespace().next().map(str::to_owned)
                        })
                        .collect()
                })
                .unwrap_or_default();
            Locked {
                name: field("name").unwrap_or_default(),
                version: field("version").unwrap_or_default(),
                source: field("source"),
                checksum: field("checksum"),
                dependencies,
            }
        })
        .collect()
}

/// Crates named after zstd that are known never to write profile 1 frames,
/// a decode-only crate say: name and reason. None today.
const UNRELATED: &[(&str, &str)] = &[];

/// Profile 1's crates as Cargo.lock must resolve them: these versions from
/// crates.io with these checksums, and the C library built from the bundled
/// sources, with no other backend among the dependencies.
fn profile_1_crates() -> Vec<Locked> {
    let entry = |name: &str, version: &str, checksum: &str, dependencies: &[&str]| Locked {
        name: name.to_owned(),
        version: version.to_owned(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_owned()),
        checksum: Some(checksum.to_owned()),
        dependencies: dependencies.iter().map(|d| d.to_string()).collect(),
    };
    vec![
        entry(
            "zstd",
            "0.13.5",
            "50adf04cf3ec914b0b9905c9dde90c80fe74bfd701e331fc80a39c512c525278",
            &["zstd-safe"],
        ),
        entry(
            "zstd-safe",
            "7.4.1",
            "498d94ad68720c89b4fbe41b9090850651e68cb2dd9210ce7ffbff3a94653819",
            &["zstd-sys"],
        ),
        entry(
            "zstd-sys",
            "2.1.3+zstd.1.5.7",
            "f729a06d0257428fd9d997ac8db7e1407bd0262ab016b085b36bc52957c0052d",
            &["cc", "pkg-config"],
        ),
    ]
}

fn check_lock(lock: &str) -> Result<(), String> {
    let named: Vec<Locked> = locked(lock)
        .into_iter()
        .filter(|c| c.name.contains("zstd") && !UNRELATED.iter().any(|(name, _)| *name == c.name))
        .collect();
    let others: Vec<String> = named
        .iter()
        .filter(|c| !["zstd", "zstd-safe", "zstd-sys"].contains(&c.name.as_str()))
        .map(|c| format!("{} {}", c.name, c.version))
        .collect();
    if !others.is_empty() {
        return Err(format!(
            "Cargo.lock now holds other crates named after zstd: {}. Profile 1 is \
             written by zstd, zstd-safe and zstd-sys alone. A crate that only \
             decodes, or never reaches image_store::codec, goes into UNRELATED \
             with the reason; one that can replace or reconfigure the encoder \
             makes a new profile.",
            others.join(", ")
        ));
    }
    let expected = profile_1_crates();
    if named != expected {
        return Err(format!(
            "Cargo.lock no longer resolves Zstandard profile 1's crates; an \
             encoder from other crates is a new profile, never a re-pin.\n\
             locked: {named:#?}\nprofile 1: {expected:#?}"
        ));
    }
    Ok(())
}

#[test]
fn cargo_lock_resolves_profile_1s_crates_and_nothing_else_named_zstd() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let lock = fs::read_to_string(&path).unwrap();
    if let Err(message) = check_lock(&lock) {
        panic!("{}: {message}", path.display());
    }
    // A second locked version of a dependency makes Cargo spell it with its
    // version; that alone changes nothing.
    let second_cc = lock.replace(
        " \"cc\",\n \"pkg-config\",",
        " \"cc 1.11.1\",\n \"pkg-config\",",
    );
    assert_ne!(second_cc, lock);
    assert_eq!(check_lock(&second_cc), Ok(()));
    // Another version, another source or another backend is noticed, and
    // another crate named after zstd is named in the message.
    for doctored in [
        lock.replace("2.1.3+zstd.1.5.7", "2.1.4+zstd.1.5.7"),
        lock.replace(
            "registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"498d",
            "git+https://example.org/zstd-rs#0\"\nchecksum = \"498d",
        ),
        lock.replace(
            " \"cc\",\n \"pkg-config\",",
            " \"cc\",\n \"libzstd-rs-sys\",\n \"pkg-config\",",
        ),
    ] {
        assert!(check_lock(&doctored).is_err());
    }
    let decoder = format!("{lock}\n[[package]]\nname = \"ruzstd\"\nversion = \"0.8.0\"\n");
    let message = check_lock(&decoder).unwrap_err();
    assert!(message.contains("ruzstd 0.8.0"), "{message}");
}
