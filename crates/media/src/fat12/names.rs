// SPDX-License-Identifier: AGPL-3.0-or-later
//! DOS 8.3 short names for host names; Microsoft FAT Specification, FAT
//! Directory Structure (short name field) and FAT Long Directory Entries (the
//! basis-name and numeric-tail generation algorithms).

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;

/// Punctuation DOS accepts in a short name besides letters and digits.
const PUNCTUATION: &[u8] = b"!#$%&'()-@^_`{}~";

/// For each character from U+00C0 to U+017F (Latin-1 Supplement letters and
/// Latin Extended-A), the upper-case ASCII letter its canonical decomposition
/// (Unicode NFD) starts with when only combining marks follow, else `_` (Æ, Ø,
/// Ł and the like have no decomposition). Derived from the Unicode Character
/// Database decomposition mappings.
const LATIN_BASE: &[u8; 192] = b"AAAAAA_CEEEEIIII_NOOOOO__UUUUY__\
AAAAAA_CEEEEIIII_NOOOOO__UUUUY_Y\
AAAAAACCCCCCCCDD__EEEEEEEEEEGGGG\
GGGGHH__IIIIIIIII___JJKK_LLLLLL_\
___NNNNNN___OOOOOO__RRRRRRSSSSSS\
SSTTTT__UUUUUUUUUUUUWWYYYZZZZZZ_";

/// Combining Diacritical Marks, which canonical decomposition splits off a
/// precomposed letter and which a short name drops.
fn combining(c: &char) -> bool {
    ('\u{300}'..='\u{36F}').contains(c)
}

/// A DOS 8.3 name as a directory entry stores it: an 8-byte stem and a 3-byte
/// extension, upper case, space padded.
///
/// Host names convert like the specification's basis-name generation, with
/// these differences: spaces become `_` instead of disappearing, periods
/// before the last one are dropped from the stem as Windows does, a lossy or
/// shortened name gets a numeric tail (`~1`, `~2`, …) only when it collides
/// with a sibling, and a name already valid as 8.3 keeps it. Letters are
/// upper-cased. A Latin letter with diacritics becomes the letter its
/// canonical decomposition starts with and combining marks are dropped, so a
/// name stored precomposed (NFC) or decomposed (NFD) by the host converts the
/// same way (`Äbc.txt` is `ABC.TXT`). Any other character DOS forbids, and any
/// other non-ASCII character, becomes `_`. A stem naming a DOS device (`CON`,
/// `NUL`, `COM1`…) gains a trailing `_`, because DOS would open the device
/// instead of the file.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShortName([u8; 11]);

impl ShortName {
    /// A name typed by a person, such as a top folder: it must already be a
    /// valid 8.3 name apart from letter case, which is upper-cased as DOS
    /// does. Nothing else is converted.
    pub fn parse(text: &str) -> Option<Self> {
        Self::strict(&text.to_ascii_uppercase()).ok()
    }

    /// A name exactly as a directory entry stores it, saying which rule a
    /// refused name breaks: a stem of one to eight characters, an optional
    /// extension of one to three after a single period, only upper-case
    /// letters, digits and `!#$%&'()-@^_`{}~`, and no DOS device name.
    /// Nothing is converted, not even letter case.
    pub fn strict(text: &str) -> Result<Self, NameFault> {
        if text.is_empty() {
            return Err(NameFault::Empty);
        }
        if let Some(bad) = text.chars().find(|&c| {
            c != '.'
                && !(c.is_ascii()
                    && (c.is_ascii_alphanumeric() || PUNCTUATION.contains(&(c as u8))))
        }) {
            return Err(NameFault::Character(bad));
        }
        if let Some(lower) = text.chars().find(char::is_ascii_lowercase) {
            return Err(NameFault::LowerCase(lower));
        }
        let (stem, extension) = text.split_once('.').unwrap_or((text, ""));
        if stem.is_empty()
            || extension.contains('.')
            || (text.contains('.') && extension.is_empty())
        {
            return Err(NameFault::Periods);
        }
        if stem.len() > 8 {
            return Err(NameFault::Stem(stem.len()));
        }
        if extension.len() > 3 {
            return Err(NameFault::Extension(extension.len()));
        }
        if reserved(stem.as_bytes()) {
            return Err(NameFault::Device);
        }
        Ok(Self::from_parts(stem.as_bytes(), extension.as_bytes()))
    }

    /// The name a host file or folder converts to when nothing beside it
    /// can clash, so it needs no numeric tail: `nightfall` is `NIGHTFAL`.
    pub fn from_host(name: &OsStr) -> Self {
        let basis = basis(name);
        Self::from_parts(&basis.stem, &basis.extension)
    }

    fn from_parts(stem: &[u8], extension: &[u8]) -> Self {
        let mut name = [b' '; 11];
        name[..stem.len()].copy_from_slice(stem);
        name[8..8 + extension.len()].copy_from_slice(extension);
        Self(name)
    }

    /// The 11 bytes of the directory entry's name field.
    pub fn as_bytes(&self) -> &[u8; 11] {
        &self.0
    }
}

/// Why a name is not a valid 8.3 name as it stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameFault {
    /// Nothing between two separators, or nothing at all.
    Empty,
    /// A character DOS does not allow in a short name.
    Character(char),
    /// A lower-case letter: a directory entry stores upper case, and the
    /// name is wanted as it is stored.
    LowerCase(char),
    /// A period that starts or ends the name, or more than one.
    Periods,
    /// A stem of this many characters, more than eight.
    Stem(usize),
    /// An extension of this many characters, more than three.
    Extension(usize),
    /// A DOS device (`CON`, `NUL`, `COM1`…), which DOS would open instead.
    Device,
}

impl fmt::Display for NameFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the name is empty"),
            Self::Character(c) => write!(f, "the character {c:?} is not allowed"),
            Self::LowerCase(c) => write!(f, "the lower-case letter {c:?} is not allowed: a directory entry stores upper case, so write the name as it is on the disk"),
            Self::Periods => {
                f.write_str("a name has at most one period, between a stem and an extension")
            }
            Self::Stem(length) => write!(
                f,
                "the name has {length} characters before the extension; at most 8 are allowed"
            ),
            Self::Extension(length) => write!(
                f,
                "the extension has {length} characters; at most 3 are allowed"
            ),
            Self::Device => f.write_str("it names a DOS device"),
        }
    }
}

impl fmt::Display for ShortName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stem = self.0[..8].trim_ascii_end();
        let extension = self.0[8..].trim_ascii_end();
        // Every byte is printable ASCII by construction.
        f.write_str(&String::from_utf8_lossy(stem))?;
        if !extension.is_empty() {
            write!(f, ".{}", String::from_utf8_lossy(extension))?;
        }
        Ok(())
    }
}

impl fmt::Debug for ShortName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShortName({self})")
    }
}

/// DOS opens these devices whatever the extension.
fn reserved(stem: &[u8]) -> bool {
    matches!(stem, b"CON" | b"PRN" | b"AUX" | b"NUL" | b"CLOCK$")
        || (stem.len() == 4
            && (stem.starts_with(b"COM") || stem.starts_with(b"LPT"))
            && matches!(stem[3], b'1'..=b'9'))
}

struct Basis {
    stem: Vec<u8>,
    extension: Vec<u8>,
    /// The host name is this 8.3 name apart from letter case.
    exact: bool,
}

fn basis(host: &OsStr) -> Basis {
    let text = host.to_string_lossy();
    let mapped: Vec<u8> = text
        .chars()
        .filter(|c| !combining(c))
        .map(|c| match c {
            '.' => b'.',
            c if c.is_ascii_alphanumeric()
                || (c.is_ascii() && PUNCTUATION.contains(&(c as u8))) =>
            {
                c.to_ascii_uppercase() as u8
            }
            '\u{C0}'..='\u{17F}' => LATIN_BASE[c as usize - 0xC0],
            _ => b'_',
        })
        .collect();
    let start = mapped
        .iter()
        .position(|&b| b != b'.')
        .unwrap_or(mapped.len());
    let trimmed = &mapped[start..];
    let (stem, extension) = match trimmed.iter().rposition(|&b| b == b'.') {
        Some(dot) => (&trimmed[..dot], &trimmed[dot + 1..]),
        None => (trimmed, &[][..]),
    };
    let mut stem: Vec<u8> = stem
        .iter()
        .copied()
        .filter(|&b| b != b'.')
        .take(8)
        .collect();
    let extension: Vec<u8> = extension.iter().copied().take(3).collect();
    if stem.is_empty() {
        stem.push(b'_');
    }
    if reserved(&stem) {
        stem.push(b'_');
    }
    let mut shown = stem.clone();
    if !extension.is_empty() {
        shown.push(b'.');
        shown.extend_from_slice(&extension);
    }
    Basis {
        exact: shown.eq_ignore_ascii_case(text.as_bytes()),
        stem,
        extension,
    }
}

/// The stem shortened to make room for `~n`, or `None` when the tail alone
/// would fill the eight characters.
fn with_tail(stem: &[u8], n: u32) -> Option<Vec<u8>> {
    let tail = format!("~{n}");
    if tail.len() >= 8 {
        return None;
    }
    let mut name = stem[..stem.len().min(8 - tail.len())].to_vec();
    name.extend_from_slice(tail.as_bytes());
    Some(name)
}

/// Short names for the members of one folder, given in sorted order. Each
/// result says whether the name differs from the host name beyond letter
/// case. Exact names are reserved first so a converted name never takes one;
/// the rest take their basis name if free, else the first free numeric tail,
/// in the given order. `Err(index)` names a member left without a name.
pub(super) fn assign(hosts: &[&OsStr]) -> Result<Vec<(ShortName, bool)>, usize> {
    let bases: Vec<Basis> = hosts.iter().map(|host| basis(host)).collect();
    let mut taken = HashSet::new();
    let mut names: Vec<Option<(ShortName, bool)>> = vec![None; bases.len()];
    for (slot, basis) in names.iter_mut().zip(&bases) {
        let name = ShortName::from_parts(&basis.stem, &basis.extension);
        if basis.exact && taken.insert(name) {
            *slot = Some((name, false));
        }
    }
    // Resume each basis name's tail search where its previous collision
    // stopped, so a folder of many similar names stays linear.
    let mut next_tail: HashMap<[u8; 11], u32> = HashMap::new();
    for (index, (slot, basis)) in names.iter_mut().zip(&bases).enumerate() {
        if slot.is_some() {
            continue;
        }
        let plain = ShortName::from_parts(&basis.stem, &basis.extension);
        if taken.insert(plain) {
            *slot = Some((plain, true));
            continue;
        }
        let counter = next_tail.entry(plain.0).or_insert(1);
        loop {
            let stem = with_tail(&basis.stem, *counter).ok_or(index)?;
            *counter += 1;
            let name = ShortName::from_parts(&stem, &basis.extension);
            if taken.insert(name) {
                *slot = Some((name, true));
                break;
            }
        }
    }
    Ok(names.into_iter().flatten().collect())
}
