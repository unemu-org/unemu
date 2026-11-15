// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image template build`: the official template images and installation
//! sets, built reproducibly from the fetched open fixtures; machine-images
//! §5.5.
//!
//! The build runs [`pc_image::template::build`] and then
//! [`pc_image::template::build_media`] with the push guard of the
//! official namespace on the default registry: every image must be all open,
//! each copyleft ROM or medium with its source archive, before any of its
//! tags is written. It never pushes; publishing is the maintainer's step
//! (commands.md, Official template images). Each image's lines are written as
//! soon as it is built; every installation-set fixture is checked against its
//! pin before the first tag. `--dry-run` builds into a scratch store under the
//! system temporary directory, removes it, and reports what a build would do
//! to the tags of `--store`, which is never written; when that store exists,
//! its marks must pass the guard as well.

use image_registry::{
    guard,
    reference::{Reference, DEFAULT_REGISTRY, OFFICIAL_NAMESPACE},
    RegistryEntry,
};
use image_store::{Manifest, Store};
use pc_image::template::{self, Built, Kind, Outcome};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
pub struct TemplateCommand {
    store: PathBuf,
    vendor: PathBuf,
    dry_run: bool,
}

pub fn parse(args: &[OsString]) -> Result<TemplateCommand, &'static str> {
    if args.first().and_then(|a| a.to_str()) != Some("build") {
        return Err("expected image template build");
    }
    let (mut store, mut vendor, mut dry_run) = (None, None, false);
    let mut args = args[1..].iter();
    while let Some(option) = args.next() {
        if option == "--dry-run" && !dry_run {
            dry_run = true;
            continue;
        }
        let value = args
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing template option value")?;
        match option.to_str() {
            Some("--store") if store.is_none() => store = Some(PathBuf::from(value)),
            Some("--vendor") if vendor.is_none() => vendor = Some(PathBuf::from(value)),
            _ => return Err("unknown or duplicate template option"),
        }
    }
    Ok(TemplateCommand {
        store: store.ok_or("image commands require --store DIR")?,
        // The fixture scripts write under the repository's vendor directory.
        vendor: vendor.unwrap_or_else(|| PathBuf::from("vendor")),
        dry_run,
    })
}

/// The guard `image push` applies to `unemu/` on the default registry, with
/// the local marks of each of `stores` and the firmware catalogue: the image
/// must pass with every store's marks.
fn official_guard<'a>(
    stores: &'a [&'a Store],
) -> impl Fn(&str, &Manifest) -> Result<(), String> + 'a {
    let entry = RegistryEntry::builtin(DEFAULT_REGISTRY);
    move |reference, manifest| {
        let target = Reference::parse(reference, DEFAULT_REGISTRY).map_err(|e| e.to_string())?;
        if target.namespace() != OFFICIAL_NAMESPACE {
            return Err(format!("{target} is outside the official namespace"));
        }
        for store in stores {
            let marks =
                super::registry::guard_marks(store, manifest, pc_image::firmware::catalogue())
                    .map_err(|e| e.to_string())?;
            let closure = guard::closure(manifest, &marks);
            guard::check(Some(&entry), &target, &closure).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Build every template (firmware packages and machines, not the
/// installation sets) into `store`, refusing any image the official
/// namespace's push guard would refuse.
pub fn build(store: &Store, vendor: &Path) -> anyhow::Result<Vec<Built>> {
    Ok(template::build(
        store,
        vendor,
        &official_guard(&[store]),
        &mut |_| {},
    )?)
}

/// A store under the system temporary directory, removed when dropped.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Firmware => "firmware",
        Kind::Machine => "machine",
        Kind::Media => "media",
    }
}

/// An outcome as the build reports it, or as a dry run predicts it.
fn word(outcome: &Outcome, dry_run: bool) -> String {
    match (outcome, dry_run) {
        (Outcome::Created, false) => "created".into(),
        (Outcome::Created, true) => "new".into(),
        (Outcome::Present, _) => "present".into(),
        (Outcome::Moved(from), false) => format!("moved:{from}"),
        (Outcome::Moved(from), true) => format!("moves:{from}"),
        (Outcome::Kept(current), _) => format!("kept:{current}"),
        (Outcome::Conflict(current), _) => format!("conflict:{current}"),
    }
}

/// One line per tag, written as soon as its image is built.
fn report(
    output: &mut impl Write,
    image: &Built,
    tags: &[(String, Outcome)],
    dry_run: bool,
) -> std::io::Result<()> {
    for (tag, outcome) in tags {
        writeln!(
            output,
            "{tag}\t{}\t{}\t{}",
            image.descriptor.digest,
            kind(image.kind),
            word(outcome, dry_run)
        )?;
    }
    output.flush()
}

pub fn execute(command: TemplateCommand, output: &mut impl Write) -> anyhow::Result<()> {
    // The installation sets' fixtures are checked before the first tag, so a
    // missing set never leaves a build half written.
    template::check_media_fixtures(&command.vendor)?;
    let mut failed: Option<anyhow::Error> = None;
    if !command.dry_run {
        let store = Store::open(&command.store)?;
        let stores = [&store];
        let guard = official_guard(&stores);
        let mut on_built = |image: &Built| {
            if failed.is_none() {
                failed = report(output, image, &image.tags, false)
                    .err()
                    .map(anyhow::Error::from);
            }
        };
        template::build(&store, &command.vendor, &guard, &mut on_built)?;
        template::build_media(&store, &command.vendor, &guard, &mut on_built)?;
        if let Some(error) = failed {
            return Err(error);
        }
        writeln!(output, "guard:allowed")?;
        return Ok(());
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "unemupc98-template-dry-run-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    let staging = Store::open(&scratch.0)?;
    let target = if command.store.join("store.toml").exists() {
        Some(Store::open_existing(&command.store)?)
    } else {
        None
    };
    // The marks of the store a real build would write count as well.
    let stores: Vec<&Store> = std::iter::once(&staging).chain(target.as_ref()).collect();
    let guard = official_guard(&stores);
    let mut on_built = |image: &Built| {
        if failed.is_some() {
            return;
        }
        let planned = match &target {
            Some(store) => template::plan_tags(store, image).map_err(anyhow::Error::from),
            None => Ok(image.tags.clone()),
        };
        failed = match planned {
            Ok(tags) => report(output, image, &tags, true).err().map(Into::into),
            Err(error) => Some(error),
        };
    };
    template::build(&staging, &command.vendor, &guard, &mut on_built)?;
    template::build_media(&staging, &command.vendor, &guard, &mut on_built)?;
    if let Some(error) = failed {
        return Err(error);
    }
    writeln!(output, "guard:allowed")?;
    writeln!(output, "dry run; no blobs or tag written")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parsing_needs_build_and_a_store_and_defaults_the_vendor_directory() {
        assert_eq!(
            parse(&os(&["build", "--store", "s", "--dry-run"])),
            Ok(TemplateCommand {
                store: "s".into(),
                vendor: "vendor".into(),
                dry_run: true,
            })
        );
        assert_eq!(
            parse(&os(&["build", "--vendor", "v", "--store", "s"])).map(|c| c.vendor),
            Ok("v".into())
        );
        for bad in [
            &["build"][..],
            &["ls", "--store", "s"],
            &["build", "--store", "s", "--store", "t"],
            &["build", "--store", "s", "--dry-run", "--dry-run"],
            &["build", "--store"],
            &["build", "--store", "s", "--push"],
        ] {
            assert!(parse(&os(bad)).is_err(), "{bad:?}");
        }
    }
}
