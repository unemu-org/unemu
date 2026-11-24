// SPDX-License-Identifier: AGPL-3.0-or-later
//! The typed model of recipe versions 1.0 and 1.1 (recipes §6): a Mediafile or a
//! Firmwarefile, and the work file of recipes §6.16.
//!
//! Every value keeps where it was written ([`At`]), so a later check reports
//! its file position and key path. A value the parser refused is absent
//! (`None`, or missing from its list): the refusal is already a finding,
//! and the checks that follow judge only what was read.

use std::ops::Range;

/// A value with its byte span in the file and its key path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct At<T> {
    pub value: T,
    pub span: Range<usize>,
    /// The key path, `step[1].members[2].path`, indices from 1.
    pub path: String,
}

/// A local date-time of a member or entry: even seconds, years 1980 to
/// 2107, which both a zip entry and a FAT directory entry hold (recipes
/// §6.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// The kind of a recipe, chosen by its identity table (recipes §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `[media]`: an installation media set.
    Media,
    /// `[firmware]`: a firmware package.
    Firmware,
}

impl Kind {
    /// The identity table's key, which is also the kind segment of the
    /// output's repository (`unemu/media/…`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Media => "media",
            Self::Firmware => "firmware",
        }
    }
}

/// A Mediafile or a Firmwarefile.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub kind: Kind,
    /// `recipe` as written, also a version rule 1 refuses.
    pub version: Option<At<String>>,
    /// The work this is a release of; its description is
    /// `<work>.work.toml` beside the recipe.
    pub work: Option<At<String>>,
    pub identity: Identity,
    pub title: Option<Title>,
    pub package: Option<Package>,
    pub license: Option<License>,
    pub needs: Vec<Need>,
    pub ids: Vec<WorkId>,
    pub inputs: Vec<Input>,
    pub steps: Vec<Step>,
    pub media: Vec<Medium>,
    pub roms: Vec<Rom>,
    pub extras: Vec<Extra>,
    pub attachments: Vec<Attachment>,
}

/// `[media]` or `[firmware]` (recipes §6.3).
#[derive(Clone, Debug)]
pub struct Identity {
    /// The table header.
    pub at: At<()>,
    pub name: Option<At<String>>,
    pub platform: Option<At<String>>,
    pub release: Option<At<String>>,
    pub revision: Option<At<i64>>,
    pub digest: Option<At<String>>,
    pub min_app_version: Option<At<String>>,
}

/// The players a title takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Players {
    pub min: i64,
    pub max: i64,
}

/// `[title]` of a Mediafile (recipes §6.4) or of a work file (§6.16), which
/// states the shared fields and `synopsis` only.
#[derive(Clone, Debug)]
pub struct Title {
    pub at: At<()>,
    pub title: Option<At<String>>,
    pub sort_title: Option<At<String>>,
    pub series: Option<At<String>>,
    pub version: Option<At<String>>,
    pub edition: Option<At<String>>,
    pub source_media: Option<At<String>>,
    pub description: Option<At<String>>,
    pub requirements: Option<At<String>>,
    pub synopsis: Option<At<String>>,
    pub year: Option<At<i64>>,
    pub publishers: Vec<At<String>>,
    pub developers: Vec<At<String>>,
    pub genres: Vec<At<String>>,
    pub languages: Vec<At<String>>,
    pub regions: Vec<At<String>>,
    pub software_platform: Option<At<String>>,
    pub players: Option<At<Players>>,
    pub labels: Vec<At<String>>,
}

/// `[package]` of a Firmwarefile (recipes §6.5).
#[derive(Clone, Debug)]
pub struct Package {
    pub at: At<()>,
    pub title: Option<At<String>>,
    pub vendor: Option<At<String>>,
    pub version: Option<At<String>>,
    pub date: Option<At<String>>,
    pub description: Option<At<String>>,
    pub notes: Option<At<String>>,
    pub labels: Vec<At<String>>,
    pub boards: Vec<At<String>>,
    pub machine_classes: Vec<At<String>>,
}

/// `[license]` (recipes §6.6).
#[derive(Clone, Debug)]
pub struct License {
    pub at: At<()>,
    pub expression: Option<At<String>>,
    /// The id of the extra that holds the licence text.
    pub terms: Option<At<String>>,
    /// The id of the extra that holds the corresponding source.
    pub source: Option<At<String>>,
}

/// One kind of `[needs]` and its alternatives, best first (recipes §6.7).
#[derive(Clone, Debug)]
pub struct Need {
    pub kind: At<String>,
    pub values: Vec<At<String>>,
}

/// One key of `[ids]`: a catalogue and the work's id there (recipes §6.8).
#[derive(Clone, Debug)]
pub struct WorkId {
    pub catalogue: At<String>,
    pub id: At<String>,
}

/// `[input.<id>]` (recipes §6.8).
#[derive(Clone, Debug)]
pub struct Input {
    pub id: At<String>,
    pub name: Option<At<String>>,
    pub form: InputForm,
    /// Other names the file or folder is known under: hints for a search.
    pub names: Vec<At<String>>,
    /// Weaker hashes a catalogue carries, lower-case hex: hints for choosing
    /// candidates, never proof.
    pub crc32: Option<At<String>>,
    pub md5: Option<At<String>>,
    pub sha1: Option<At<String>>,
    /// The dump's entries in the catalogues of dumps.
    pub ids: Vec<DbId>,
    /// `https` URLs that serve exactly these bytes, in order.
    pub urls: Vec<At<String>>,
    pub bundle: Option<At<bool>>,
    pub disclosable: Option<At<bool>>,
}

/// One entry of an input's `ids`: a dump in No-Intro, Redump, TOSEC or
/// MAME (recipes §6.8).
#[derive(Clone, Debug)]
pub struct DbId {
    pub db: At<String>,
    pub set: At<String>,
    pub name: At<String>,
}

/// A single file or a file set.
#[derive(Clone, Debug)]
pub enum InputForm {
    File {
        size: Option<At<u64>>,
        sha256: Option<At<String>>,
    },
    FileSet {
        files: Vec<FileEntry>,
    },
    /// A value that is not a table, refused by rule 1: neither form.
    Refused,
}

/// One file of a file set.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: At<String>,
    pub size: Option<At<u64>>,
    pub sha256: Option<At<String>>,
    pub time: Option<At<LocalTime>>,
    pub disclosable: Option<At<bool>>,
}

/// One member of an `unpack` step.
#[derive(Clone, Debug)]
pub struct Member {
    pub path: At<String>,
    pub size: Option<At<u64>>,
    pub sha256: Option<At<String>>,
    pub time: Option<At<LocalTime>>,
    /// `true`: the member is under 256 bytes and its hash may be published
    /// all the same (recipes §6.13), as for an input.
    pub disclosable: Option<At<bool>>,
}

/// A `[[step]]` (recipes §6.9).
#[derive(Clone, Debug)]
pub struct Step {
    pub at: At<()>,
    /// The name of the value the step makes.
    pub id: Option<At<String>>,
    pub op: Op,
}

/// A direct file input stored unchanged in the FAT root directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub from: Option<At<String>>,
    pub dos_name: Option<At<String>>,
    pub time: Option<At<LocalTime>>,
}

/// What a step does.
// A recipe has at most 64 steps; boxing the largest verb would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Op {
    /// Listed members of an archive, as a tree.
    Unpack {
        archive: Option<At<String>>,
        members: Vec<Member>,
        complete: bool,
    },
    /// A FAT volume, or `volumes` of them, from a tree or file placements.
    Fat {
        tree: Option<At<String>>,
        placements: Option<Vec<Placement>>,
        format: Option<At<String>>,
        label: Option<At<String>>,
        serial: Option<At<i64>>,
        order: Option<At<String>>,
        time: Option<At<LocalTime>>,
        volumes: Option<At<i64>>,
    },
    /// Our own text with DOS line ends.
    Text { text: Option<At<String>> },
    /// A step whose verb could not be told (none, or several): the
    /// references its verb keys hold, so that nothing behind them is
    /// reported again.
    Unknown { references: Vec<At<String>> },
}

/// A `[[medium]]` (recipes §6.10).
#[derive(Clone, Debug)]
pub struct Medium {
    pub at: At<()>,
    pub id: Option<At<String>>,
    pub label: Option<At<String>>,
    pub format: Option<At<String>>,
    pub file_name: Option<At<String>>,
    pub from: Option<At<String>>,
}

/// A `[[rom]]` (recipes §6.10).
#[derive(Clone, Debug)]
pub struct Rom {
    pub at: At<()>,
    pub id: Option<At<String>>,
    pub role: Option<At<String>>,
    pub catalogue_id: Option<At<String>>,
    pub source: RomSource,
}

/// Where a ROM's bytes come from.
#[derive(Clone, Debug)]
pub enum RomSource {
    /// One blob; its size and hash are the value's.
    File(Option<At<String>>),
    /// One blob per chip, and the logical ROM they assemble.
    Parts {
        parts: Vec<Part>,
        size: Option<At<i64>>,
        sha256: Option<At<String>>,
    },
}

/// One chip of a ROM.
#[derive(Clone, Debug)]
pub struct Part {
    pub at: At<()>,
    pub position: Option<At<String>>,
    pub part_number: Option<At<String>>,
    pub lane: Option<At<String>>,
    pub offset: Option<At<u64>>,
    pub from: Option<At<String>>,
}

/// An `[[extra]]` or `[[attachment.extra]]` (recipes §6.10).
#[derive(Clone, Debug)]
pub struct Extra {
    pub at: At<()>,
    pub id: Option<At<String>>,
    pub file_name: Option<At<String>>,
    pub kind: Option<At<String>>,
    pub dos_name: Option<At<String>>,
    pub from: Option<At<String>>,
}

/// An `[[attachment]]`: an optional part (recipes §6.10, §3.9).
#[derive(Clone, Debug)]
pub struct Attachment {
    pub at: At<()>,
    pub id: Option<At<String>>,
    pub title: Option<At<String>>,
    pub digest: Option<At<String>>,
    pub extras: Vec<Extra>,
}

/// A work file, `<id>.work.toml` (recipes §6.16).
#[derive(Clone, Debug)]
pub struct WorkFile {
    /// `recipe` as written, also a version rule 1 refuses.
    pub version: Option<At<String>>,
    /// `[work].id`.
    pub id: Option<At<String>>,
    pub title: Option<Title>,
    pub ids: Vec<WorkId>,
    pub needs: Vec<Need>,
}

/// What one file holds.
#[derive(Clone, Debug)]
pub enum Document {
    Recipe(Box<Recipe>),
    Work(Box<WorkFile>),
}
