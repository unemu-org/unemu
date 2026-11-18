// SPDX-License-Identifier: AGPL-3.0-or-later
//! The zip archives of a library (recipes §3.4): the central directory read
//! without unpacking, which gives each member's name, size and CRC-32, and
//! one member's bytes hashed when a recipe wants a file of its size.
//!
//! The rules are those of an `unpack` step (recipes §6.9): stored and
//! deflate members only, no encryption, no archive split over several disks.
//! The ZIP64 extension is not read in this version: an archive that needs it
//! for its directory cannot be listed, and a member whose sizes or offset
//! hold the ZIP64 marker is listed with that problem, never with a size it
//! does not have. Only bytes are taken from a member; names are hints for
//! the search and are reported as they are stored. A name that is not
//! ASCII or flagged UTF-8, or that two members share, is listed as
//! ambiguous and never searched: the unpack step must not guess which bytes
//! a recipe means.
//!
//! An archive is untrusted input. A deflated member is decoded with
//! `miniz_oxide`, and decoding stops with an error as soon as the output
//! passes the size the directory states, so a member that expands far past
//! it costs one buffer, not its expansion.

use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};
use sha2::{Digest as _, Sha256};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// The largest central directory read, far above any archive a library
/// holds; a larger one is refused rather than read into memory.
const MAX_DIRECTORY: u64 = 64 * 1024 * 1024;

/// One file member of an archive, as its central directory lists it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    /// The name decoded for people, `/` separators; see `raw_name`.
    pub name: String,
    /// The name's bytes as stored, lower-case hex.
    pub raw_name: String,
    /// The uncompressed size.
    pub size: u64,
    /// CRC-32 of the uncompressed bytes, lower-case hex.
    pub crc32: String,
    /// 0 stored, 8 deflate; anything else cannot be read.
    pub method: u16,
    pub compressed: u64,
    /// Where the member's local header starts.
    pub offset: u64,
    pub encrypted: bool,
    /// Why the member is never searched: ZIP64 fields, or an ambiguous name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

fn bad(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("zip archive: {message}"),
    )
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The file members of the archive at `path`, in central-directory order.
/// Folder entries are skipped.
pub(crate) fn list(path: &Path) -> io::Result<Vec<Member>> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    Ok(directory(&mut file, length)?
        .into_iter()
        .map(|entry| entry.member)
        .collect())
}

/// One file member and the DOS date and time its central directory states
/// (PKWARE APPNOTE §4.4.6: last mod file time and date).
#[derive(Clone, Debug)]
pub(crate) struct Listed {
    pub member: Member,
    pub time: u16,
    pub date: u16,
}

/// The file members of an archive of `length` bytes, in central-directory
/// order, with their DOS times. Folder entries are skipped.
fn directory<R: Read + Seek>(file: &mut R, length: u64) -> io::Result<Vec<Listed>> {
    let tail_length = length.min(65_535 + 22);
    let mut tail = vec![0u8; tail_length as usize];
    file.seek(SeekFrom::Start(length - tail_length))?;
    file.read_exact(&mut tail)?;
    // The end record is 22 bytes and a comment that runs to the file's end.
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&at| {
            u32_at(&tail, at) == 0x0605_4b50
                && at + 22 + usize::from(u16_at(&tail, at + 20)) == tail.len()
        })
        .ok_or_else(|| bad("no end of central directory record"))?;
    let record = &tail[end..end + 22];
    if u16_at(record, 4) != 0 || u16_at(record, 6) != 0 {
        return Err(bad("an archive split over several disks"));
    }
    let entries = u16_at(record, 10);
    let size = u64::from(u32_at(record, 12));
    let offset = u64::from(u32_at(record, 16));
    if entries == 0xffff || size == 0xffff_ffff || offset == 0xffff_ffff {
        return Err(bad("the ZIP64 extension, which this version does not read"));
    }
    if size > MAX_DIRECTORY || offset + size > length {
        return Err(bad("a central directory outside the file"));
    }
    let mut directory = vec![0u8; size as usize];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut directory)?;
    let mut members = Vec::new();
    let mut at = 0usize;
    for _ in 0..entries {
        if at + 46 > directory.len() || u32_at(&directory, at) != 0x0201_4b50 {
            return Err(bad("a damaged central directory"));
        }
        let header = &directory[at..at + 46];
        let flags = u16_at(header, 8);
        let name_length = usize::from(u16_at(header, 28));
        let skip = name_length + usize::from(u16_at(header, 30)) + usize::from(u16_at(header, 32));
        if at + 46 + skip > directory.len() {
            return Err(bad("a damaged central directory"));
        }
        let raw = &directory[at + 46..at + 46 + name_length];
        let utf8 = flags & 0x0800 != 0;
        let decoded = if raw.is_ascii() || utf8 {
            std::str::from_utf8(raw).ok()
        } else {
            None
        };
        let name = decoded
            .map(str::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(raw).into_owned())
            .replace('\\', "/");
        let (size, compressed, offset) =
            (u32_at(header, 24), u32_at(header, 20), u32_at(header, 42));
        let problem = if [size, compressed, offset].contains(&0xffff_ffff) {
            Some("ZIP64 sizes or offset, which this version does not read".to_owned())
        } else if decoded.is_none() {
            Some("a name that is neither ASCII nor flagged UTF-8: ambiguous".to_owned())
        } else {
            None
        };
        if !name.ends_with('/') {
            members.push(Listed {
                member: Member {
                    name,
                    raw_name: raw.iter().map(|b| format!("{b:02x}")).collect(),
                    size: u64::from(size),
                    crc32: format!("{:08x}", u32_at(header, 16)),
                    method: u16_at(header, 10),
                    compressed: u64::from(compressed),
                    offset: u64::from(offset),
                    encrypted: flags & 1 != 0,
                    problem,
                },
                time: u16_at(header, 12),
                date: u16_at(header, 14),
            });
        }
        at += 46 + skip;
    }
    // Two members of one name: which one a recipe means cannot be told.
    let mut seen = std::collections::BTreeMap::new();
    for listed in &members {
        *seen.entry(listed.member.name.clone()).or_insert(0usize) += 1;
    }
    for listed in &mut members {
        let member = &mut listed.member;
        if seen[&member.name] > 1 && member.problem.is_none() {
            member.problem = Some("a name two members share: ambiguous".to_owned());
        }
    }
    Ok(members)
}

/// The SHA-256 of a member's bytes, lower-case hex, after its size and
/// CRC-32 were checked against the central directory.
pub(crate) fn hash(path: &Path, member: &Member) -> io::Result<String> {
    let mut sha = Sha256::new();
    decode(&mut File::open(path)?, member, &mut |chunk| {
        sha.update(chunk);
        Ok(())
    })?;
    Ok(format!("{:x}", sha.finalize()))
}

/// Hand a member's bytes to `sink`, checked against the size and CRC-32 of
/// the central directory as they are decoded; the sink has seen the whole
/// member only when this returns `Ok`.
pub(crate) fn extract(
    path: &Path,
    member: &Member,
    sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    decode(&mut File::open(path)?, member, sink)
}

/// Decode one member of the archive `input`, checking the size and CRC-32
/// the central directory states, and stopping as soon as the output passes
/// the stated size.
fn decode<R: Read + Seek>(
    input: &mut R,
    member: &Member,
    sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    if member.encrypted {
        return Err(bad(format!("{}: an encrypted member", member.name)));
    }
    input.seek(SeekFrom::Start(member.offset))?;
    let mut local = [0u8; 30];
    input.read_exact(&mut local)?;
    if u32_at(&local, 0) != 0x0403_4b50 {
        return Err(bad(format!("{}: no local header", member.name)));
    }
    let data = member.offset + 30 + u64::from(u16_at(&local, 26)) + u64::from(u16_at(&local, 28));
    input.seek(SeekFrom::Start(data))?;
    let compressed = input.take(member.compressed);
    let mut crc = Crc32::default();
    let mut size = 0u64;
    let mut checked = |chunk: &[u8]| {
        size += chunk.len() as u64;
        if size > member.size {
            return Err(bad(format!(
                "{}: the data expands past the {} bytes the directory states",
                member.name, member.size
            )));
        }
        crc.update(chunk);
        sink(chunk)
    };
    match member.method {
        0 => copy(compressed, &mut checked)?,
        8 => deflated(compressed, &mut checked)?,
        method => {
            return Err(bad(format!(
                "{}: compression method {method}; only stored and deflate are read",
                member.name
            )))
        }
    }
    if size != member.size || format!("{:08x}", crc.finish()) != member.crc32 {
        return Err(bad(format!(
            "{}: the bytes do not match the size and CRC-32 the directory states",
            member.name
        )));
    }
    Ok(())
}

/// An archive held in memory, as an `unpack` step reads it: its directory
/// and each member's bytes.
pub(crate) struct Archive<'a> {
    bytes: &'a [u8],
    pub entries: Vec<Listed>,
}

impl<'a> Archive<'a> {
    pub(crate) fn open(bytes: &'a [u8]) -> io::Result<Self> {
        let entries = directory(&mut io::Cursor::new(bytes), bytes.len() as u64)?;
        Ok(Self { bytes, entries })
    }

    /// A member's bytes, checked against its size and CRC-32.
    pub(crate) fn read(&self, member: &Member) -> io::Result<Vec<u8>> {
        // The stated size is the archive's claim; deflate expands at most
        // 1032 to 1, so a claim beyond that is not reserved up front.
        let bound = member.compressed.saturating_mul(1032).saturating_add(64);
        let mut out = Vec::with_capacity(usize::try_from(member.size.min(bound)).unwrap_or(0));
        decode(&mut io::Cursor::new(self.bytes), member, &mut |chunk| {
            out.extend_from_slice(chunk);
            Ok(())
        })?;
        Ok(out)
    }
}

/// Decode raw DEFLATE data, handing the output on in chunks. The sink may
/// stop it with an error, which ends the decoding at once.
fn deflated(mut input: impl Read, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut buffer = vec![0u8; 64 * 1024];
    let (mut start, mut end) = (0usize, 0usize);
    let mut output = vec![0u8; 64 * 1024];
    let mut ended = false;
    loop {
        if start == end && !ended {
            end = loop {
                match input.read(&mut buffer) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            };
            start = 0;
            ended = end == 0;
        }
        let flush = if ended {
            MZFlush::Finish
        } else {
            MZFlush::None
        };
        let result = inflate(&mut state, &buffer[start..end], &mut output, flush);
        start += result.bytes_consumed;
        if result.bytes_written > 0 {
            sink(&output[..result.bytes_written])?;
        }
        match result.status {
            Ok(MZStatus::StreamEnd) => return Ok(()),
            Ok(_) => {}
            // No progress for want of input that is not there.
            Err(MZError::Buf) if ended && result.bytes_written == 0 => {
                return Err(bad("the deflate data ends before its last block"))
            }
            Err(MZError::Buf) => {}
            Err(error) => return Err(bad(format!("damaged deflate data ({error:?})"))),
        }
    }
}

fn copy(mut input: impl Read, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match input.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => sink(&buffer[..n])?,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// CRC-32 of zip and PNG (the reflected polynomial 0xEDB88320).
#[derive(Default)]
struct Crc32(u32);

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

impl Crc32 {
    fn update(&mut self, bytes: &[u8]) {
        let mut c = !self.0;
        for &byte in bytes {
            c = CRC_TABLE[((c ^ u32::from(byte)) & 0xff) as usize] ^ (c >> 8);
        }
        self.0 = !c;
    }

    fn finish(&self) -> u32 {
        self.0
    }
}

/// Test archives built in memory: stored members, or deflated ones whose
/// data the caller supplies, with the name bytes, flags and stated size a
/// test wants.
#[cfg(test)]
pub(crate) mod build {
    use super::Crc32;

    /// One member: its name, its bytes, the deflated form of the bytes when
    /// it is stored deflated, the general-purpose flags and, when it is to
    /// differ from the bytes', the size the directory states.
    pub(crate) struct Entry<'a> {
        pub name: &'a [u8],
        pub bytes: &'a [u8],
        pub deflated: Option<&'a [u8]>,
        pub flags: u16,
        pub stated: Option<u32>,
    }

    pub(crate) fn stored<'a>(name: &'a str, bytes: &'a [u8]) -> Entry<'a> {
        Entry {
            name: name.as_bytes(),
            bytes,
            deflated: None,
            flags: 0,
            stated: None,
        }
    }

    pub(crate) fn deflated<'a>(name: &'a str, bytes: &'a [u8], data: &'a [u8]) -> Entry<'a> {
        Entry {
            deflated: Some(data),
            ..stored(name, bytes)
        }
    }

    pub(crate) fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = Crc32::default();
        crc.update(bytes);
        crc.finish()
    }

    /// A zip archive of `entries`, in order.
    pub(crate) fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut directory = Vec::new();
        for entry in entries {
            let offset = out.len() as u32;
            let (method, data): (u16, &[u8]) = match entry.deflated {
                Some(data) => (8, data),
                None => (0, entry.bytes),
            };
            let crc = crc32(entry.bytes);
            let size = entry.stated.unwrap_or(entry.bytes.len() as u32);
            let common = |v: &mut Vec<u8>| {
                v.extend_from_slice(&20u16.to_le_bytes());
                v.extend_from_slice(&entry.flags.to_le_bytes());
                v.extend_from_slice(&method.to_le_bytes());
                v.extend_from_slice(&0u16.to_le_bytes());
                v.extend_from_slice(&0x21u16.to_le_bytes());
                v.extend_from_slice(&crc.to_le_bytes());
                v.extend_from_slice(&(data.len() as u32).to_le_bytes());
                v.extend_from_slice(&size.to_le_bytes());
                v.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
                v.extend_from_slice(&0u16.to_le_bytes());
            };
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            common(&mut out);
            out.extend_from_slice(entry.name);
            out.extend_from_slice(data);
            directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            directory.extend_from_slice(&20u16.to_le_bytes());
            common(&mut directory);
            directory.extend_from_slice(&[0; 8]);
            directory.extend_from_slice(&0u32.to_le_bytes()[..2]);
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(entry.name);
        }
        let start = out.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::build::{archive, crc32, deflated, stored, Entry};
    use super::*;

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("recipe-zip-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, entries: &[Entry<'_>]) -> std::path::PathBuf {
            let path = self.0.join("a.zip");
            std::fs::write(&path, archive(entries)).unwrap();
            path
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_crc_is_the_zip_one() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn the_directory_lists_members_and_a_member_hashes_to_its_bytes() {
        let temp = Temp::new("list");
        // "hello hello hello " deflated by Python's zlib (a fixed block).
        let fixed = hex("cb48cdc9c957c8402201");
        let dynamic = hex(DYNAMIC);
        assert_eq!(dynamic[0] & 0x06, 0x04, "the fixture is a dynamic block");
        let skewed = skewed();
        let path = temp.write(&[
            stored("DIR/", b""),
            stored("DIR/A.TXT", b"stored bytes"),
            deflated("B.TXT", b"hello hello hello ", &fixed),
            deflated("C.TXT", &skewed, &dynamic),
        ]);
        let members = list(&path).unwrap();
        let names: Vec<(&str, u64, u16)> = members
            .iter()
            .map(|m| (m.name.as_str(), m.size, m.method))
            .collect();
        assert_eq!(
            names,
            [("DIR/A.TXT", 12, 0), ("B.TXT", 18, 8), ("C.TXT", 3000, 8)]
        );
        assert_eq!(members[0].crc32, format!("{:08x}", crc32(b"stored bytes")));
        assert_eq!(members[0].raw_name, "4449522f412e545854");
        assert_eq!(hash(&path, &members[0]).unwrap(), sha256(b"stored bytes"));
        assert_eq!(
            hash(&path, &members[1]).unwrap(),
            sha256(b"hello hello hello ")
        );
        assert_eq!(hash(&path, &members[2]).unwrap(), sha256(&skewed));
        // A directory that claims another CRC is refused.
        let mut wrong = members[1].clone();
        wrong.crc32 = "00000000".into();
        assert!(hash(&path, &wrong).is_err());
        std::fs::write(&path, b"not a zip").unwrap();
        assert!(list(&path).is_err());
    }

    #[test]
    fn broken_deflate_data_is_refused_not_guessed() {
        let temp = Temp::new("broken");
        let cases: [(&[u8], &str); 4] = [
            (&[0x07], "a reserved block type"),
            (
                &[0x01, 0x05, 0x00, 0x00, 0x00],
                "a stored length that fails its check",
            ),
            (
                &[0x01, 0x05, 0x00, 0xfa, 0xff, b'h'],
                "data that ends early",
            ),
            // A fixed block whose first match reaches before the data.
            (&[0x03, 0x02, 0x00], "a distance too far back"),
        ];
        for (data, why) in cases {
            let path = temp.write(&[deflated("X", b"hello", data)]);
            let member = &list(&path).unwrap()[0];
            assert!(hash(&path, member).is_err(), "{why}");
        }
    }

    /// Fixed-Huffman DEFLATE data of one literal and `matches` copies of 258
    /// bytes at distance 1: 13 bits each, so it expands about 160 times.
    fn bomb(matches: usize) -> Vec<u8> {
        let (mut out, mut bits, mut count) = (Vec::new(), 0u32, 0u32);
        let mut put = |value: u32, n: u32, out: &mut Vec<u8>| {
            bits |= value << count;
            count += n;
            while count >= 8 {
                out.push(bits as u8);
                bits >>= 8;
                count -= 8;
            }
        };
        // Huffman codes go most significant bit first.
        let code = |code: u32, n: u32| (0..n).fold(0, |r, i| r | ((code >> i) & 1) << (n - 1 - i));
        put(1, 1, &mut out);
        put(1, 2, &mut out);
        put(code(0x30, 8), 8, &mut out);
        let (length, distance) = (code(0xc5, 8), code(0, 5));
        for _ in 0..matches {
            put(length, 8, &mut out);
            put(distance, 5, &mut out);
        }
        put(code(0, 7), 7, &mut out);
        put(0, 7, &mut out);
        out
    }

    #[test]
    fn a_member_that_expands_past_its_stated_size_stops_at_once() {
        let temp = Temp::new("bomb");
        // About 258 MB of zeros from 1.6 MB, stated as 10 bytes.
        let data = bomb(1_000_000);
        let path = temp.write(&[Entry {
            stated: Some(10),
            ..deflated("BOMB.BIN", &[0; 10], &data)
        }]);
        let member = &list(&path).unwrap()[0];
        assert_eq!(member.size, 10);
        let started = std::time::Instant::now();
        let error = hash(&path, member).unwrap_err();
        let took = started.elapsed();
        assert!(
            error.to_string().contains("expands past the 10 bytes"),
            "{error}"
        );
        assert!(took.as_millis() < 500, "took {took:?}");
    }

    #[test]
    fn zip64_markers_and_ambiguous_names_are_listed_as_problems() {
        let temp = Temp::new("problems");
        let path = temp.write(&[
            Entry {
                stated: Some(0xffff_ffff),
                ..stored("BIG.BIN", b"x")
            },
            stored("TWICE.TXT", b"one"),
            stored("TWICE.TXT", b"two"),
            // Code page 437 bytes, not flagged UTF-8.
            Entry {
                name: b"CAF\x82.TXT",
                ..stored("", b"x")
            },
            // The same bytes flagged UTF-8 are no valid UTF-8 either.
            Entry {
                name: b"CAF\x82.DOC",
                flags: 0x0800,
                ..stored("", b"x")
            },
            Entry {
                name: "CAFÉ.TXT".as_bytes(),
                flags: 0x0800,
                ..stored("", b"x")
            },
        ]);
        let members = list(&path).unwrap();
        let problems: Vec<(&str, Option<&str>)> = members
            .iter()
            .map(|m| (m.raw_name.as_str(), m.problem.as_deref()))
            .collect();
        let zip64 = Some("ZIP64 sizes or offset, which this version does not read");
        let twice = Some("a name two members share: ambiguous");
        let undecodable = Some("a name that is neither ASCII nor flagged UTF-8: ambiguous");
        assert_eq!(
            problems,
            [
                ("4249472e42494e", zip64),
                ("54574943452e545854", twice),
                ("54574943452e545854", twice),
                ("434146822e545854", undecodable),
                ("434146822e444f43", undecodable),
                ("434146c3892e545854", None),
            ]
        );
        assert_eq!(members[5].name, "CAFÉ.TXT");
    }

    /// 3000 letters of a skewed alphabet from a linear congruential
    /// sequence, so that the compressor writes a dynamic block.
    fn skewed() -> Vec<u8> {
        let mut x: u32 = 1;
        (0..3000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345) & 0x7fff_ffff;
                b"aaaaaaaabbbbccde  "[((x >> 16) % 18) as usize]
            })
            .collect()
    }

    /// [`skewed`] deflated by Python's zlib at level 9: one dynamic block.
    const DYNAMIC: &str = concat!(
        "655687ade5300c5b45aba9edbfc2b1c8791f38fc8238b155288a72c5665666eb276b178baa2d2e66",
        "aaa2923f39faab6cec8f4cfd6b6cdad68a46022f4266f0c74378c609dbaf499eaec8c02f4dd342e8",
        "ebc074ece850fb5de92476aeac710b97cd9738ba3032614fe7bd1872cbddc0195d71978e21f07260",
        "f08f876dbea52d664b6bc3556e36f3e5d2a0943c971e72800dbeb4332a18e10288558d605bd86482",
        "4a1ac830e35ada19055a4def00c04197ccf1b5f76f004c1de69a50d11a4025780c78112910a191e0",
        "5740c0d880dca67c2add611d96d0c344940ad40ada91d3c596d164954298b1d24082d18502d8629c",
        "8a1511c8742010c384b4b01f48b23e3043208ae9c1fbd0b2e3b16d1f50e6c70cd3c2df611b18310c",
        "d69e2bc2a5ea141c1a3ac636f8b697e41964a44170040b3f2073a68dc05224265a460509d5a24ee2",
        "73b056e3d0daac3195c68f75a49ef872302cb5667a361225a5587946ce62d26e1851378e6a29a064",
        "5cb5ddaec346e09d0fee1bb3589da6bd2c2ba88bc4b9987ca195e2eb1728732d437251b7621d1c5b",
        "d58a1ccf23b678b2e6709ae8a277990d077019a70927237eb5f948622f922fb52328d132cfa6237d",
        "59bb207b98a1eb68fcce7e1dfb9d34dcb2dc2c4d87ca4b879488becdb16b5c44d97483e00c3eed51",
        "e1a875265162f86a37af854ca0f8dc2365cd1719321055ac55cb70424c20c74de83de510e1eb290e",
        "77b59453d4718349590eace851beed7e6695ea22058a6b0090fa7584b68ddbc8baa91a15fbdeca20",
        "4e9d97d1439cbc10a1b69cd509f00955e6e453caada3df44bf40d4362c65a0dacbd792f49aa7f22b",
        "92321597b3a592bb7d683c099938e71444b37a0552bd30524eb6f20f7b959605d01d29596e76f194",
        "d4b3c55ab77c592f4fedd8771c3937472427f93a945c61fddd83adc63cf9561e808d71af3a90ecdd",
        "fc7d7a82b032487fd0d15293fb9095fb35754b83d8acf53927afa0126a62928304b7ae5056297c95",
        "5fd76af2cd6a0cb444b7ea6974506b42898da9afd054e22e11ecdaae3c3428ccdd1e2b8fe437ca34",
        "e33baed547c2b2a3a7d57827fbd8419a04c25b6d1842374623df151db6e04d61fddbf33327de37da",
        "d3c55b8d67491027a6773cc941f6439e1f4d941ca26cf396f1cedd0dd6e382caf3898408fb247124",
        "2352da35b735d1f2baef1c1c34c7e675a25b2782cd2ed14de7dd5f547a0d490d99d3ea389d548b69",
        "2061c28413bee42a9e64df20198b6fe76fdaf0e8a8287d1cb6e2f14ed4ff8f5131fe80e3887f5719",
        "37527d02eb6b93ae21514fc7f3682502c64f86df84d2cd89398453d40c46addb1526c1ee72040173",
        "ab4a57de854ac2da3700c8055f1c3c7ff29b3de189c4bb98c5d2b3f0dd5f48d2bddb8ac7820aa0bb",
        "d55cebb9342a8174603d263da5e457aa469db5d2f992c6d0b2fb40be1b8d661d83dfb88ab047e99e",
        "8258ca60faca2cff184b04952ac5eebfd6aeef17b52bf6a095b44fbfe6cd208ac478d09a0de3bb81",
        "caf176852a66bb84747cef620bfe03",
    );
}
