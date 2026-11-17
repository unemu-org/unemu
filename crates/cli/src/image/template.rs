// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image template build`: the official template machines, built
//! reproducibly from the fetched open fixtures; machine-images §5.5.
//!
//! The build runs [`pc_image::template::build`] with the push guard of the
//! official namespace on the default registry: every template must be all
//! open, each copyleft ROM with its source archive, before any of its tags is
//! written. The firmware packages it binds are imported from the fixtures
//! first, as local copies of the published packages under their published
//! tags; they are neither judged nor reported apart from the templates. The
//! firmware packages and the installation sets are published by the
//! unemu-media repository; `image pull` fetches them. The build never pushes;
//! publishing is the maintainer's step (commands.md, Official template
//! images). Each template's lines are written as soon as it is built.
//! `--dry-run` builds into a scratch store under the system temporary
//! directory, removes it, and reports what a build would do to the template
//! tags of `--store`, which is never written; when that store exists, its
//! marks must pass the guard as well, and a package tag of it that names
//! another image refuses the dry run as it would the build.

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
/// must pass with every store's marks, and declare at least the release the
/// PC tables compute from its config.
fn official_guard<'a>(
    stores: &'a [&'a Store],
) -> impl Fn(&str, &Manifest, &[u8]) -> Result<(), String> + 'a {
    let entry = RegistryEntry::builtin(DEFAULT_REGISTRY);
    move |reference, manifest, config| {
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
        let required = pc_image::compat::release(manifest, config).map_err(|e| e.to_string())?;
        guard::check_release(&target, manifest, required).map_err(|e| e.to_string())
    }
}

/// Build every template machine into `store`, with local copies of the
/// firmware packages it binds, refusing any template the official
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
        if let Some(error) = failed {
            return Err(error);
        }
        writeln!(output, "guard:allowed")?;
        return Ok(());
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "unemupc-template-dry-run-{}-{}",
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
    let mut packages_checked = false;
    let mut on_built = |image: &Built| {
        if failed.is_some() {
            return;
        }
        // The packages are in the scratch store before the first template. A
        // real build would stop at a package tag of the store that names
        // another image before it writes any template, so the dry run stops
        // there too, before its first line.
        if let (false, Some(store)) = (packages_checked, &target) {
            packages_checked = true;
            if let Err(error) = template::check_package_tags(store, &staging) {
                failed = Some(error.into());
                return;
            }
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

    /// The official guard also refuses a manifest that does not declare the
    /// release the PC tables compute, as a push into `unemu/` does.
    #[test]
    fn the_official_guard_checks_the_declared_release() {
        use image_store::{firmware, license, media_type as mt, Descriptor, Digest};
        let temp = assembly::test_support::TempDir::new("template-guard-release");
        let store = Store::open(temp.path().join("store")).unwrap();
        let rom = [0x5a_u8; 2048];
        let digest = Digest::of(&rom);
        let config = firmware::Config {
            schema: 1,
            platform: pc_image::PLATFORM.into(),
            name: "Synthetic ROM".into(),
            vendor: None,
            version: None,
            date: None,
            notes: None,
            description: None,
            labels: Vec::new(),
            fits: toml::Table::new(),
            provenance: None,
            roms: vec![firmware::Rom {
                id: "synthetic".into(),
                role: "option-rom".into(),
                catalogue_id: None,
                sha256: digest.hex().into(),
                size: rom.len() as u64,
                blob: Some(digest.clone()),
                position: None,
                part_number: None,
                requires: Vec::new(),
                parts: Vec::new(),
            }],
            sources: Vec::new(),
            extras: Vec::new(),
            readme: None,
        };
        let bytes = config.encode().unwrap();
        let open = |mut d: Descriptor| {
            d.annotations
                .insert(license::REDISTRIBUTION.into(), license::OPEN.into());
            d.annotations.insert(license::LICENSE.into(), "MIT".into());
            d
        };
        let mut manifest = Manifest::new_firmware(
            pc_image::PLATFORM,
            open(Descriptor::new(
                mt::FIRMWARE_CONFIG,
                Digest::of(&bytes),
                bytes.len() as u64,
            )),
            vec![open(Descriptor::new(
                mt::FIRMWARE,
                digest,
                rom.len() as u64,
            ))],
        )
        .unwrap();
        let stores = [&store];
        let guard = official_guard(&stores);
        // The platform id is first read by 0.0.3, so the bare manifest is
        // refused until it declares that release.
        let refusal = guard("unemu/firmware/ibm-pc/synthetic:r1", &manifest, &bytes).unwrap_err();
        assert!(refusal.contains("unemu.min-app-version"), "{refusal}");
        pc_image::compat::stamp(&mut manifest, &bytes, image_store::compat::BASELINE, None)
            .unwrap();
        guard("unemu/firmware/ibm-pc/synthetic:r1", &manifest, &bytes).unwrap();
        manifest
            .annotations
            .insert(image_store::compat::MIN_APP_VERSION.into(), "soon".into());
        let refusal = guard("unemu/firmware/ibm-pc/synthetic:r1", &manifest, &bytes).unwrap_err();
        assert!(refusal.contains("unemu.min-app-version"), "{refusal}");
    }

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
