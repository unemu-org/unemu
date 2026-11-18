// SPDX-License-Identifier: AGPL-3.0-or-later
//! The model half of the media picker: the installation media sets of the
//! library and the disks already in a drive's catalogue, as the picker, the
//! drive chips and the kept catalogue name them; machine-images §5.2 and §7.
//! The picker's state and view stay with the desktop, which re-exports these.

use assembly::FloppyGeometry;
use image_store::{Descriptor, Digest};

/// One disk of a set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    pub id: String,
    pub label: String,
    /// `None` for an optical medium.
    pub format: Option<FloppyGeometry>,
}

/// An installation media set of this library, as the picker lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetEntry {
    pub reference: String,
    pub image: Descriptor,
    pub title: String,
    pub publisher: String,
    pub year: Option<i64>,
    pub genre: String,
    pub platform: String,
    pub labels: Vec<String>,
    pub disks: Vec<Disk>,
    pub extras: usize,
    pub open: bool,
    /// Why no drive takes the set: damaged, optical or of an unknown format.
    pub problem: Option<String>,
}

pub fn format_name(format: FloppyGeometry) -> String {
    crate::media_dialog::geometry_name(format)
}

impl SetEntry {
    /// The format of the set's disks, as "2 × 1.2M" names it.
    pub fn format(&self) -> Option<FloppyGeometry> {
        self.disks.iter().find_map(|disk| disk.format)
    }
    pub fn disks_label(&self) -> String {
        format!(
            "{} × {}",
            self.disks.len(),
            self.format().map_or_else(|| "CD".to_owned(), format_name)
        )
    }
    /// Whether a drive of this type takes every disk of the set.
    pub fn fits(&self, drive: FloppyGeometry) -> bool {
        let accepted = drive.accepted_media();
        self.problem.is_none()
            && !self.disks.is_empty()
            && self
                .disks
                .iter()
                .all(|disk| disk.format.is_some_and(|f| accepted.contains(&f)))
    }
    pub fn words(&self) -> Vec<String> {
        format!(
            "{} {} {} {} {} {} {}",
            self.title,
            self.publisher,
            self.genre,
            self.platform,
            self.year.map(|y| y.to_string()).unwrap_or_default(),
            self.labels.join(" "),
            self.reference
        )
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '.'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
    }
    /// The facet values of the set: Format, Platform, Genre, Year,
    /// Publisher and Labels.
    pub fn facet(&self, name: &str) -> Vec<String> {
        match name {
            "Format" => self.format().map(format_name).into_iter().collect(),
            "Platform" => vec![self.platform.clone()],
            "Genre" => vec![self.genre.clone()],
            "Year" => self.year.map(|y| y.to_string()).into_iter().collect(),
            "Publisher" => vec![self.publisher.clone()],
            "Labels" => self.labels.clone(),
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
    }
}

/// A disk already in a drive's catalogue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueDisk {
    pub name: String,
    pub format: FloppyGeometry,
    /// How writes go, in plain words.
    pub write: String,
    pub inserted: bool,
    /// Its place in the drive's catalogue.
    pub index: usize,
    /// The set and the disk's position in it, for a library disk.
    pub set: Option<(Digest, usize)>,
}

pub fn drive_name(unit: u8) -> String {
    format!("{}:", char::from(b'A' + unit))
}

/// A title and the edition in parentheses after it, as the mock-up sets
/// them apart: "FreeDOS 1.4 (Floppy Edition, 1.2M)" is the title
/// "FreeDOS 1.4" and the edition "(Floppy Edition, 1.2M)". The edition is
/// the outermost parenthesis that ends the title.
pub fn title_parts(title: &str) -> (&str, &str) {
    if !title.ends_with(')') {
        return (title, "");
    }
    let mut depth = 0usize;
    for (at, c) in title.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    let head = title[..at].trim_end();
                    return if head.is_empty() || head.len() == at {
                        (title, "")
                    } else {
                        (head, &title[at..])
                    };
                }
            }
            _ => (),
        }
    }
    (title, "")
}

/// Whether `name` is a file name: a stem, a dot and an extension of one to
/// four letters and digits that starts with a letter, with no spaces.
fn is_file_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, extension)) => {
            !stem.is_empty()
                && !name.contains(char::is_whitespace)
                && (1..=4).contains(&extension.len())
                && extension.chars().all(|c| c.is_ascii_alphanumeric())
                && extension.starts_with(|c: char| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// A disk's label without a file name in parentheses at its end, and that
/// file name: "Disk 1 of 8 - packages (x86DSK01.img)" is "Disk 1 of 8 -
/// packages" from "x86DSK01.img". Other parentheses, as "(1.2M)" or
/// "(v2.0)", stay in the label.
pub fn label_without_file(label: &str) -> (&str, Option<&str>) {
    if let Some(rest) = label.strip_suffix(')') {
        if let Some(at) = rest.rfind(" (") {
            let file = &rest[at + 2..];
            if at > 0 && is_file_name(file) {
                return (&label[..at], Some(file));
            }
        }
    }
    (label, None)
}
