// SPDX-License-Identifier: AGPL-3.0-or-later
//! Recipes: Mediafiles and Firmwarefiles that state how an artifact is made
//! from inputs found by SHA-256 and size (`backlog/docs/design/recipes.md`,
//! the specification of version 0.1 in §6).
//!
//! This crate is the platform-neutral part: the typed model ([`model`]), the
//! parser of TOML 1.0 with exact positions, and the static checks of
//! recipes §6.11, which [`check_paths`] runs without reading any input. The
//! engine knows files, trees, hashes and TOML; everything about a platform,
//! from media formats to the needs vocabulary, comes from its provider
//! through the traits of [`provider`] (recipes §10). [`schema`] writes the
//! JSON Schema of the format from the same key tables and the providers.
//!
//! Work files (recipes §6.16) are checked with the recipes that name them:
//! their tables, their recipe version and their vocabulary against the
//! platform of each such recipe. [`merge_path`] merges a recipe that passes
//! the checks with its work file into one description record
//! ([`merge::Merged`]), the title metadata a build writes. Building,
//! fetching and planning are not here.

mod check;
mod finding;
pub mod merge;
pub mod model;
mod parse;
pub mod provider;
pub mod schema;
pub mod spec;
mod syntax;

pub use check::{
    Split, DISCLOSABLE_BELOW, MAX_BUNDLE, MAX_FILE_SET_ENTRIES, MAX_INPUTS, MAX_OUTPUTS, MAX_STEPS,
};
pub use finding::{Finding, Rule};
pub use provider::Providers;
pub use syntax::MAX_FILE;

use finding::{Sink, Source};
use model::{Document, WorkFile};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

/// What a checked file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Media,
    Firmware,
    Work,
}

/// One checked file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReport {
    /// The file as it was named, or found in a named folder.
    pub file: String,
    /// `None` when the file could not be read as any kind.
    pub what: Option<What>,
    /// The inputs a recipe's artifact needs and those only an attachment
    /// uses (rule 5).
    pub split: Option<Split>,
}

/// The result of `recipe check`: every file and every finding, sorted by
/// file and position, each finding once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub files: Vec<FileReport>,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// The text `upcli recipe check` prints: the findings, then a line per
    /// file with its kind and the split of its inputs, then a total.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for finding in &self.findings {
            out.push_str(&format!("{finding}\n"));
        }
        for file in &self.files {
            let count = self.findings.iter().filter(|f| f.file == file.file).count();
            let status = match count {
                0 => "ok".to_owned(),
                1 => "1 finding".to_owned(),
                n => format!("{n} findings"),
            };
            out.push_str(&format!("{}: {status}", file.file));
            match file.what {
                Some(What::Media) => out.push_str(", media recipe"),
                Some(What::Firmware) => out.push_str(", firmware recipe"),
                Some(What::Work) => out.push_str(", work file"),
                None => {}
            }
            if let Some(split) = &file.split {
                if !split.required.is_empty() {
                    out.push_str(&format!("; required inputs: {}", split.required.join(", ")));
                }
                if !split.optional.is_empty() {
                    out.push_str(&format!("; optional inputs: {}", split.optional.join(", ")));
                }
            }
            out.push('\n');
        }
        let files = self.files.len();
        out.push_str(&format!(
            "{files} file{} checked, {} finding{}\n",
            if files == 1 { "" } else { "s" },
            self.findings.len(),
            if self.findings.len() == 1 { "" } else { "s" },
        ));
        out
    }
}

/// The file names a folder walk takes: recipes and work files by their
/// conventional suffixes (recipes §6.1, §6.16).
pub const SUFFIXES: &[&str] = &[
    ".media.toml",
    ".firmware.toml",
    ".machine.toml",
    ".work.toml",
];

/// The recipe files named: each file itself, whatever its name, and under a
/// folder every file with one of the [`SUFFIXES`], in path order. Hidden
/// files and folders and lock folders (`*.lock`) are skipped.
pub fn files(paths: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .and_then(|entries| entries.map(|e| e.map(|e| e.path())).collect())
            .map_err(|e| named(dir, e))?;
        entries.sort();
        for path in entries {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if !name.ends_with(".lock") {
                    walk(&path, out)?;
                }
            } else if SUFFIXES.iter().any(|s| name.ends_with(s)) {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, &mut out)?;
        } else {
            std::fs::metadata(path).map_err(|e| named(path, e))?;
            out.push(path.clone());
        }
    }
    Ok(out)
}

/// An I/O error that names its file.
fn named(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

/// Run the static checks of recipes §6.11 on the files and folders named
/// ([`files`]). An I/O error is a file that cannot be read at all, not a
/// finding.
pub fn check_paths(paths: &[PathBuf], providers: Providers<'_>) -> io::Result<Report> {
    let files = files(paths)?;
    let mut findings = BTreeSet::new();
    let mut reports = Vec::new();
    let mut works: BTreeMap<PathBuf, Option<Loaded>> = BTreeMap::new();
    for path in &files {
        let bytes = std::fs::read(path).map_err(|e| named(path, e))?;
        let name = path.display().to_string();
        let lock = PathBuf::from(format!("{name}.lock")).is_dir();
        reports.push(check_file(
            Some(path),
            &name,
            bytes,
            providers,
            lock,
            &mut works,
            &mut findings,
        ));
    }
    Ok(Report {
        files: reports,
        findings: findings.into_iter().collect(),
    })
}

/// Check one file held in memory, which names no work file and has no lock.
#[cfg(test)]
pub(crate) fn check_text(name: &str, text: &str, providers: Providers<'_>) -> Report {
    let mut findings = BTreeSet::new();
    let report = check_file(
        None,
        name,
        text.as_bytes().to_vec(),
        providers,
        false,
        &mut BTreeMap::new(),
        &mut findings,
    );
    Report {
        files: vec![report],
        findings: findings.into_iter().collect(),
    }
}

/// What [`merge_path`] gives for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// The file is a Mediafile that passes the checks: its merged record.
    Merged(Box<merge::Merged>),
    /// The file does not pass the checks: their report, with findings.
    Refused(Report),
    /// The file passes the checks but is no Mediafile, so it has no title
    /// record to merge: a Firmwarefile or a work file.
    NotMedia(What),
}

/// Check one recipe file, as [`check_paths`] does, and merge a Mediafile
/// that passes with the work file it names (recipes §6.16). An I/O error is
/// a file that cannot be read at all, or one that no longer reads as it did
/// when it was checked (`InvalidData`): the files are read twice.
pub fn merge_path(path: &Path, providers: Providers<'_>) -> io::Result<Shown> {
    let report = check_paths(&[path.to_path_buf()], providers)?;
    if !report.is_clean() {
        return Ok(Shown::Refused(report));
    }
    match report.files.first().and_then(|f| f.what) {
        Some(What::Media) => {}
        Some(what) => return Ok(Shown::NotMedia(what)),
        None => return Err(changed(path)),
    }
    let Some(Document::Recipe(recipe)) = document(path)? else {
        return Err(changed(path));
    };
    let work = match &recipe.work {
        Some(id) => {
            let file = path
                .parent()
                .unwrap_or(Path::new(""))
                .join(format!("{}.work.toml", id.value));
            match document(&file)? {
                Some(Document::Work(work)) => Some(work),
                _ => return Err(changed(&file)),
            }
        }
        None => None,
    };
    match merge::merge(&recipe, work.as_deref()) {
        Some(merged) => Ok(Shown::Merged(Box::new(merged))),
        None => Err(changed(path)),
    }
}

/// A file that passed the checks and then no longer read as it did.
fn changed(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "{}: changed while it was read; check it again",
            path.display()
        ),
    )
}

/// The document a file holds, read again after the checks passed on it.
fn document(path: &Path) -> io::Result<Option<Document>> {
    let bytes = std::fs::read(path).map_err(|e| named(path, e))?;
    let (Some(source), _) = read(&path.display().to_string(), bytes) else {
        return Ok(None);
    };
    let mut sink = Sink::new(&source);
    Ok(syntax::parse(&mut sink).and_then(|root| parse::document(&mut sink, &root)))
}

/// A work file read for the recipes that name it.
struct Loaded {
    source: Source,
    work: Option<WorkFile>,
    /// The file holds a recipe, not a work file.
    recipe: bool,
}

enum WorkLookup<'w> {
    Missing(String),
    Unreadable,
    NotWork,
    Read {
        work: &'w WorkFile,
        source: &'w Source,
        file: String,
    },
}

/// Read `<id>.work.toml` beside `recipe`, once per run. What it holds is
/// reported once, whether or not it was named itself.
fn work_file<'w>(
    recipe: Option<&Path>,
    id: &str,
    works: &'w mut BTreeMap<PathBuf, Option<Loaded>>,
    findings: &mut BTreeSet<Finding>,
) -> WorkLookup<'w> {
    let file = format!("{id}.work.toml");
    let Some(recipe) = recipe else {
        return WorkLookup::Missing(file);
    };
    let path = recipe.parent().unwrap_or(Path::new("")).join(&file);
    let entry = works.entry(path.clone()).or_insert_with(|| {
        let bytes = std::fs::read(&path).ok()?;
        let name = path.display().to_string();
        let (source, mut found) = read(&name, bytes);
        let mut recipe = false;
        let work = match &source {
            Some(source) => {
                let mut sink = Sink::new(source);
                let work = syntax::parse(&mut sink).and_then(|root| {
                    match parse::document(&mut sink, &root) {
                        Some(Document::Work(work)) => {
                            if let Some(title) = &work.title {
                                check::players(title, &mut sink);
                            }
                            Some(*work)
                        }
                        Some(Document::Recipe(_)) => {
                            recipe = true;
                            None
                        }
                        None => None,
                    }
                });
                found.extend(sink.findings);
                work
            }
            None => None,
        };
        findings.extend(found);
        Some(Loaded {
            source: source.unwrap_or_else(|| Source::new(name, String::new())),
            work,
            recipe,
        })
    });
    match entry {
        None => WorkLookup::Missing(file),
        Some(Loaded {
            work: None,
            recipe: true,
            ..
        }) => WorkLookup::NotWork,
        Some(Loaded { work: None, .. }) => WorkLookup::Unreadable,
        Some(Loaded {
            source,
            work: Some(work),
            ..
        }) => WorkLookup::Read { work, source, file },
    }
}

/// The source of a file, or the finding that it is not one.
fn read(name: &str, bytes: Vec<u8>) -> (Option<Source>, Vec<Finding>) {
    match syntax::text(bytes) {
        Ok(text) => (Some(Source::new(name, text)), Vec::new()),
        Err((before, message)) => {
            let at = before.len();
            let before = Source::new(name, before);
            let mut sink = Sink::new(&before);
            sink.report(Rule::Syntax, &(at..at), "", message);
            (None, sink.findings)
        }
    }
}

fn check_file(
    path: Option<&Path>,
    name: &str,
    bytes: Vec<u8>,
    providers: Providers<'_>,
    lock: bool,
    works: &mut BTreeMap<PathBuf, Option<Loaded>>,
    findings: &mut BTreeSet<Finding>,
) -> FileReport {
    let mut report = FileReport {
        file: name.to_owned(),
        what: None,
        split: None,
    };
    let (source, found) = read(name, bytes);
    findings.extend(found);
    let Some(source) = source else {
        return report;
    };
    let mut sink = Sink::new(&source);
    let document = syntax::parse(&mut sink).and_then(|root| parse::document(&mut sink, &root));
    match document {
        Some(Document::Work(work)) => {
            report.what = Some(What::Work);
            if let Some(title) = &work.title {
                check::players(title, &mut sink);
            }
        }
        Some(Document::Recipe(recipe)) => {
            report.what = Some(match recipe.kind {
                model::Kind::Media => What::Media,
                model::Kind::Firmware => What::Firmware,
            });
            let lookup = recipe
                .work
                .as_ref()
                .map(|id| work_file(path, &id.value, works, findings));
            let split = match lookup {
                None => check::check(&recipe, &mut sink, check::Work::None, providers, lock),
                Some(WorkLookup::Missing(file)) => check::check(
                    &recipe,
                    &mut sink,
                    check::Work::Missing(file),
                    providers,
                    lock,
                ),
                Some(WorkLookup::NotWork) => {
                    if let Some(work) = &recipe.work {
                        sink.report(
                            Rule::Kind,
                            &work.span,
                            &work.path,
                            format!(
                                "{}.work.toml is a recipe, not a work file: a work file has [work] with id",
                                work.value
                            ),
                        );
                    }
                    check::check(&recipe, &mut sink, check::Work::Recipe, providers, lock)
                }
                Some(WorkLookup::Unreadable) => {
                    check::check(&recipe, &mut sink, check::Work::Unreadable, providers, lock)
                }
                Some(WorkLookup::Read { work, source, file }) => {
                    let mut work_sink = Sink::new(source);
                    let split = check::check(
                        &recipe,
                        &mut sink,
                        check::Work::Read {
                            work,
                            sink: &mut work_sink,
                            file,
                        },
                        providers,
                        lock,
                    );
                    findings.extend(work_sink.findings);
                    split
                }
            };
            report.split = Some(split);
        }
        None => {}
    }
    findings.extend(sink.findings);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(text: &str) -> Vec<String> {
        check_text("x.media.toml", text, Providers(&[]))
            .findings
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    const MINIMAL: &str = "recipe = \"0.1\"\n\n[media]\nname = \"private/media/ibm-pc/x\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n[title]\ntitle = \"X\"\n\n[input.disk]\nname = \"x.img\"\nsize = 1\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n\n[[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n";

    #[test]
    fn without_a_provider_only_the_platform_is_reported() {
        assert_eq!(
            findings(MINIMAL),
            ["x.media.toml:5:12: error[6.11.7] media.platform: no provider of platform `ibm-pc` in this build"]
        );
    }

    #[test]
    fn a_byte_order_mark_is_refused_and_the_rest_still_read() {
        let found = findings(&format!("\u{feff}{MINIMAL}"));
        assert_eq!(
            found[0],
            "x.media.toml:1:1: error[6.11.1] the file starts with a byte-order mark; a recipe is UTF-8 without one"
        );
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn a_file_that_is_not_utf_8_or_too_long_is_refused_at_its_position() {
        let mut report = check_file(
            None,
            "x.media.toml",
            b"a = 1\nb = \"\xff\"\n".to_vec(),
            Providers(&[]),
            false,
            &mut BTreeMap::new(),
            &mut BTreeSet::new(),
        );
        assert_eq!(report.what, None);
        let mut findings = BTreeSet::new();
        report = check_file(
            None,
            "x.media.toml",
            vec![b'#'; MAX_FILE + 1],
            Providers(&[]),
            false,
            &mut BTreeMap::new(),
            &mut findings,
        );
        assert_eq!(report.what, None);
        assert!(findings
            .iter()
            .any(|f| f.message.contains("a recipe is at most 1048576")));
        let (source, found) = read("x", b"a = 1\nb = \"\xff\"\n".to_vec());
        assert!(source.is_none());
        assert_eq!(
            found[0].to_string(),
            "x:2:6: error[6.11.1] the file is not UTF-8"
        );
    }

    #[test]
    fn a_folder_walk_takes_recipe_and_work_files_and_skips_hidden_ones() {
        let dir = std::env::temp_dir().join(format!("recipe-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for path in [
            "a.media.toml",
            "b.work.toml",
            "c.firmware.toml",
            "d.machine.toml",
            "notes.toml",
            ".hidden.media.toml",
            ".git/e.media.toml",
            "x.media.toml.lock/f.media.toml",
            "sub/g.media.toml",
        ] {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let found: Vec<String> = files(std::slice::from_ref(&dir))
            .unwrap()
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().display().to_string())
            .collect();
        assert_eq!(
            found,
            [
                "a.media.toml",
                "b.work.toml",
                "c.firmware.toml",
                "d.machine.toml",
                "sub/g.media.toml"
            ]
        );
        // A file named itself is taken whatever its name.
        assert_eq!(files(&[dir.join("notes.toml")]).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_finding_is_reported_not_only_the_first() {
        let text = MINIMAL
            .replace("revision = 1", "revision = 1.0\nrevison = 2")
            .replace("size = 1\n", "size = \"1\"\n")
            .replace("label = \"Disk 1\"", "label = 5");
        let found = findings(&text);
        let rule_1: Vec<&String> = found.iter().filter(|f| f.contains("6.11.1")).collect();
        assert_eq!(rule_1.len(), 4, "{found:#?}");
    }
}
