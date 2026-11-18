// SPDX-License-Identifier: AGPL-3.0-or-later
//! The one short name a disk has wherever a drive names it: the drive chip,
//! the Media menu's drive line, This machine in the picker and the Catalogue
//! submenu; machine-images §7. The submenu itself stays with the desktop,
//! which re-exports these.

use crate::media_picker::SetEntry;

/// A disk of an installation media set: what its short name is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetDisk<'a> {
    pub title: &'a str,
    /// The disk's place in the set, from 1.
    pub number: usize,
    pub count: usize,
    pub label: &'a str,
}

/// The disk `medium` of `set`, when the set has it. The set's title leaves
/// out its edition in parentheses, as the picker sets the two apart.
pub fn set_disk<'a>(set: &'a SetEntry, medium: &str) -> Option<SetDisk<'a>> {
    let at = set.disks.iter().position(|disk| disk.id == medium)?;
    Some(SetDisk {
        title: crate::media_picker::title_parts(&set.title).0,
        number: at + 1,
        count: set.disks.len(),
        label: &set.disks[at].label,
    })
}

/// A disk's short name. A disk of a set is the set's title and its place,
/// "AZURAMON 3 of 10", then its own name after a dash when that says more
/// than its number, "FreeDOS 1 of 2 — Boot disk"; a title that ends in a
/// number takes a middle dot, so "Win 3.1 · 1 of 7" never reads as a disk
/// numbered 3.11. A one-disk set is its title alone. Any other disk, such
/// as an image file chosen with Browse…, keeps `name`.
pub fn short_name(disk: Option<SetDisk<'_>>, name: &str) -> String {
    let Some(disk) = disk else {
        return name.to_owned();
    };
    let title = disk.title;
    if disk.count <= 1 {
        return title.to_owned();
    }
    let separator = if title.ends_with(|c: char| c.is_ascii_digit()) {
        " · "
    } else {
        " "
    };
    let mut short = format!("{title}{separator}{} of {}", disk.number, disk.count);
    if let Some(own) = own_name(disk.label, disk.number) {
        short.push_str(" — ");
        short.push_str(&own);
    }
    short
}

/// What the label of disk `number` names it, without what only repeats its
/// place: "Disk 1 (Setup)" is "Setup", "Disk 1 of 8 - packages
/// (x86DSK01.img)" is "Packages", and "Disk 2", "Disk 2 of 7", "Disk #2",
/// "Diskette 2", "Disk2" or "2/7" of disk 2 name nothing more. A file name in
/// parentheses at the end is no name of the disk's own; a number that is not
/// the disk's place stays, as it says something.
pub fn own_name(label: &str, number: usize) -> Option<String> {
    let (label, _) = crate::media_picker::label_without_file(label);
    let label = label.trim();
    let Some(rest) = after_place(label, number) else {
        return (!label.is_empty()).then(|| label.to_owned());
    };
    let mut rest = rest
        .trim_start_matches(|c: char| c.is_whitespace() || "-–—:,".contains(c))
        .trim_end();
    if let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        rest = inner.trim();
    }
    if rest.is_empty() {
        return None;
    }
    // What follows a place in lower case starts with a capital.
    let first = rest.split_whitespace().next().unwrap_or_default();
    if first.chars().all(|c| c.is_ascii_lowercase()) {
        let mut chars = rest.chars();
        return chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str());
    }
    Some(rest.to_owned())
}

/// `label` after a leading place of disk `number`: "Disk 2", "Disk #2",
/// "Diskette 2" or "Disk2", each with " of 7" or "/7" after it, or a bare
/// "2 of 7" or "2/7". `None` when the label does not start with that place;
/// a bare number without a count, as "2 Players", is a name.
fn after_place(label: &str, number: usize) -> Option<&str> {
    let mut rest = label;
    let word = ["diskette", "disk"].into_iter().find_map(|word| {
        let head = rest.get(..word.len())?;
        head.eq_ignore_ascii_case(word).then(|| &rest[word.len()..])
    });
    if let Some(after) = word {
        let after = after.trim_start();
        rest = after.strip_prefix('#').unwrap_or(after).trim_start();
    }
    let (place, after) = leading_number(rest)?;
    if place != number {
        return None;
    }
    rest = after;
    let tail = rest.trim_start();
    let count = tail
        .get(..2)
        .filter(|of| of.eq_ignore_ascii_case("of") && tail[2..].starts_with(' '))
        .map(|_| tail[2..].trim_start())
        .or_else(|| tail.strip_prefix('/').map(str::trim_start))
        .and_then(leading_number);
    match count {
        Some((_, after)) => rest = after,
        None if word.is_none() => return None,
        None => {}
    }
    (!rest.starts_with(|c: char| c.is_alphanumeric())).then_some(rest)
}

/// The decimal number `text` starts with, and what follows it.
fn leading_number(text: &str) -> Option<(usize, &str)> {
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let number = text.get(..digits)?.parse().ok()?;
    Some((number, &text[digits..]))
}
