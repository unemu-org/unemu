// SPDX-License-Identifier: AGPL-3.0-or-later
//! The keys of recipe version 0.1 (recipes §6.2 to §6.10, §6.16): every
//! table, its keys, their types and which are required.
//!
//! This one description drives both the parser's checks of rule 1 (an
//! unknown key, a missing one, a type or a value outside its form) and the
//! JSON Schema ([`crate::schema`]), so the two cannot disagree. Values whose
//! vocabulary the platform provider owns are [`Ty::Vocab`]: the parser takes
//! any string there and rule 7 asks the provider.

/// The form of a string value. Each has a check here and a JSON Schema
/// pattern in [`crate::schema`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    /// Not blank; line breaks allowed, no other control character.
    Text,
    /// One line: not blank, no control character.
    Line,
    /// One line of at most 128 characters.
    Line128,
    /// `[a-z0-9][a-z0-9._-]{0,63}`, the store's media-id grammar.
    Id,
    /// A value name, optionally followed by `/` and a member path.
    Ref,
    /// 64 lower-case hexadecimal digits.
    Sha256,
    /// 40 lower-case hexadecimal digits.
    Sha1,
    /// 32 lower-case hexadecimal digits.
    Md5,
    /// 8 lower-case hexadecimal digits.
    Crc32,
    /// `sha256:` and 64 lower-case hexadecimal digits.
    Digest,
    /// An OCI repository of four components: `unemu/media/ibm-pc/freedos`.
    Repository,
    /// A platform id: lower-case letters, digits and `-`, `_`, `.` after the
    /// first character, at most 64.
    Platform,
    /// A release, the tag stem of recipes §8.
    Release,
    /// `X.Y.Z`, three decimal numbers.
    Version,
    /// An `https` URL without white space.
    Url,
    /// A file name: one line of at most 128 characters, without `/` or `\`,
    /// not `.` or `..`.
    FileName,
    /// A relative path with `/` separators: no empty, `.` or `..` part, no
    /// `\`, no drive.
    Path,
    /// A calendar date, `YYYY-MM-DD`, that exists.
    Date,
    /// An SPDX licence expression; rule 6 parses it.
    Spdx,
}

/// The type of a value.
#[derive(Debug)]
pub enum Ty {
    Str(Text),
    /// One of these strings.
    Enum(&'static [&'static str]),
    /// A string from the platform provider's vocabulary (rule 7).
    Vocab(Vocab),
    /// An integer from `min` to `max`.
    Int {
        min: i64,
        max: i64,
    },
    Bool,
    /// A local date-time with even seconds and a year from 1980 to 2107.
    Time,
    /// An array of values of one type, with at least `min` items.
    Array {
        item: &'static Ty,
        min: usize,
    },
    Table(&'static Table),
    /// A table of one of these variants; [`Table::marker`] says which.
    OneOf(&'static [&'static Table]),
    /// A table with keys of a form and values of one type.
    Map {
        key: MapKey,
        value: &'static Ty,
    },
    /// A need's alternatives: one string or a non-empty array of strings.
    Needs,
}

/// The form of a map's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapKey {
    /// An [`Text::Id`].
    Id,
    /// A catalogue name or need kind: lower-case letters, digits and `_`,
    /// starting with a letter, at most 32.
    Kind,
}

/// A vocabulary the platform provider owns (recipes §10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vocab {
    /// A media format of a medium.
    MediaFormat,
    /// A media format with a volume profile, for a `fat` step.
    VolumeFormat,
    RomRole,
    ChipLane,
    Board,
    MachineClass,
    SoftwarePlatform,
    /// An extra's DOS name, which the provider's name rule judges.
    DosName,
}

/// A table and its keys.
#[derive(Debug)]
pub struct Table {
    /// The name of the table's JSON Schema definition.
    pub name: &'static str,
    /// For a variant of [`Ty::OneOf`], the key whose presence selects it;
    /// the last variant is taken when no marker is present.
    pub marker: Option<&'static str>,
    pub fields: &'static [Field],
}

/// A key of a table.
#[derive(Debug)]
pub struct Field {
    pub key: &'static str,
    pub ty: Ty,
    pub required: bool,
}

impl Table {
    pub fn field(&self, key: &str) -> Option<&'static Field> {
        self.fields.iter().find(|f| f.key == key)
    }

    pub fn keys(&self) -> Vec<&'static str> {
        self.fields.iter().map(|f| f.key).collect()
    }
}

const fn req(key: &'static str, ty: Ty) -> Field {
    Field {
        key,
        ty,
        required: true,
    }
}

const fn opt(key: &'static str, ty: Ty) -> Field {
    Field {
        key,
        ty,
        required: false,
    }
}

const fn table(name: &'static str, fields: &'static [Field]) -> Table {
    Table {
        name,
        marker: None,
        fields,
    }
}

/// The largest integer a size may be: the store keeps sizes as `i64`.
const MAX: i64 = i64::MAX;
const SIZE: Ty = Ty::Int { min: 0, max: MAX };
const LINE: Ty = Ty::Str(Text::Line);
const LINES: Ty = Ty::Array {
    item: &LINE,
    min: 0,
};
/// A recipe's credits and genres: a list it states replaces its work
/// file's whole, so an empty one could only mean "none", which metadata
/// cannot hold (absent is unknown there); it is refused, and leaving the key
/// out keeps the work file's list (recipes §6.16).
const CREDITS: Ty = Ty::Array {
    item: &LINE,
    min: 1,
};
const LABELS: Ty = Ty::Array {
    item: &Ty::Str(Text::Id),
    min: 0,
};

/// The longest release: its tag `<release>-r<N>` fits the 128 characters
/// of an OCI tag with `-r` and the 19 digits of the largest revision.
pub const MAX_RELEASE: usize = 128 - 2 - 19;

/// The recipe version this release reads (recipes §6.1, §9).
pub const VERSION: &str = "0.1";
const RECIPE: Field = req("recipe", Ty::Enum(&[VERSION]));

/// `[media]`, `[firmware]`: the output's identity (recipes §6.3).
pub static IDENTITY: Table = table(
    "identity",
    &[
        req("name", Ty::Str(Text::Repository)),
        req("platform", Ty::Str(Text::Platform)),
        opt("release", Ty::Str(Text::Release)),
        req("revision", Ty::Int { min: 1, max: MAX }),
        opt("digest", Ty::Str(Text::Digest)),
        opt("min_app_version", Ty::Str(Text::Version)),
    ],
);

static PLAYERS: Table = table(
    "players",
    &[
        req("min", Ty::Int { min: 1, max: 9999 }),
        req("max", Ty::Int { min: 1, max: 9999 }),
    ],
);

/// `[title]` of a Mediafile (recipes §6.4).
pub static TITLE: Table = table(
    "title",
    &[
        opt("title", Ty::Str(Text::Line128)),
        opt("sort_title", Ty::Str(Text::Line128)),
        opt("series", Ty::Str(Text::Line128)),
        opt("version", Ty::Str(Text::Line128)),
        opt("edition", Ty::Str(Text::Line128)),
        opt("source_media", LINE),
        opt("description", Ty::Str(Text::Text)),
        opt("requirements", LINE),
        opt("year", Ty::Int { min: 1, max: 9999 }),
        opt("publishers", CREDITS),
        opt("developers", CREDITS),
        opt("genres", CREDITS),
        opt("languages", LINES),
        opt("regions", LINES),
        opt("software_platform", Ty::Vocab(Vocab::SoftwarePlatform)),
        opt("players", Ty::Table(&PLAYERS)),
        opt("labels", LABELS),
    ],
);

/// `[title]` of a work file: the shared fields and the synopsis (recipes
/// §6.16, which field lives where).
pub static WORK_TITLE: Table = table(
    "work-title",
    &[
        opt("title", Ty::Str(Text::Line128)),
        opt("sort_title", Ty::Str(Text::Line128)),
        opt("series", Ty::Str(Text::Line128)),
        opt("software_platform", Ty::Vocab(Vocab::SoftwarePlatform)),
        opt("players", Ty::Table(&PLAYERS)),
        opt("publishers", LINES),
        opt("developers", LINES),
        opt("genres", LINES),
        opt("labels", LABELS),
        opt("synopsis", LINE),
    ],
);

static FITS: Table = table(
    "fits",
    &[
        opt(
            "boards",
            Ty::Array {
                item: &Ty::Vocab(Vocab::Board),
                min: 0,
            },
        ),
        opt(
            "machine_classes",
            Ty::Array {
                item: &Ty::Vocab(Vocab::MachineClass),
                min: 0,
            },
        ),
    ],
);

/// `[package]` of a Firmwarefile (recipes §6.5).
pub static PACKAGE: Table = table(
    "package",
    &[
        req("title", Ty::Str(Text::Line128)),
        opt("vendor", LINE),
        opt("version", LINE),
        opt("date", Ty::Str(Text::Date)),
        opt("description", Ty::Str(Text::Text)),
        opt("notes", Ty::Str(Text::Text)),
        opt("labels", LABELS),
        opt("fits", Ty::Table(&FITS)),
    ],
);

/// `[license]` (recipes §6.6).
pub static LICENSE: Table = table(
    "license",
    &[
        req("expression", Ty::Str(Text::Spdx)),
        opt("terms", Ty::Str(Text::Id)),
        opt("source", Ty::Str(Text::Id)),
    ],
);

/// The catalogues an input may name an entry of (recipes §6.8).
pub const DATABASES: &[&str] = &["no-intro", "redump", "tosec", "mame"];

static DB_ID: Table = table(
    "db-id",
    &[
        req("db", Ty::Enum(DATABASES)),
        req("set", LINE),
        req("name", LINE),
    ],
);

const URLS: Ty = Ty::Array {
    item: &Ty::Str(Text::Url),
    min: 0,
};
const DB_IDS: Ty = Ty::Array {
    item: &Ty::Table(&DB_ID),
    min: 0,
};

/// An input that is one file (recipes §6.8).
pub static FILE_INPUT: Table = Table {
    name: "file-input",
    marker: None,
    fields: &[
        req("name", Ty::Str(Text::FileName)),
        req("size", SIZE),
        req("sha256", Ty::Str(Text::Sha256)),
        opt("names", LINES),
        opt("crc32", Ty::Str(Text::Crc32)),
        opt("md5", Ty::Str(Text::Md5)),
        opt("sha1", Ty::Str(Text::Sha1)),
        opt("ids", DB_IDS),
        opt("urls", URLS),
        opt("bundle", Ty::Bool),
        opt("disclosable", Ty::Bool),
    ],
};

static FILE_SET_ENTRY: Table = table(
    "file-set-entry",
    &[
        req("path", Ty::Str(Text::Path)),
        req("size", SIZE),
        req("sha256", Ty::Str(Text::Sha256)),
        opt("time", Ty::Time),
        opt("disclosable", Ty::Bool),
    ],
);

/// An input that is a set of files (recipes §6.8).
pub static FILE_SET_INPUT: Table = Table {
    name: "file-set-input",
    marker: Some("files"),
    fields: &[
        req("name", Ty::Str(Text::FileName)),
        req(
            "files",
            Ty::Array {
                item: &Ty::Table(&FILE_SET_ENTRY),
                min: 1,
            },
        ),
        opt("names", LINES),
        opt("ids", DB_IDS),
        opt("urls", URLS),
    ],
};

static MEMBER: Table = table(
    "member",
    &[
        req("path", Ty::Str(Text::Path)),
        opt("size", SIZE),
        opt("sha256", Ty::Str(Text::Sha256)),
        opt("time", Ty::Time),
    ],
);

/// The archive formats `unpack` reads (recipes §6.9).
pub const ARCHIVES: &[&str] = &["zip"];
/// The orders of a `fat` step's entries.
pub const ORDERS: &[&str] = &["name", "listed"];

/// The verbs of a step, in the order of recipes §6.9.
pub const VERBS: &[&str] = &["unpack", "fat", "text"];

/// `unpack`: listed members of an archive, as a tree.
pub static UNPACK: Table = Table {
    name: "unpack-step",
    marker: Some("unpack"),
    fields: &[
        req("id", Ty::Str(Text::Id)),
        req("unpack", Ty::Str(Text::Ref)),
        req("format", Ty::Enum(ARCHIVES)),
        req(
            "members",
            Ty::Array {
                item: &Ty::Table(&MEMBER),
                min: 1,
            },
        ),
        opt("complete", Ty::Bool),
    ],
};

/// `fat`: a FAT volume, or several, from a tree.
pub static FAT: Table = Table {
    name: "fat-step",
    marker: Some("fat"),
    fields: &[
        req("id", Ty::Str(Text::Id)),
        req("fat", Ty::Str(Text::Ref)),
        req("format", Ty::Vocab(Vocab::VolumeFormat)),
        opt("label", LINE),
        opt(
            "serial",
            Ty::Int {
                min: 0,
                max: u32::MAX as i64,
            },
        ),
        opt("order", Ty::Enum(ORDERS)),
        opt("time", Ty::Time),
        opt("volumes", Ty::Int { min: 2, max: 9999 }),
    ],
};

/// `text`: our own text with DOS line ends.
pub static TEXT: Table = Table {
    name: "text-step",
    marker: Some("text"),
    fields: &[
        req("id", Ty::Str(Text::Id)),
        req("text", Ty::Str(Text::Ref)),
        req("eol", Ty::Enum(&["crlf"])),
    ],
};

/// A medium of a Mediafile (recipes §6.10).
pub static MEDIUM: Table = table(
    "medium",
    &[
        req("id", Ty::Str(Text::Id)),
        req("label", Ty::Str(Text::Line128)),
        req("format", Ty::Vocab(Vocab::MediaFormat)),
        opt("file_name", Ty::Str(Text::FileName)),
        req("from", Ty::Str(Text::Ref)),
    ],
);

static PART: Table = table(
    "part",
    &[
        opt("position", LINE),
        opt("part_number", LINE),
        req("lane", Ty::Vocab(Vocab::ChipLane)),
        opt("offset", SIZE),
        req("from", Ty::Str(Text::Ref)),
    ],
);

/// A ROM stored as one blob.
pub static ROM_FILE: Table = table(
    "rom-file",
    &[
        req("id", Ty::Str(Text::Id)),
        req("role", Ty::Vocab(Vocab::RomRole)),
        opt("catalogue_id", Ty::Str(Text::Id)),
        req("from", Ty::Str(Text::Ref)),
    ],
);

/// A ROM in chips: its parts and the logical ROM they assemble.
pub static ROM_PARTS: Table = Table {
    name: "rom-parts",
    marker: Some("parts"),
    fields: &[
        req("id", Ty::Str(Text::Id)),
        req("role", Ty::Vocab(Vocab::RomRole)),
        opt("catalogue_id", Ty::Str(Text::Id)),
        req(
            "parts",
            Ty::Array {
                item: &Ty::Table(&PART),
                min: 1,
            },
        ),
        req("size", Ty::Int { min: 1, max: MAX }),
        req("sha256", Ty::Str(Text::Sha256)),
    ],
};

/// The kinds of an extra (recipes §6.10).
pub const EXTRA_KINDS: &[&str] = &["document", "archive", "executable", "other"];

/// A loose file of the artifact (recipes §6.10).
pub static EXTRA: Table = table(
    "extra",
    &[
        req("id", Ty::Str(Text::Id)),
        req("file_name", Ty::Str(Text::FileName)),
        req("kind", Ty::Enum(EXTRA_KINDS)),
        opt("dos_name", Ty::Vocab(Vocab::DosName)),
        req("from", Ty::Str(Text::Ref)),
    ],
);

/// An optional part (recipes §6.10).
pub static ATTACHMENT: Table = table(
    "attachment",
    &[
        req("id", Ty::Str(Text::Id)),
        req("title", Ty::Str(Text::Line128)),
        req("kind", Ty::Enum(&["documentation"])),
        opt("digest", Ty::Str(Text::Digest)),
        req(
            "extra",
            Ty::Array {
                item: &Ty::Table(&EXTRA),
                min: 1,
            },
        ),
    ],
);

const IDS: Ty = Ty::Map {
    key: MapKey::Kind,
    value: &Ty::Str(Text::Line128),
};
const NEEDS: Ty = Ty::Map {
    key: MapKey::Kind,
    value: &Ty::Needs,
};
const INPUTS: Ty = Ty::Map {
    key: MapKey::Id,
    value: &Ty::OneOf(&[&FILE_SET_INPUT, &FILE_INPUT]),
};
const STEPS: Ty = Ty::Array {
    item: &Ty::OneOf(&[&UNPACK, &FAT, &TEXT]),
    min: 0,
};
const EXTRAS: Ty = Ty::Array {
    item: &Ty::Table(&EXTRA),
    min: 0,
};
const ATTACHMENTS: Ty = Ty::Array {
    item: &Ty::Table(&ATTACHMENT),
    min: 0,
};

/// A Mediafile (recipes §6.2). `[[medium]]` is required of the kind and
/// `[title]` of the recipe and its work file together, which rule 2 checks.
pub static MEDIAFILE: Table = table(
    "mediafile",
    &[
        RECIPE,
        opt("work", Ty::Str(Text::Id)),
        req("media", Ty::Table(&IDENTITY)),
        opt("title", Ty::Table(&TITLE)),
        opt("license", Ty::Table(&LICENSE)),
        opt("needs", NEEDS),
        opt("ids", IDS),
        opt("input", INPUTS),
        opt("step", STEPS),
        opt(
            "medium",
            Ty::Array {
                item: &Ty::Table(&MEDIUM),
                min: 0,
            },
        ),
        opt("extra", EXTRAS),
        opt("attachment", ATTACHMENTS),
    ],
);

/// A Firmwarefile (recipes §6.2). `[package]` and `[[rom]]` are required of
/// the kind, which rule 2 checks.
pub static FIRMWAREFILE: Table = table(
    "firmwarefile",
    &[
        RECIPE,
        req("firmware", Ty::Table(&IDENTITY)),
        opt("package", Ty::Table(&PACKAGE)),
        opt("license", Ty::Table(&LICENSE)),
        opt("ids", IDS),
        opt("input", INPUTS),
        opt("step", STEPS),
        opt(
            "rom",
            Ty::Array {
                item: &Ty::OneOf(&[&ROM_PARTS, &ROM_FILE]),
                min: 0,
            },
        ),
        opt("extra", EXTRAS),
        opt("attachment", ATTACHMENTS),
    ],
);

static WORK: Table = table("work", &[req("id", Ty::Str(Text::Id))]);

/// A work file (recipes §6.16): what the releases of one work share.
pub static WORK_FILE: Table = table(
    "work-file",
    &[
        RECIPE,
        req("work", Ty::Table(&WORK)),
        opt("title", Ty::Table(&WORK_TITLE)),
        opt("ids", IDS),
        opt("needs", NEEDS),
    ],
);

/// The identity tables, one per kind; `machine` is reserved for the
/// Machinefile, which version 0.1 does not specify (recipes §6.2, §11).
pub const IDENTITIES: &[&str] = &["media", "firmware", "machine"];

/// The top-level keys only a Mediafile has, and only a Firmwarefile has.
pub const MEDIA_ONLY: &[&str] = &["work", "media", "title", "needs", "medium"];
pub const FIRMWARE_ONLY: &[&str] = &["firmware", "package", "rom"];

impl Text {
    /// Why `value` is not of this form, or `None`.
    pub fn fault(self, value: &str) -> Option<String> {
        let ok = match self {
            Self::Text => {
                !value.trim().is_empty() && !value.chars().any(|c| control(c) && c != '\n')
            }
            Self::Line => line(value),
            Self::Line128 => image_store::media_set::valid_text(value),
            Self::Id => image_store::media_set::valid_id(value),
            Self::Ref => reference(value),
            Self::Sha256 => hex(value, 64),
            Self::Sha1 => hex(value, 40),
            Self::Md5 => hex(value, 32),
            Self::Crc32 => hex(value, 8),
            Self::Digest => value.strip_prefix("sha256:").is_some_and(|h| hex(h, 64)),
            Self::Repository => repository(value),
            Self::Platform => platform(value),
            Self::Release => value.len() <= MAX_RELEASE && image_store::tag::valid_release(value),
            Self::Version => version(value),
            Self::Url => {
                value.len() > "https://".len()
                    && value.starts_with("https://")
                    && !value.chars().any(|c| c.is_whitespace() || control(c))
            }
            Self::FileName => {
                image_store::media_set::valid_text(value)
                    && !value.contains(['/', '\\'])
                    && value != "."
                    && value != ".."
            }
            Self::Path => path(value),
            Self::Date => image_store::firmware::valid_date(value),
            Self::Spdx => line(value),
        };
        (!ok).then(|| self.expected().to_owned())
    }

    /// What a value of this form is, for a message.
    pub fn expected(self) -> &'static str {
        match self {
            Self::Text => "expected text that is not blank, without control characters other than line breaks",
            Self::Line => "expected one line that is not blank, without control characters",
            Self::Line128 => "expected one line of at most 128 characters that is not blank, without control characters",
            Self::Id => "expected an id: lower-case letters, digits, '.', '_' and '-', starting with a letter or digit, at most 64 characters",
            Self::Ref => "expected a value name, optionally followed by '/' and a member path",
            Self::Sha256 => "expected 64 lower-case hexadecimal digits",
            Self::Sha1 => "expected 40 lower-case hexadecimal digits",
            Self::Md5 => "expected 32 lower-case hexadecimal digits",
            Self::Crc32 => "expected 8 lower-case hexadecimal digits",
            Self::Digest => "expected sha256: and 64 lower-case hexadecimal digits",
            Self::Repository => "expected a repository of four components in the OCI grammar, such as unemu/media/ibm-pc/freedos",
            Self::Platform => "expected a platform id: lower-case letters and digits, then also '-', '_' and '.', at most 64 characters",
            Self::Release => "expected a release of at most 107 characters: parts of lower-case letters and digits joined by '-', '.' or '_', none of them r followed by digits",
            Self::Version => "expected a version X.Y.Z",
            Self::Url => "expected an https URL without white space",
            Self::FileName => "expected a file name of at most 128 characters, without '/' or '\\', not '.' or '..'",
            Self::Path => "expected a relative path with '/' separators, without empty, '.' or '..' parts, '\\' or a drive",
            Self::Date => "expected a calendar date YYYY-MM-DD",
            Self::Spdx => "expected an SPDX licence expression",
        }
    }
}

/// A C0 control character or DEL, as the store's text checks refuse them.
fn control(c: char) -> bool {
    c <= '\u{1f}' || c == '\u{7f}'
}

fn line(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(control)
}

fn hex(value: &str, digits: usize) -> bool {
    value.len() == digits
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One path component of the OCI distribution grammar:
/// `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*`.
fn component(part: &str) -> bool {
    let bytes = part.as_bytes();
    let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !bytes.first().is_some_and(alnum) || !bytes.last().is_some_and(alnum) {
        return false;
    }
    let mut at = 0;
    while at < bytes.len() {
        if alnum(&bytes[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < bytes.len() && !alnum(&bytes[at]) {
            at += 1;
        }
        let separator = &part[start..at];
        if !(separator == "." || separator == "_" || separator == "__")
            && !separator.bytes().all(|b| b == b'-')
        {
            return false;
        }
    }
    true
}

fn repository(value: &str) -> bool {
    value.len() <= 255 && value.split('/').count() == 4 && value.split('/').all(component)
}

fn platform(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && b"-_.".contains(&b))
        })
}

fn version(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The parts of a relative path; refuses what [`Text::Path`] refuses.
fn path(value: &str) -> bool {
    let drive = value.len() >= 2
        && value.as_bytes()[1] == b':'
        && value.as_bytes()[0].is_ascii_alphabetic();
    !drive
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains('\\')
                && !part.chars().any(control)
        })
}

fn reference(value: &str) -> bool {
    match value.split_once('/') {
        Some((name, member)) => image_store::media_set::valid_id(name) && path(member),
        None => image_store::media_set::valid_id(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_forms_accept_the_corpus_shapes_and_refuse_their_neighbours() {
        for (form, good, bad) in [
            (
                Text::Repository,
                "unemu/media/ibm-pc/freedos",
                "unemu/media/freedos",
            ),
            (
                Text::Repository,
                "unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3",
                "unemu/media/ibm-pc/Free",
            ),
            (Text::Repository, "a/b/c/d__e", "a/b/c/d___e"),
            (Text::Repository, "a/b/c/d--e", "a/b/c/d-"),
            (Text::Release, "1.4-720k", "1.4-r2"),
            (
                Text::Release,
                &"a".repeat(MAX_RELEASE),
                &"a".repeat(MAX_RELEASE + 1),
            ),
            (Text::Ref, "disks/720k/x86BOOT.img", "disks//x"),
            (Text::Ref, "files", "Files"),
            (Text::Ref, "game/LEVEL1/A.BIN", "game/../A.BIN"),
            (Text::Path, "LEVEL1/ATTACK.BIN", "/ATTACK.BIN"),
            (Text::Path, "readme.txt", "C:/x"),
            (Text::Path, "a/b", "a\\b"),
            (
                Text::Url,
                "https://example.org/x.zip",
                "http://example.org/x.zip",
            ),
            (
                Text::Url,
                "https://example.org/x",
                "https://example.org/a b",
            ),
            (Text::FileName, "Contra (USA).nes", "a/b"),
            (Text::FileName, "x", ".."),
            (
                Text::Digest,
                &format!("sha256:{}", "a".repeat(64)),
                "sha256:ABC",
            ),
            (Text::Sha256, &"0".repeat(64), &"0".repeat(63)),
            (Text::Platform, "ibm-pc", "-pc"),
            (Text::Version, "0.0.3", "0.0"),
            (Text::Date, "1985-06-10", "1985-02-30"),
            (Text::Line, "x", " "),
            (Text::Line, "x", "a\nb"),
            (Text::Text, "a\nb", "a\tb"),
            (Text::Id, "disk-1", "Disk-1"),
        ] {
            assert_eq!(form.fault(good), None, "{form:?} {good}");
            assert!(form.fault(bad).is_some(), "{form:?} {bad}");
        }
    }

    /// The JSON Schema needs a definition name per table; two tables with
    /// one name would overwrite each other.
    #[test]
    fn table_names_are_unique() {
        type Seen = Vec<(*const Table, &'static str)>;
        fn walk(table: &'static Table, seen: &mut Seen) {
            if seen.iter().any(|(t, _)| std::ptr::eq(*t, table)) {
                return;
            }
            seen.push((table, table.name));
            for field in table.fields {
                walk_ty(&field.ty, seen);
            }
        }
        fn walk_ty(ty: &'static Ty, seen: &mut Seen) {
            match ty {
                Ty::Array { item, .. } => walk_ty(item, seen),
                Ty::Table(table) => walk(table, seen),
                Ty::OneOf(tables) => tables.iter().for_each(|t| walk(t, seen)),
                Ty::Map { value, .. } => walk_ty(value, seen),
                _ => {}
            }
        }
        let mut seen = Seen::new();
        for root in [&MEDIAFILE, &FIRMWAREFILE, &WORK_FILE] {
            walk(root, &mut seen);
        }
        let names: Vec<&str> = seen.iter().map(|(_, name)| *name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
    }
}
