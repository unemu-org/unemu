// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pinning a recipe and the conformance harness (recipes §3.1, §6.12, §6.15,
//! §9).
//!
//! **Pin** ([`pin_path`], `unemu recipe pin`): the author's build, once per
//! revision. A draft is built and tagged as `recipe build` builds it, with
//! this build's writers; then its manifest, its config and every layer no
//! step made (the title metadata, a firmware package's README) are copied
//! from the store into the lock beside the recipe ([`crate::lock`]), and the
//! manifest digest is written into the recipe as `digest`, on the line
//! after `version` for machines or `revision` for packages. The recipe is
//! then checked again, lock included. A recipe that already has a `digest`
//! is not pinned again: a changed artifact needs a new version or revision. A pin that fails after the build restores the
//! recipe file and any lock the draft had beside it. A package's build tag
//! stays: it names exactly what the draft builds. A machine stages its
//! artifact without tagging, then tags only after the lock and pinned text
//! pass their checks. Immutable output tags are never removed or moved.
//!
//! **Conformance** ([`conformance`], `unemu recipe conformance`): every
//! pinned recipe of the files and folders named is built into a scratch
//! store from the libraries given, and the digest it publishes is compared
//! with its `digest`. A machine recipe is composed after the package
//! recipes named with it, whose builds hold the packages its uses pin. A
//! build of the program conforms when every pinned recipe it can find the
//! inputs of builds its digest; this is how a release proves it still
//! builds what earlier releases pinned (recipes §9), and how a repository
//! of recipes checks a release against its own library.

use crate::build::{build_path, Outcome, Report, Request};
use crate::library::Library;
use crate::model::{Document, Kind};
use crate::Providers;
use image_store::Store;
use std::io;
use std::path::{Path, PathBuf};
use unemu_core_api::Core;

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
pub fn pin_path(request: Request<'_>, log: &mut dyn FnMut(&str)) -> io::Result<Pinned> {
    pin_with(request, log, build_path)
}

/// Pin with the caller's build dispatch, including its selected template core.
pub(crate) fn pin_with<'a>(
    mut request: Request<'a>,
    log: &mut dyn FnMut(&str),
    build: impl FnOnce(Request<'a>, &mut dyn FnMut(&str)) -> io::Result<Outcome>,
) -> io::Result<Pinned> {
    let path = request.path;
    if let Some(Document::Recipe(recipe)) = crate::document(path)? {
        if let Some(digest) = &recipe.identity.digest {
            return Ok(Pinned::Already(digest.value.clone()));
        }
    }
    request.keep_as = None;
    let providers = request.providers;
    let store = request.store;
    let mut report = match build(request, log)? {
        Outcome::Built(report) => report,
        other => return Ok(Pinned::NotBuilt(Box::new(other))),
    };
    let other = |e: String| io::Error::other(e);
    let descriptor = report.published.descriptor.clone();
    let files = crate::lock::files_of(store, &descriptor, &report.outputs).map_err(other)?;
    let lock = crate::lock::dir(path);
    let before = std::fs::read(path)?;
    let text = String::from_utf8(before.clone())
        .map_err(|_| other(format!("{}: not UTF-8", path.display())))?;
    let Some(Document::Recipe(recipe)) = crate::document(path)? else {
        return Err(crate::changed(path));
    };
    let anchor = if recipe.kind == crate::model::Kind::Machine {
        recipe.identity.version.as_ref().map(|v| &v.span)
    } else {
        recipe.identity.revision.as_ref().map(|v| &v.span)
    }
    .ok_or_else(|| crate::changed(path))?;
    let digest = descriptor.digest.to_string();
    let pinned = crate::lock::with_digest(&text, anchor, &digest);
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
    if recipe.kind == crate::model::Kind::Machine {
        let family = recipe
            .identity
            .platform
            .as_ref()
            .map(|p| p.value.as_str())
            .unwrap_or_default();
        let publisher = providers
            .get(family)
            .and_then(|provider| provider.publisher())
            .ok_or_else(|| restore("no template publisher".into()))?;
        report.published = publisher
            .tag_locked(store, &report.reference, &descriptor)
            .map_err(restore)?;
    }
    if replaced {
        let _ = std::fs::remove_dir_all(&backup);
    }
    // The draft's text built the tag; the pinned text, which builds it
    // from now on, is the one recorded.
    crate::build::record(
        store,
        &report.reference,
        &report.published,
        &image_store::Digest::of(pinned.as_bytes()),
        log,
    );
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
/// skipped; drafts are listed and not built. A machine recipe is composed
/// with the core `core_for` names for its platform; the packages its uses
/// pin are found in `scratch` only, where the package recipes named with
/// it were built first. The results keep the order of the files.
pub fn conformance(
    paths: &[PathBuf],
    providers: Providers<'_>,
    core_for: &dyn Fn(&str) -> Option<&'static dyn Core>,
    scratch: &Store,
    libraries: &mut [Library],
) -> io::Result<Vec<Conformance>> {
    let mut out: Vec<Option<Conformance>> = Vec::new();
    let mut packages = Vec::new();
    let mut machines = Vec::new();
    for file in crate::files(paths)? {
        let name = file.display().to_string();
        let result = match crate::document(&file)? {
            Some(Document::Work(_)) => continue,
            Some(Document::Recipe(recipe)) => match recipe.identity.digest {
                Some(digest) => {
                    // A use names a package by its manifest digest only, so
                    // a machine is built after every package of the run.
                    let queue = if recipe.kind == Kind::Machine {
                        &mut machines
                    } else {
                        &mut packages
                    };
                    queue.push((out.len(), file, digest.value));
                    out.push(None);
                    continue;
                }
                None => Conformed::Draft,
            },
            None => Conformed::Failed("it does not read as a recipe".into()),
        };
        out.push(Some(Conformance { file: name, result }));
    }
    for (index, file, digest) in packages.into_iter().chain(machines) {
        let outcome = crate::template::build_path(
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
            core_for,
            None,
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
        out[index] = Some(Conformance {
            file: file.display().to_string(),
            result,
        });
    }
    Ok(out.into_iter().flatten().collect())
}

/// The text `unemu recipe conformance` prints: a line per recipe, then a
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
