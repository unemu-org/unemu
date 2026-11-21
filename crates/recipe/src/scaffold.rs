// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu recipe scaffold` (recipes §6.15): the `[input]` tables, member
//! lists and file sets of files the author has, written in the recipe
//! syntax so that a recipe starts from measured sizes, hashes and times
//! instead of typed ones. The author adds the description, the outputs and
//! whatever else the recipe states; nothing here is a placeholder.
//!
//! What each operand gives, in the order the operands are named:
//!
//! - A **plain file**: `[input.ID]` with its `name`, `size` and `sha256`.
//! - A **zip archive** (a file whose name ends in `.zip`, in any case): the
//!   same table, and an `unpack` step that lists every file member in the
//!   archive's own order with its size, SHA-256 and the time its central
//!   directory states (recipes §6.9), and `complete = true`, as the list is
//!   the whole archive. A member without a valid DOS time is listed without
//!   one. This is the only place the archive's order and times are read:
//!   a build takes nothing but bytes from an archive (recipes §3.4), so the
//!   recipe states them. An archive an `unpack` step would refuse when it
//!   opens it (ZIP64, an ambiguous, unsafe or backslashed name, an encrypted
//!   member or a method other than stored and deflate) is refused here.
//! - A **folder**: a file-set input (recipes §3.3, §6.8) of every file below
//!   it, in the order of a `fat` step's `order = "name"` (recipes §6.9):
//!   in every folder its files, then its folders, each group by the bytes
//!   of the 8.3 names as a directory entry stores them, the 11-byte field
//!   with stem and extension padded by spaces, as the FAT builder orders
//!   them (`order_key`); names that are not valid 8.3 names follow, by
//!   the bytes of the name (a `fat` step refuses them, so their order only
//!   matters for a file set that feeds none). No time is written: a host
//!   file's time is the time of the user's copy, never the work's (recipes
//!   §6.12). Hidden files and folders (a name starting with `.`) are
//!   skipped, symbolic links are followed, and a folder holds at most
//!   [`MAX_FILE_SET_ENTRIES`] files, counted over every folder named, as a
//!   recipe does (rule 9 of recipes §6.11), which also limits a run to
//!   [`MAX_INPUTS`] operands and [`MAX_STEPS`] archives.
//!
//! The input tables come first, then the steps. An input's id is its name
//! in lower case, a file's last extension dropped, every character other
//! than letters, digits, `_` and `-` written as `-`; an `unpack` step's id
//! is the input's id and `-files`; a second use of an id takes `-2`, `-3`.
//! Names are the file's or folder's own name and paths are relative to the
//! folder named, so no host path is written. The output is a function of
//! the operands' names and bytes, and the archive's directory.

use crate::check::{MAX_FILE_SET_ENTRIES, MAX_INPUTS, MAX_STEPS};
use crate::model::LocalTime;
use crate::spec::Text;
use crate::steps::{dos_time, member_problem, show_time};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Why nothing was written.
#[derive(Debug)]
pub enum Error {
    /// A file or folder a recipe cannot hold as it is: a name or member a
    /// recipe refuses, an archive an `unpack` step refuses, too many files.
    Refused(String),
    /// A file or folder that cannot be read.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(message) => f.write_str(message),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

fn refused(message: impl Into<String>) -> Error {
    Error::Refused(message.into())
}

/// A file that changed while it was read: a read error, not a refusal.
fn changed(shown: &Path, detail: &str) -> Error {
    Error::Io(io::Error::other(format!(
        "{}: changed while it was read{detail}",
        shown.display()
    )))
}

/// An error of the zip reader: data an `unpack` step refuses is a refusal,
/// anything else a read error.
fn zip_error(shown: &Path, error: io::Error) -> Error {
    if error.kind() == io::ErrorKind::InvalidData {
        refused(format!("{}: {error}", shown.display()))
    } else {
        io_at(shown, error)
    }
}

/// An I/O error that names the operand as it was given.
fn io_at(shown: &Path, error: io::Error) -> Error {
    Error::Io(io::Error::new(
        error.kind(),
        format!("{}: {error}", shown.display()),
    ))
}

/// One file of a folder before it is hashed: its path in the file set,
/// where it is on the host and the size its metadata states.
#[derive(Clone, Debug)]
pub(crate) struct Listed {
    pub path: String,
    pub host: PathBuf,
    pub size: u64,
}

/// One member or file-set entry as it is written.
pub(crate) struct Row {
    path: String,
    size: u64,
    sha256: String,
    time: Option<LocalTime>,
}

enum Operand {
    File {
        name: String,
        size: u64,
        sha256: String,
    },
    Archive {
        name: String,
        size: u64,
        sha256: String,
        members: Vec<Row>,
    },
    Folder {
        name: String,
        files: Vec<Row>,
    },
}

/// The tables of the files and folders at `paths`, as recipe text.
pub fn scaffold(paths: &[PathBuf]) -> Result<String, Error> {
    // Every folder is listed before anything is hashed, so that the limit
    // refuses a run before it reads a byte.
    let mut planned = Vec::new();
    let mut entries = 0usize;
    for path in paths {
        let meta = std::fs::metadata(path).map_err(|e| io_at(path, e))?;
        let name = own_name(path)?;
        if meta.is_dir() {
            if let Some(fault) = Text::FileName.fault(&name) {
                return Err(refused(format!("{}: {fault}", path.display())));
            }
            let mut listed = Vec::new();
            walk(path, path, "", &mut Vec::new(), &mut listed, entries)?;
            if listed.is_empty() {
                return Err(refused(format!(
                    "{}: no files; a file set lists at least one",
                    path.display()
                )));
            }
            entries += listed.len();
            planned.push((path, name, Some(listed)));
        } else if meta.is_file() {
            planned.push((path, name, None));
        } else {
            return Err(refused(format!(
                "{}: neither a file nor a folder",
                path.display()
            )));
        }
    }
    // Rule 9's other limits: an input per operand, a step per archive.
    let archives = planned
        .iter()
        .filter(|(_, name, listed)| listed.is_none() && is_zip(name))
        .count();
    for (count, max, what) in [
        (
            planned.len(),
            MAX_INPUTS,
            "files and folders, one input each",
        ),
        (archives, MAX_STEPS, "zip archives, one step each"),
    ] {
        if count > max {
            return Err(refused(format!(
                "{count} {what}; a recipe holds at most {max} (recipes §6.11, rule 9)"
            )));
        }
    }
    let mut operands = Vec::new();
    let mut earlier = 0usize;
    for (path, name, listed) in planned {
        operands.push(match listed {
            Some(listed) => {
                let count = listed.len();
                let files = file_set(listed, earlier)?;
                earlier += count;
                Operand::Folder { name, files }
            }
            None => file_operand(path, name)?,
        });
    }
    Ok(render(&operands))
}

/// The name of the file or folder at `path` itself: its last component, or
/// for a path such as `.` the last component of where it leads.
fn own_name(path: &Path) -> Result<String, Error> {
    let last = match path.file_name() {
        Some(name) => name.to_owned(),
        None => path
            .canonicalize()
            .map_err(|e| io_at(path, e))?
            .file_name()
            .ok_or_else(|| refused(format!("{}: a path without a name", path.display())))?
            .to_owned(),
    };
    last.into_string()
        .map_err(|_| refused(format!("{}: a name that is not UTF-8", path.display())))
}

/// Whether a file is taken as a zip archive: by its name.
fn is_zip(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".zip")
}

/// Punctuation DOS accepts in an 8.3 name besides letters and digits.
const PUNCTUATION: &[u8] = b"!#$%&'()-@^_`{}~";

/// The sort key of a name in a folder of a file set: a valid 8.3 name as
/// stored (one to eight upper-case letters, digits or DOS punctuation, then
/// optionally a period and one to three more) is keyed by its 11-byte
/// directory field, stem and extension padded with spaces, the bytes the
/// FAT builder sorts by; any other name comes after them, by its bytes.
pub(crate) fn order_key(name: &str) -> (bool, Vec<u8>) {
    let valid = |part: &str, max: usize| {
        (1..=max).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || PUNCTUATION.contains(&b))
    };
    let (stem, extension) = name.split_once('.').unwrap_or((name, ""));
    let ok = valid(stem, 8) && (!name.contains('.') || valid(extension, 3));
    if !ok {
        return (true, name.as_bytes().to_vec());
    }
    let mut field = [b' '; 11];
    field[..stem.len()].copy_from_slice(stem.as_bytes());
    field[8..8 + extension.len()].copy_from_slice(extension.as_bytes());
    (false, field.to_vec())
}

/// Refuse a run with more file-set entries than a recipe holds.
fn limit(entries: usize) -> Result<(), Error> {
    if entries > MAX_FILE_SET_ENTRIES {
        return Err(refused(format!(
            "the folders named hold more than {MAX_FILE_SET_ENTRIES} files; a recipe holds at most {MAX_FILE_SET_ENTRIES} file-set entries (recipes §6.11, rule 9)"
        )));
    }
    Ok(())
}

/// List the files below `dir` into `out`: in every folder its files, then
/// its folders, each by [`order_key`]. `earlier` files were
/// listed for folders named before; the walk stops at the limit.
fn walk(
    root: &Path,
    dir: &Path,
    prefix: &str,
    ancestors: &mut Vec<PathBuf>,
    out: &mut Vec<Listed>,
    earlier: usize,
) -> Result<(), Error> {
    let shown = |relative: &str| root.join(relative);
    let canonical = dir.canonicalize().map_err(|e| io_at(&shown(prefix), e))?;
    if ancestors.contains(&canonical) {
        return Err(refused(format!(
            "{}: a folder that contains itself through a link",
            shown(prefix).display()
        )));
    }
    ancestors.push(canonical);
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let read = std::fs::read_dir(dir).map_err(|e| io_at(&shown(prefix), e))?;
    for entry in read {
        let entry = entry.map_err(|e| io_at(&shown(prefix), e))?;
        if entry.file_name().as_encoded_bytes().starts_with(b".") {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            return Err(refused(format!(
                "{}: a name that is not UTF-8",
                entry.path().display()
            )));
        };
        let relative = format!("{prefix}{name}");
        let host = entry.path();
        let meta = std::fs::metadata(&host).map_err(|e| io_at(&shown(&relative), e))?;
        if meta.is_dir() {
            folders.push((name, host));
        } else if meta.is_file() {
            files.push((name, host, meta.len()));
        } else {
            return Err(refused(format!(
                "{}: neither a file nor a folder",
                shown(&relative).display()
            )));
        }
    }
    files.sort_by_cached_key(|file| order_key(&file.0));
    folders.sort_by_cached_key(|folder| order_key(&folder.0));
    for (name, host, size) in files {
        let path = format!("{prefix}{name}");
        if let Some(fault) = Text::Path.fault(&path) {
            return Err(refused(format!("{}: {fault}", shown(&path).display())));
        }
        out.push(Listed { path, host, size });
        limit(earlier + out.len())?;
    }
    for (name, host) in folders {
        walk(
            root,
            &host,
            &format!("{prefix}{name}/"),
            ancestors,
            out,
            earlier,
        )?;
    }
    ancestors.pop();
    Ok(())
}

/// Hash a folder's files. `earlier` entries were listed for folders named
/// before; the limit is checked before any file is read.
pub(crate) fn file_set(listed: Vec<Listed>, earlier: usize) -> Result<Vec<Row>, Error> {
    limit(earlier + listed.len())?;
    listed
        .into_iter()
        .map(|file| {
            Ok(Row {
                sha256: hash_file(&file.host, file.size, Path::new(&file.path))?,
                path: file.path,
                size: file.size,
                time: None,
            })
        })
        .collect()
}

/// The SHA-256 of a file of `size` bytes; a file whose length is another
/// when it is read changed meanwhile.
fn hash_file(host: &Path, size: u64, shown: &Path) -> Result<String, Error> {
    let mut file = File::open(host).map_err(|e| io_at(shown, e))?;
    let mut sha = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut read = 0u64;
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                sha.update(&buffer[..n]);
                read += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io_at(shown, e)),
        }
    }
    if read != size {
        return Err(changed(shown, &format!(" ({size} bytes, then {read})")));
    }
    Ok(format!("{:x}", sha.finalize()))
}

/// A plain file or, by its name, a zip archive.
fn file_operand(path: &Path, name: String) -> Result<Operand, Error> {
    if let Some(fault) = Text::FileName.fault(&name) {
        return Err(refused(format!("{}: {fault}", path.display())));
    }
    let size = std::fs::metadata(path).map_err(|e| io_at(path, e))?.len();
    let sha256 = hash_file(path, size, path)?;
    if !is_zip(&name) {
        return Ok(Operand::File { name, size, sha256 });
    }
    let shown = path.display();
    let listing = crate::zip::listing(path).map_err(|e| zip_error(path, e))?;
    if listing.is_empty() {
        return Err(refused(format!(
            "{shown}: no file members; an unpack step lists at least one"
        )));
    }
    let mut members = Vec::new();
    for entry in &listing {
        let member = &entry.member;
        if let Some(problem) = member_problem(member) {
            return Err(refused(format!("{shown}: {problem}")));
        }
        if let Some(fault) = Text::Path.fault(&member.name) {
            return Err(refused(format!(
                "{shown}: the archive's member {:?}: {fault}",
                member.name
            )));
        }
        let sha256 = crate::zip::hash(path, member).map_err(|e| zip_error(path, e))?;
        members.push(Row {
            path: member.name.clone(),
            size: member.size,
            sha256,
            time: dos_time(entry.time, entry.date),
        });
    }
    // The archive must still be the one hashed above.
    let after = std::fs::metadata(path).map_err(|e| io_at(path, e))?.len();
    if after != size {
        return Err(changed(path, ""));
    }
    Ok(Operand::Archive {
        name,
        size,
        sha256,
        members,
    })
}

/// The id a name gives: lower case, a file's last extension dropped, every
/// character other than a letter, digit, `_` or `-` written as `-`,
/// starting with a letter or digit, at most 64 characters; `input` when
/// nothing is left. A taken id gets `-2`, `-3`, ….
fn id_for(name: &str, file: bool, taken: &mut BTreeSet<String>) -> String {
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if file && !stem.is_empty() => stem,
        _ => name,
    };
    let mapped: String = stem
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '_' | '-') => c,
            _ => '-',
        })
        .collect();
    let trimmed = mapped.trim_start_matches(['_', '-']);
    let base = if trimmed.is_empty() { "input" } else { trimmed };
    unique(base, taken)
}

/// `base`, cut to 64 characters, or with the first free `-N` after it.
fn unique(base: &str, taken: &mut BTreeSet<String>) -> String {
    let cut = |text: &str, room: usize| text[..text.len().min(room)].to_owned();
    let mut id = cut(base, 64);
    let mut n = 2;
    while taken.contains(&id) {
        let suffix = format!("-{n}");
        id = format!("{}{suffix}", cut(base, 64 - suffix.len()));
        n += 1;
    }
    taken.insert(id.clone());
    id
}

/// A TOML basic string.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c <= '\u{1f}' || c == '\u{7f}' => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn row(row: &Row) -> String {
    let mut line = format!(
        "    {{ path = {}, size = {}, sha256 = {}",
        quote(&row.path),
        row.size,
        quote(&row.sha256)
    );
    if let Some(time) = &row.time {
        line.push_str(&format!(", time = {}", show_time(time)));
    }
    line.push_str(" },\n");
    line
}

/// The input tables in operand order, then the `unpack` steps.
fn render(operands: &[Operand]) -> String {
    let mut taken = BTreeSet::new();
    let ids: Vec<String> = operands
        .iter()
        .map(|operand| match operand {
            Operand::File { name, .. } | Operand::Archive { name, .. } => {
                id_for(name, true, &mut taken)
            }
            Operand::Folder { name, .. } => id_for(name, false, &mut taken),
        })
        .collect();
    let mut tables = Vec::new();
    for (operand, id) in operands.iter().zip(&ids) {
        let table = match operand {
            Operand::File { name, size, sha256 }
            | Operand::Archive {
                name, size, sha256, ..
            } => {
                format!(
                    "[input.{id}]\nname = {}\nsize = {size}\nsha256 = {}\n",
                    quote(name),
                    quote(sha256)
                )
            }
            Operand::Folder { name, files } => {
                let mut table = format!("[input.{id}]\nname = {}\nfiles = [\n", quote(name));
                for file in files {
                    table.push_str(&row(file));
                }
                table.push_str("]\n");
                table
            }
        };
        tables.push(table);
    }
    for (operand, id) in operands.iter().zip(&ids) {
        if let Operand::Archive { members, .. } = operand {
            let step = unique(&format!("{id}-files"), &mut taken);
            let mut table = format!(
                "[[step]]\nid = {}\nunpack = {}\nformat = \"zip\"\ncomplete = true\nmembers = [\n",
                quote(&step),
                quote(id)
            );
            for member in members {
                table.push_str(&row(member));
            }
            table.push_str("]\n");
            tables.push(table);
        }
    }
    tables.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::build::{archive, stored, Entry};

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("recipe-scaffold-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn refusal(paths: &[PathBuf]) -> String {
        match scaffold(paths) {
            Err(Error::Refused(message)) => message,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn ids_are_names_in_lower_case_and_unique() {
        let mut taken = BTreeSet::new();
        let ids: Vec<String> = [
            ("LABFULL.ZIP", true),
            ("labfull.zip", true),
            ("AZURAMON", false),
            ("FreeDOS 1.4.img", true),
            ("v1.0", false),
            ("--x", false),
            ("é.bin", true),
            (".hidden", true),
        ]
        .iter()
        .map(|(name, file)| id_for(name, *file, &mut taken))
        .collect();
        assert_eq!(
            ids,
            [
                "labfull",
                "labfull-2",
                "azuramon",
                "freedos-1-4",
                "v1-0",
                "x",
                "input",
                "hidden"
            ]
        );
        let long = "A".repeat(70);
        let mut taken = BTreeSet::new();
        assert_eq!(id_for(&long, false, &mut taken), "a".repeat(64));
        assert_eq!(
            id_for(&long, false, &mut taken),
            format!("{}-2", "a".repeat(62))
        );
        for id in taken {
            assert_eq!(Text::Id.fault(&id), None, "{id}");
        }
    }

    #[test]
    fn the_file_set_limit_refuses_before_any_file_is_read() {
        // A generated listing whose files do not exist: the limit is checked
        // first, so 100,001 entries are refused without a read, and 100,000
        // pass it and fail on the first file that is not there.
        let listing = |n: usize| -> Vec<Listed> {
            (0..n)
                .map(|i| Listed {
                    path: format!("F{i}"),
                    host: PathBuf::from("/nonexistent/recipe-scaffold-limit"),
                    size: 0,
                })
                .collect()
        };
        match file_set(listing(MAX_FILE_SET_ENTRIES + 1), 0) {
            Err(Error::Refused(message)) => assert_eq!(
                message,
                "the folders named hold more than 100000 files; a recipe holds at most 100000 file-set entries (recipes §6.11, rule 9)"
            ),
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("accepted"),
        }
        assert!(matches!(
            file_set(listing(MAX_FILE_SET_ENTRIES), 0),
            Err(Error::Io(_))
        ));
        // Counted over every folder named.
        assert!(matches!(
            file_set(listing(1), MAX_FILE_SET_ENTRIES),
            Err(Error::Refused(_))
        ));
        // The walk stops at the limit too.
        let temp = Temp::new("walk-limit");
        std::fs::write(temp.0.join("A"), b"a").unwrap();
        let mut out = Vec::new();
        assert!(matches!(
            walk(
                &temp.0,
                &temp.0,
                "",
                &mut Vec::new(),
                &mut out,
                MAX_FILE_SET_ENTRIES
            ),
            Err(Error::Refused(_))
        ));
    }

    #[test]
    fn an_archive_an_unpack_step_refuses_is_refused() {
        let temp = Temp::new("refused");
        let cases: [(&str, Vec<u8>, &str); 5] = [
            (
                "TWICE.ZIP",
                archive(&[stored("A.TXT", b"one"), stored("A.TXT", b"two")]),
                "a name two members share: ambiguous",
            ),
            (
                "SLASH.ZIP",
                archive(&[Entry {
                    name: b"DIR\\A.TXT",
                    ..stored("", b"x")
                }]),
                "a backslash in its stored name",
            ),
            (
                "UP.ZIP",
                archive(&[stored("../A.TXT", b"x")]),
                "a `..` path component",
            ),
            (
                "EMPTY.ZIP",
                archive(&[stored("DIR/", b"")]),
                "no file members",
            ),
            (
                "NOT.ZIP",
                b"not a zip".to_vec(),
                "no end of central directory",
            ),
        ];
        for (name, bytes, why) in cases {
            let path = temp.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            let message = refusal(&[path]);
            assert!(message.contains(why), "{name}: {message}");
        }
    }

    #[test]
    fn a_folder_without_files_or_with_a_loop_is_refused() {
        let temp = Temp::new("folders");
        let empty = temp.0.join("EMPTY");
        std::fs::create_dir_all(empty.join("SUB")).unwrap();
        std::fs::write(empty.join(".hidden"), b"x").unwrap();
        assert!(refusal(&[empty]).contains("no files"));
        #[cfg(unix)]
        {
            let looped = temp.0.join("LOOP");
            std::fs::create_dir_all(&looped).unwrap();
            std::fs::write(looped.join("A"), b"a").unwrap();
            std::os::unix::fs::symlink(&looped, looped.join("BACK")).unwrap();
            assert!(refusal(&[looped]).contains("contains itself"));
        }
        let missing = temp.0.join("MISSING");
        assert!(matches!(scaffold(&[missing]), Err(Error::Io(_))));
    }

    #[test]
    fn a_folder_is_ordered_by_its_8_3_names_as_stored() {
        // Bytes below `.` (here `-`) order a full name otherwise than its
        // space-padded 11-byte field: by bytes `A-B.TXT` comes before
        // `A.TXT`, by the field `A       TXT` comes before `A-B     TXT`.
        let names = ["A-B.TXT", "A.TXT", "AB", "AB-C.TXT", "AB.TXT"];
        let mut sorted = names.to_vec();
        sorted.sort_by_cached_key(|n| order_key(n));
        assert_eq!(sorted, ["A.TXT", "A-B.TXT", "AB", "AB.TXT", "AB-C.TXT"]);
        // Names that are not 8.3 names follow, by bytes.
        let mut mixed = vec!["readme.txt", "LONGNAME.TEXT", "B.TXT", "A b.TXT"];
        mixed.sort_by_cached_key(|n| order_key(n));
        assert_eq!(mixed, ["B.TXT", "A b.TXT", "LONGNAME.TEXT", "readme.txt"]);
        let temp = Temp::new("order");
        for name in names {
            std::fs::write(temp.0.join(name), name).unwrap();
        }
        let text = scaffold(std::slice::from_ref(&temp.0)).unwrap();
        let paths: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("    { path = \""))
            .map(|l| &l[..l.find('"').unwrap()])
            .collect();
        assert_eq!(paths, ["A.TXT", "A-B.TXT", "AB", "AB.TXT", "AB-C.TXT"]);
    }

    #[test]
    fn rule_9_limits_inputs_and_steps_before_anything_is_hashed() {
        let temp = Temp::new("rule-9");
        let one = temp.0.join("A.BIN");
        std::fs::write(&one, b"a").unwrap();
        let message = refusal(&vec![one.clone(); MAX_INPUTS + 1]);
        assert_eq!(
            message,
            "4097 files and folders, one input each; a recipe holds at most 4096 (recipes §6.11, rule 9)"
        );
        assert!(scaffold(&vec![one; MAX_INPUTS]).is_ok());
        let zip = temp.0.join("A.ZIP");
        std::fs::write(&zip, archive(&[stored("A", b"a")])).unwrap();
        let message = refusal(&vec![zip.clone(); MAX_STEPS + 1]);
        assert_eq!(
            message,
            "65 zip archives, one step each; a recipe holds at most 64 (recipes §6.11, rule 9)"
        );
        assert!(scaffold(&vec![zip; MAX_STEPS]).is_ok());
    }

    #[test]
    fn a_member_on_a_day_its_month_lacks_is_listed_without_a_time() {
        let temp = Temp::new("bad-day");
        let path = temp.0.join("D.ZIP");
        std::fs::write(
            &path,
            archive(&[Entry {
                time: 10 << 11,
                date: (13 << 9) | (2 << 5) | 30,
                ..stored("A", b"a")
            }]),
        )
        .unwrap();
        let text = scaffold(&[path]).unwrap();
        assert!(text.contains("{ path = \"A\", size = 1, sha256 = \"ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb\" },\n"), "{text}");
    }

    #[test]
    fn a_member_without_a_valid_time_is_listed_without_one() {
        let temp = Temp::new("times");
        let path = temp.0.join("T.ZIP");
        std::fs::write(
            &path,
            archive(&[
                Entry {
                    time: (13 << 11) | (58 << 5) | 11,
                    date: (13 << 9) | (3 << 5) | 27,
                    ..stored("A", b"a")
                },
                Entry {
                    date: 0,
                    ..stored("B", b"b")
                },
            ]),
        )
        .unwrap();
        let text = scaffold(&[path]).unwrap();
        assert!(text.contains("{ path = \"A\", size = 1, sha256 = \"ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb\", time = 1993-03-27T13:58:22 },\n"), "{text}");
        assert!(text.contains("{ path = \"B\", size = 1, sha256 = \"3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d\" },\n"), "{text}");
    }
}
