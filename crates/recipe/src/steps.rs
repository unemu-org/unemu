// SPDX-License-Identifier: AGPL-3.0-or-later
//! The three steps of recipe version 0.1 (recipes §6.9), each a pure
//! function of its operand's bytes and its keys (recipes §3.6, §6.12): it
//! reads no clock, no locale, no host file or name, and opens no
//! connection. A time exists only where the recipe states one.
//!
//! - [`unpack`]: listed members of a zip archive, as a tree in the listed
//!   order with the listed times. Members found elsewhere by their size and
//!   SHA-256 (recipes §3.4, "found instead of made") are taken as found; the
//!   archive is opened only when some member is still missing, and is then
//!   checked whole: every name safe and unambiguous, and each listed
//!   member's stated size, hash and time equal to the archive's.
//! - [`text`]: our own ASCII text, every LF written as CR LF.
//! - [`fat`]: one FAT volume, or exactly N, from a tree, built by the
//!   platform provider's volume profile, which refuses a bad name and never
//!   renames.

use crate::model::LocalTime;
use crate::provider::{VolumeFile, VolumeKeys, VolumeProfile};
use crate::zip::Archive;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Bytes shared between values without copying: an input and the trees and
/// outputs that take it hold one buffer.
pub type Bytes = Arc<Vec<u8>>;

/// A value of a recipe (recipes §3.2): a file, or a tree of files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    File(Bytes),
    Tree(Vec<TreeFile>),
}

/// One file of a tree: a relative path with `/` separators, its bytes and
/// the time the recipe states for it, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeFile {
    pub path: String,
    pub bytes: Bytes,
    pub time: Option<LocalTime>,
}

/// One member an `unpack` step lists, with what the recipe states of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberSpec {
    pub path: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub time: Option<LocalTime>,
}

/// The lower-case hex SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Why a member name of an archive cannot be unpacked, or `None`: the rules
/// of recipes §6.9 (relative, no `..`, no drive, no leading `/`), and no
/// empty or `.` part, no control character and no backslash, which is a
/// separator on some hosts, so that a name means one path and nothing
/// outside the tree.
pub fn unsafe_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("an empty name");
    }
    if name.starts_with('/') {
        return Some("an absolute path");
    }
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Some("a drive letter");
    }
    if name.chars().any(char::is_control) {
        return Some("a control character");
    }
    if name.contains('\\') {
        return Some("a backslash");
    }
    for part in name.split('/') {
        match part {
            "" => return Some("an empty path component"),
            "." => return Some("a `.` path component"),
            ".." => return Some("a `..` path component"),
            _ => {}
        }
    }
    None
}

/// The bytes of a name the directory reader keeps as lower-case hex.
fn raw_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok())
        .collect()
}

/// Why an archive's file member makes the archive one an `unpack` step
/// refuses when it opens it, or `None`: a problem the directory reader
/// found (ZIP64 fields, an ambiguous name), a backslash in the stored name,
/// or a name [`unsafe_name`] refuses.
pub(crate) fn member_problem(member: &crate::zip::Member) -> Option<String> {
    if let Some(problem) = &member.problem {
        return Some(format!("the archive's member {:?}: {problem}", member.name));
    }
    // The directory reader shows a stored backslash as `/`; the stored
    // bytes decide.
    if raw_bytes(&member.raw_name).contains(&b'\\') {
        return Some(format!(
            "the archive's member {:?} has a backslash in its stored name; it is refused, never renamed",
            member.name
        ));
    }
    unsafe_name(&member.name).map(|problem| {
        format!(
            "the archive's member {:?} has {problem}; it is refused, never renamed",
            member.name
        )
    })
}

/// The DOS date and time of a zip entry as a local time, or `None` when the
/// fields hold no valid date (PKWARE APPNOTE §4.4.6, the MS-DOS format): a
/// day the month does not have (the field holds 1 to 31 whatever the
/// month, so 1993-02-30 can be stored) is no date, as it is no TOML date
/// either.
pub(crate) fn dos_time(time: u16, date: u16) -> Option<LocalTime> {
    let local = LocalTime {
        year: 1980 + (date >> 9),
        month: ((date >> 5) & 0x0f) as u8,
        day: (date & 0x1f) as u8,
        hour: (time >> 11) as u8,
        minute: ((time >> 5) & 0x3f) as u8,
        second: ((time & 0x1f) * 2) as u8,
    };
    (image_store::firmware::valid_date(&format!(
        "{:04}-{:02}-{:02}",
        local.year, local.month, local.day
    )) && local.hour < 24
        && local.minute < 60
        && local.second < 60)
        .then_some(local)
}

pub(crate) fn show_time(time: &LocalTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}

/// The `unpack` step: the listed members, in order, each with its stated
/// time. `found` holds the members found elsewhere by their stated size
/// and SHA-256, already verified; `archive` is the archive's bytes, given
/// when a member an output reads is still missing. Without the archive the
/// tree holds the found members only. With `complete`, an opened archive's
/// file members must be exactly the listed ones, in order.
pub fn unpack(
    archive: Option<&[u8]>,
    members: &[MemberSpec],
    found: &BTreeMap<String, Bytes>,
    complete: bool,
) -> Result<Vec<TreeFile>, String> {
    let mut taken: BTreeMap<&str, Bytes> = BTreeMap::new();
    if let Some(bytes) = archive {
        let archive = Archive::open(bytes).map_err(|e| e.to_string())?;
        for entry in &archive.entries {
            if let Some(problem) = member_problem(&entry.member) {
                return Err(problem);
            }
        }
        if complete {
            let listed: Vec<&str> = members.iter().map(|m| m.path.as_str()).collect();
            let held: Vec<&str> = archive
                .entries
                .iter()
                .map(|e| e.member.name.as_str())
                .collect();
            if listed != held {
                return Err(format!(
                    "complete: the archive's file members are {held:?}, not the {} listed, in that order",
                    listed.len()
                ));
            }
        }
        for spec in members {
            let Some(entry) = archive.entries.iter().find(|e| e.member.name == spec.path) else {
                return Err(format!("the archive has no member {:?}", spec.path));
            };
            let member = &entry.member;
            if let Some(size) = spec.size {
                if size != member.size {
                    return Err(format!(
                        "member {:?}: the recipe states {size} bytes, the archive holds {}",
                        spec.path, member.size
                    ));
                }
            }
            if let Some(time) = &spec.time {
                let held = dos_time(entry.time, entry.date);
                if held.as_ref() != Some(time) {
                    return Err(format!(
                        "member {:?}: the recipe states the time {}, the archive {}; a recipe may not claim an archive's times and state others",
                        spec.path,
                        show_time(time),
                        held.as_ref().map_or_else(|| "none".to_owned(), show_time)
                    ));
                }
            }
            let bytes = archive
                .read(member)
                .map_err(|e| format!("member {:?}: {e}", spec.path))?;
            if let Some(sha) = &spec.sha256 {
                let held = sha256(&bytes);
                if &held != sha {
                    return Err(format!(
                        "member {:?}: the recipe states sha256 {sha}, the archive holds {held}",
                        spec.path
                    ));
                }
            }
            taken.insert(
                &spec.path,
                found
                    .get(&spec.path)
                    .cloned()
                    .unwrap_or_else(|| Arc::new(bytes)),
            );
        }
    }
    let opened = archive.is_some();
    let mut tree = Vec::new();
    for spec in members {
        let bytes = taken
            .get(spec.path.as_str())
            .or_else(|| found.get(&spec.path))
            .cloned();
        match bytes {
            Some(bytes) => tree.push(TreeFile {
                path: spec.path.clone(),
                bytes,
                time: spec.time,
            }),
            // Without the archive the tree holds the members found: the
            // plan opens the archive whenever an output reads one that was
            // not, so one left out here is one nothing reads.
            None if !opened => {}
            None => {
                return Err(format!(
                    "member {:?} was neither found nor taken from the archive",
                    spec.path
                ))
            }
        }
    }
    Ok(tree)
}

/// The `text` step with `eol = "crlf"`: ASCII only, every LF becomes CR LF,
/// and a CR already there is refused, so a text is never converted twice.
pub fn text(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 32);
    for (at, &byte) in bytes.iter().enumerate() {
        match byte {
            b'\r' => {
                return Err(format!(
                    "a CR at byte {at}: the operand must have LF line ends only"
                ))
            }
            b'\n' => out.extend_from_slice(b"\r\n"),
            byte if byte.is_ascii() => out.push(byte),
            byte => {
                return Err(format!(
                    "the byte {byte:#04x} at {at} is not ASCII; a text operand is ASCII in this version"
                ))
            }
        }
    }
    Ok(out)
}

/// The `fat` step: the volume profile builds the tree into one image, a
/// file, or into exactly `keys.volumes`, a tree of the members `1` to `N`.
pub fn fat(
    tree: &[TreeFile],
    keys: &VolumeKeys,
    profile: &dyn VolumeProfile,
) -> Result<Value, String> {
    let files: Vec<VolumeFile<'_>> = tree
        .iter()
        .map(|file| VolumeFile {
            path: &file.path,
            bytes: &file.bytes,
            time: file.time,
        })
        .collect();
    let mut images = profile.build(&files, keys)?;
    match keys.volumes {
        None if images.len() == 1 => Ok(Value::File(Arc::new(images.remove(0)))),
        None => Err(format!(
            "the volume profile built {} volumes, not one",
            images.len()
        )),
        Some(count) if images.len() == count => Ok(Value::Tree(
            images
                .into_iter()
                .enumerate()
                .map(|(index, image)| TreeFile {
                    path: (index + 1).to_string(),
                    bytes: Arc::new(image),
                    time: None,
                })
                .collect(),
        )),
        Some(count) => Err(format!(
            "the volume profile built {} volumes, not the {count} stated",
            images.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::build::{archive, stored, Entry};

    fn bytes(b: &[u8]) -> Bytes {
        Arc::new(b.to_vec())
    }

    fn spec(path: &str) -> MemberSpec {
        MemberSpec {
            path: path.into(),
            size: None,
            sha256: None,
            time: None,
        }
    }

    #[test]
    fn text_writes_dos_line_ends_and_refuses_cr_and_non_ascii() {
        assert_eq!(text(b"a\nb\n\nc").unwrap(), b"a\r\nb\r\n\r\nc");
        assert_eq!(text(b"").unwrap(), b"");
        assert!(text(b"a\r\nb").unwrap_err().contains("a CR at byte 1"));
        assert!(text("caf\u{e9}\n".as_bytes())
            .unwrap_err()
            .contains("is not ASCII"));
    }

    #[test]
    fn unpack_takes_listed_members_in_listed_order_with_stated_times() {
        let zip = archive(&[
            stored("B.TXT", b"bee"),
            stored("A.TXT", b"a"),
            stored("UNUSED", b"x"),
        ]);
        let time = LocalTime {
            year: 1980,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        };
        let tree = unpack(
            Some(&zip),
            &[spec("A.TXT"), spec("B.TXT")],
            &BTreeMap::new(),
            false,
        )
        .unwrap();
        assert_eq!(
            tree.iter()
                .map(|f| (f.path.as_str(), f.bytes.as_slice(), f.time))
                .collect::<Vec<_>>(),
            [("A.TXT", &b"a"[..], None), ("B.TXT", &b"bee"[..], None)]
        );
        // The test archives state 1980-01-01 00:00:00 for every member: a
        // listed time equal to it is taken, another is refused.
        let mut timed = spec("A.TXT");
        timed.time = Some(time);
        let tree = unpack(Some(&zip), &[timed.clone()], &BTreeMap::new(), false).unwrap();
        assert_eq!(tree[0].time, Some(time));
        timed.time = Some(LocalTime { year: 1999, ..time });
        let error = unpack(Some(&zip), &[timed], &BTreeMap::new(), false).unwrap_err();
        assert!(error.contains("a recipe may not claim"), "{error}");
        // With `complete`, an unlisted member is refused.
        let error = unpack(
            Some(&zip),
            &[spec("A.TXT"), spec("B.TXT")],
            &BTreeMap::new(),
            true,
        )
        .unwrap_err();
        assert!(error.starts_with("complete:"), "{error}");
        // A stated size or hash the archive does not hold is refused.
        let mut sized = spec("A.TXT");
        sized.size = Some(2);
        assert!(unpack(Some(&zip), &[sized], &BTreeMap::new(), false)
            .unwrap_err()
            .contains("states 2 bytes"));
        let mut hashed = spec("A.TXT");
        hashed.sha256 = Some(sha256(b"b"));
        assert!(unpack(Some(&zip), &[hashed], &BTreeMap::new(), false)
            .unwrap_err()
            .contains("states sha256"));
        // A member the archive lacks.
        assert!(
            unpack(Some(&zip), &[spec("C.TXT")], &BTreeMap::new(), false)
                .unwrap_err()
                .contains("no member \"C.TXT\"")
        );
    }

    #[test]
    fn found_members_need_no_archive() {
        let found = BTreeMap::from([("X/A.BIN".to_owned(), bytes(b"found"))]);
        let tree = unpack(None, &[spec("X/A.BIN"), spec("X/B.BIN")], &found, true).unwrap();
        assert_eq!(tree.len(), 1, "a member not found and not read is left out");
        assert_eq!(tree[0].bytes.as_slice(), b"found");
    }

    #[test]
    fn unsafe_and_ambiguous_member_names_are_refused_never_guessed() {
        for (name, why) in [
            ("../ESCAPE.TXT", "a `..` path component"),
            ("A/../../B", "a `..` path component"),
            ("/ETC/PASSWD", "an absolute path"),
            ("C:/AUTOEXEC.BAT", "a drive letter"),
            ("A//B", "an empty path component"),
            ("./A", "a `.` path component"),
            ("..\\ESCAPE.TXT", "a backslash"),
        ] {
            assert_eq!(unsafe_name(name), Some(why), "{name}");
            let zip = archive(&[stored("OK.TXT", b"ok"), stored(name, b"x")]);
            let error = unpack(Some(&zip), &[spec("OK.TXT")], &BTreeMap::new(), false).unwrap_err();
            assert!(error.contains(why), "{name}: {error}");
        }
        assert_eq!(unsafe_name("LEVEL1/GAME.COM"), None);
        // A stored backslash, which the directory reader shows as `/`, is
        // refused even where the recipe lists the `/` spelling.
        let zip = archive(&[stored("LEVEL1\\GAME.COM", b"x")]);
        let error = unpack(
            Some(&zip),
            &[spec("LEVEL1/GAME.COM")],
            &BTreeMap::new(),
            false,
        )
        .unwrap_err();
        assert!(error.contains("a backslash in its stored name"), "{error}");
        // Two members of one name: which one is meant cannot be told.
        let zip = archive(&[stored("A.TXT", b"one"), stored("A.TXT", b"two")]);
        assert!(
            unpack(Some(&zip), &[spec("A.TXT")], &BTreeMap::new(), false)
                .unwrap_err()
                .contains("ambiguous")
        );
        // A name neither ASCII nor flagged UTF-8.
        let zip = archive(&[Entry {
            name: b"CAF\x82.TXT",
            ..stored("", b"x")
        }]);
        assert!(
            unpack(Some(&zip), &[spec("CAF.TXT")], &BTreeMap::new(), false)
                .unwrap_err()
                .contains("ambiguous")
        );
    }

    #[test]
    fn the_dos_time_of_an_entry_reads_as_its_fields() {
        // 1993-03-27 13:58:22: date (13 << 9) | (3 << 5) | 27, time
        // (13 << 11) | (58 << 5) | 11.
        let date = (13 << 9) | (3 << 5) | 27;
        let time = (13 << 11) | (58 << 5) | 11;
        assert_eq!(
            dos_time(time, date),
            Some(LocalTime {
                year: 1993,
                month: 3,
                day: 27,
                hour: 13,
                minute: 58,
                second: 22
            })
        );
        assert_eq!(dos_time(0, 0), None);
        // The day field holds 1 to 31 whatever the month: a day the month
        // lacks is no date, a leap day is one in a leap year only.
        let day = |year: u16, month: u16, day: u16| ((year - 1980) << 9) | (month << 5) | day;
        assert_eq!(dos_time(0, day(1993, 2, 30)), None);
        assert_eq!(dos_time(0, day(1993, 4, 31)), None);
        assert_eq!(dos_time(0, day(1993, 2, 29)), None);
        assert_eq!(dos_time(0, day(1992, 2, 29)).map(|t| t.day), Some(29));
        assert_eq!(dos_time(0, day(2000, 2, 29)).map(|t| t.day), Some(29));
    }

    #[test]
    fn unpack_reads_a_member_with_an_impossible_date_as_having_no_time() {
        let zip = archive(&[Entry {
            time: 10 << 11,
            date: (13 << 9) | (2 << 5) | 30,
            ..stored("A.TXT", b"a")
        }]);
        // No stated time is the archive's: taken.
        let tree = unpack(Some(&zip), &[spec("A.TXT")], &BTreeMap::new(), true).unwrap();
        assert_eq!(tree[0].time, None);
        // Any stated time differs from the archive's none: refused.
        let mut timed = spec("A.TXT");
        timed.time = dos_time(10 << 11, (13 << 9) | (2 << 5) | 28);
        let error = unpack(Some(&zip), &[timed], &BTreeMap::new(), true).unwrap_err();
        assert!(error.contains("the archive none"), "{error}");
    }
}
