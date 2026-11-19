// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pinning a recipe and the conformance harness (recipes §3.1, §6.12, §6.15,
//! §9).
//!
//! **Pin** ([`pin_path`], `upcli recipe pin`): the author's build, once per
//! revision. A draft is built and tagged as `recipe build` builds it, with
//! this build's writers; then its manifest, its config and every layer no
//! step made (the title metadata, a firmware package's README) are copied
//! from the store into the lock beside the recipe ([`crate::lock`]), and the
//! manifest digest is written into the recipe as `digest`, on the line
//! after `revision`. The recipe is then checked again, lock included. A
//! recipe that already has a `digest` is not pinned again: an artifact that
//! changes is a new revision. A pin that fails after the build restores the
//! recipe file and any lock the draft had beside it. The tag the build
//! wrote stays: it names exactly the artifact `recipe build` makes of the
//! draft, and a revision tag is never removed or moved.
//!
//! **Conformance** ([`conformance`], `upcli recipe conformance`): every
//! pinned recipe of the files and folders named is built into a scratch
//! store from the libraries given, and the digest it publishes is compared
//! with its `digest`. A build of the program conforms when every pinned
//! recipe it can find the inputs of builds its digest; this is how a
//! release proves it still builds what earlier releases pinned (recipes §9),
//! and how a repository of recipes checks a release against its own
//! library.

use crate::build::{build_path, Outcome, Report, Request};
use crate::library::Library;
use crate::model::Document;
use crate::Providers;
use image_store::Store;
use std::io;
use std::path::{Path, PathBuf};

/// What [`pin_path`] gives.
#[derive(Debug)]
pub enum Pinned {
    /// The recipe already states a `digest`.
    Already(String),
    /// The draft was not built: the build's own outcome, nothing written
    /// beside the recipe.
    NotBuilt(Box<Outcome>),
    /// Built, locked and pinned.
    Pinned {
        report: Box<Report>,
        digest: String,
        lock: PathBuf,
        /// How many files the lock holds.
        files: usize,
    },
}

/// Pin the draft at `request.path`: build it (`request.keep_as` is ignored),
/// write its lock and its digest. An I/O error is a file that cannot be
/// read or written; a recipe that fails its checks after the digest was
/// written is restored and its lock removed, and that is an error too.
pub fn pin_path(mut request: Request<'_>, log: &mut dyn FnMut(&str)) -> io::Result<Pinned> {
    let path = request.path;
    if let Some(Document::Recipe(recipe)) = crate::document(path)? {
        if let Some(digest) = &recipe.identity.digest {
            return Ok(Pinned::Already(digest.value.clone()));
        }
    }
    request.keep_as = None;
    let providers = request.providers;
    let store = request.store;
    let report = match build_path(request, log)? {
        Outcome::Built(report) => report,
        other => return Ok(Pinned::NotBuilt(Box::new(other))),
    };
    let other = |e: String| io::Error::other(e);
    let descriptor = &report.published.descriptor;
    let files = crate::lock::files_of(store, descriptor, &report.outputs).map_err(other)?;
    let lock = crate::lock::dir(path);
    let before = std::fs::read(path)?;
    let text = String::from_utf8(before.clone())
        .map_err(|_| other(format!("{}: not UTF-8", path.display())))?;
    let Some(Document::Recipe(recipe)) = crate::document(path)? else {
        return Err(crate::changed(path));
    };
    let Some(revision) = &recipe.identity.revision else {
        return Err(crate::changed(path));
    };
    let digest = descriptor.digest.to_string();
    let pinned = crate::lock::with_digest(&text, &revision.span, &digest);
    // A lock a draft had beside it is set aside, and put back if the pin
    // fails.
    let mut backup = lock.as_os_str().to_owned();
    backup.push(format!(".pin-{}", std::process::id()));
    let backup = PathBuf::from(backup);
    let replaced = lock.exists();
    if replaced {
        std::fs::rename(&lock, &backup)?;
    }
    let restore = |error: String| -> io::Error {
        let _ = std::fs::write(path, &before);
        let _ = std::fs::remove_dir_all(&lock);
        if replaced {
            let _ = std::fs::rename(&backup, &lock);
        }
        other(error)
    };
    crate::lock::write(&lock, &files).map_err(|e| restore(e.to_string()))?;
    let count = files.len();
    replace(path, pinned.as_bytes()).map_err(|e| restore(e.to_string()))?;
    let check =
        crate::check_paths(&[path.to_path_buf()], providers).map_err(|e| restore(e.to_string()))?;
    if !check.is_clean() {
        return Err(restore(format!(
            "the pinned recipe does not pass its checks; it was restored:\n{}",
            check.render()
        )));
    }
    if replaced {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(Pinned::Pinned {
        report,
        digest,
        lock,
        files: count,
    })
}

/// Write `bytes` to `path` through a file beside it, renamed into place.
fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".pin-{}", std::process::id()));
    let temporary = PathBuf::from(name);
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

/// How one recipe fared in [`conformance`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conformed {
    /// A draft: nothing to compare, not built.
    Draft,
    /// Built to the digest it pins.
    Conforms { reference: String, digest: String },
    /// Its payloads or its lock disagree: the mismatches.
    Differs(Vec<String>),
    /// An input is missing from the libraries: the plan's summary.
    Missing(String),
    /// The checks refuse it, or the build failed: why.
    Failed(String),
}

/// One recipe's line of a conformance run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conformance {
    pub file: String,
    pub result: Conformed,
}

/// Build every pinned recipe among the files and folders named into
/// `scratch`, a store of its own, from `libraries` only (no download, no
/// other store), and compare each digest with its pin. Work files are
/// skipped; drafts are listed and not built.
pub fn conformance(
    paths: &[PathBuf],
    providers: Providers<'_>,
    scratch: &Store,
    libraries: &mut [Library],
) -> io::Result<Vec<Conformance>> {
    let mut out = Vec::new();
    for file in crate::files(paths)? {
        let name = file.display().to_string();
        let digest = match crate::document(&file)? {
            Some(Document::Work(_)) => continue,
            Some(Document::Recipe(recipe)) => match recipe.identity.digest {
                Some(digest) => digest.value,
                None => {
                    out.push(Conformance {
                        file: name,
                        result: Conformed::Draft,
                    });
                    continue;
                }
            },
            None => {
                out.push(Conformance {
                    file: name,
                    result: Conformed::Failed("it does not read as a recipe".into()),
                });
                continue;
            }
        };
        let outcome = build_path(
            Request {
                path: &file,
                providers,
                store: scratch,
                index: None,
                store_read: None,
                libraries: &mut *libraries,
                fetcher: None,
                keep_as: None,
                // Conformance proves the steps still make the pinned
                // bytes, so nothing is found instead of made.
                find_outputs: false,
            },
            &mut |_| {},
        )?;
        let result = match outcome {
            Outcome::Built(report) if report.published.descriptor.digest.to_string() == digest => {
                Conformed::Conforms {
                    reference: report.reference,
                    digest,
                }
            }
            Outcome::Built(report) => Conformed::Differs(vec![format!(
                "built {}, the recipe pins {digest}",
                report.published.descriptor.digest
            )]),
            Outcome::Mismatch { mismatches, .. } => Conformed::Differs(mismatches),
            Outcome::Missing(plan) => Conformed::Missing(plan.summary()),
            Outcome::Refused(report) => Conformed::Failed(
                report
                    .findings
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Outcome::NotRecipe => continue,
            Outcome::Failed { error, .. } => Conformed::Failed(error),
        };
        out.push(Conformance { file: name, result });
    }
    Ok(out)
}

/// The text `upcli recipe conformance` prints: a line per recipe, then a
/// total. `true` when every pinned recipe conforms.
pub fn render(results: &[Conformance]) -> (String, bool) {
    let mut out = String::new();
    let (mut conform, mut differ, mut other, mut drafts) = (0, 0, 0, 0);
    for result in results {
        let line = match &result.result {
            Conformed::Draft => {
                drafts += 1;
                "draft, not built".to_owned()
            }
            Conformed::Conforms { reference, digest } => {
                conform += 1;
                format!("conforms: {reference}\t{digest}")
            }
            Conformed::Differs(mismatches) => {
                differ += 1;
                format!("differs: {}", mismatches.join("; "))
            }
            Conformed::Missing(summary) => {
                other += 1;
                format!("not built: {summary}")
            }
            Conformed::Failed(error) => {
                other += 1;
                format!("not built: {error}")
            }
        };
        out.push_str(&format!("{}: {line}\n", result.file));
    }
    let pinned = conform + differ + other;
    out.push_str(&format!(
        "{pinned} pinned recipe{}: {conform} conform, {differ} differ, {other} not built; {drafts} draft{} skipped\n",
        if pinned == 1 { "" } else { "s" },
        if drafts == 1 { "" } else { "s" },
    ));
    (out, differ == 0 && other == 0)
}
