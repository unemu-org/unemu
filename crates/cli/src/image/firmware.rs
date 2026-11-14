// SPDX-License-Identifier: AGPL-3.0-or-later
//! Firmware package import and inspection; machine-images §5.3.
//! Folder hashing shares the persistent ROM index; sources remain untouched.

use image_store::{
    firmware::{Config, Validator},
    media_type,
    summary::{ArtifactKind, SummaryCache},
    Store,
};
use pc_image::{
    firmware::{catalogue, PcFirmwareRules},
    firmware_import::{self, Attachment},
};
use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub struct FirmwareCommand {
    action: String,
    store: PathBuf,
    files: Vec<PathBuf>,
    tag: String,
    create: bool,
    dry_run: bool,
    rescan: bool,
    selected: Option<String>,
    extras: Vec<(PathBuf, Option<String>)>,
}
pub fn parse(args: &[OsString]) -> Result<FirmwareCommand, &'static str> {
    let action = args
        .first()
        .and_then(|s| s.to_str())
        .filter(|s| matches!(*s, "import" | "ls" | "show"))
        .ok_or("expected image firmware import, ls or show")?;
    let mut command = FirmwareCommand {
        action: action.into(),
        store: PathBuf::new(),
        files: Vec::new(),
        tag: String::new(),
        create: false,
        dry_run: false,
        rescan: false,
        selected: None,
        extras: Vec::new(),
    };
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        if arg.is_empty() {
            return Err("empty firmware argument");
        }
        if !arg.as_encoded_bytes().starts_with(b"--") {
            if action == "import" {
                command.files.push(arg.into());
            } else if action == "show" && command.tag.is_empty() {
                command.tag = arg.to_str().ok_or("invalid firmware tag")?.into();
            } else {
                return Err("unexpected firmware argument");
            }
            continue;
        }
        match arg.to_str() {
            Some("--create") if action == "import" && !command.create => {
                command.create = true;
                continue;
            }
            Some("--dry-run") if action == "import" && !command.dry_run => {
                command.dry_run = true;
                continue;
            }
            Some("--rescan") if action == "import" && !command.rescan => {
                command.rescan = true;
                continue;
            }
            _ => (),
        }
        let value = args
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing firmware option value")?;
        match arg.to_str() {
            Some("--store") if command.store.as_os_str().is_empty() => command.store = value.into(),
            Some("--for") if action == "import" && command.selected.is_none() => {
                command.selected = Some(value.to_str().ok_or("invalid catalogue id")?.into())
            }
            Some("--extra") if action == "import" => {
                let text = value.to_str().ok_or("invalid extra path")?;
                let (path, name) = text
                    .rsplit_once(':')
                    .filter(|(p, _)| !(p.len() == 1 && p.as_bytes()[0].is_ascii_alphabetic()))
                    .map_or((text, None), |(p, n)| (p, Some(n.to_owned())));
                if path.is_empty() {
                    return Err("invalid extra path");
                }
                command.extras.push((path.into(), name));
            }
            _ => return Err("unknown or duplicate firmware option"),
        }
    }
    if action == "import" && command.files.is_empty() {
        return Err("firmware import requires a folder or files");
    }
    if action == "show" && command.tag.is_empty() {
        return Err("firmware show requires NAME:TAG");
    }
    if !command.extras.is_empty() && command.selected.is_none() {
        return Err("--extra requires --for CATALOGUE-ID");
    }
    if command
        .selected
        .as_ref()
        .is_some_and(|id| !catalogue().iter().any(|e| e.id == id))
    {
        return Err("unknown firmware catalogue id");
    }
    Ok(command)
}
/// Without `--store`, the store is the default the executable supplies; library
/// callers that supply none get an error instead of a host location.
pub fn execute(
    mut command: FirmwareCommand,
    output: &mut impl Write,
    sources: &crate::command::FirmwareSources,
) -> anyhow::Result<()> {
    if command.store.as_os_str().is_empty() {
        command.store = sources
            .store_root
            .clone()
            .ok_or_else(|| anyhow::anyhow!("default store unavailable; use --store DIR"))?;
    }
    if command.action == "import" {
        let control = assembly::firmware_discovery::DiscoveryControl::default();
        let cache = if command.dry_run {
            None
        } else {
            sources.index.as_deref()
        };
        return assembly::firmware_discovery::with_discovery_session(
            cache,
            command.rescan,
            &control,
            || import(command, output, catalogue()),
        );
    }
    let store = Store::open_existing(&command.store)?;
    if command.action == "show" {
        let image = store.resolve(&command.tag, &Validator(&PcFirmwareRules(catalogue())))?;
        let config = Config::decode(&image.config)?;
        writeln!(
            output,
            "{}\t{}\t{:?}",
            command.tag, image.image.digest, config.name
        )?;
        writeln!(output, "fits\t{:?}", config.fits)?;
        for rom in &config.roms {
            writeln!(
                output,
                "{}\t{}\t{}\t{}",
                rom.id, rom.role, rom.size, rom.sha256
            )?;
            for p in &rom.parts {
                writeln!(
                    output,
                    "chip\t{:?}\t{}\t{}\t{}",
                    p.position, p.lane, p.offset, p.blob
                )?;
            }
        }
        for e in &config.extras {
            writeln!(output, "extra\t{:?}\t{}\t{}", e.filename, e.size, e.blob)?;
        }
    } else {
        let mut summaries = SummaryCache::open(&store)?;
        while !summaries.pending().is_empty() {
            summaries.rebuild(&store, 64, &[], &[])?;
        }
        for tag in summaries.tags() {
            if let Some(row) = summaries
                .summary(&tag.manifest.digest)
                .filter(|s| s.kind == ArtifactKind::Firmware)
            {
                writeln!(
                    output,
                    "{}\t{}\t{:?}\t{:?}",
                    tag.name, tag.manifest.digest, row.name, row.firmware
                )?;
            }
        }
        for tag in summaries.tags() {
            if let Some(failure) = summaries.failure(&tag.manifest.digest) {
                writeln!(
                    output,
                    "unavailable\t{}\t{:?}",
                    failure.image, failure.reason
                )?;
            }
        }
    }
    Ok(())
}
fn import(
    command: FirmwareCommand,
    output: &mut impl Write,
    catalogue: &[assembly::FirmwareEntry],
) -> anyhow::Result<()> {
    let scan = firmware_import::scan(&command.files, catalogue)?;
    if scan.size_mismatches + scan.hash_mismatches > 0 {
        writeln!(
            output,
            "unmatched: {} files ({} size mismatches, {} SHA-256 mismatches)",
            scan.size_mismatches + scan.hash_mismatches,
            scan.size_mismatches,
            scan.hash_mismatches
        )?;
    }
    for issue in scan.issues {
        writeln!(output, "{:?}: {}", issue.source, issue.reason)?;
    }
    let candidates = scan
        .candidates
        .into_iter()
        .filter(|c| {
            command
                .selected
                .as_ref()
                .is_none_or(|id| id == &c.catalogue_id)
        })
        .collect::<Vec<_>>();
    if command.selected.is_some() && candidates.is_empty() {
        anyhow::bail!("selected catalogue entry has no complete unambiguous package");
    }
    let mut eligible = Vec::new();
    for candidate in candidates {
        if command.selected.is_none()
            && catalogue.iter().any(|entry| {
                entry.id != candidate.catalogue_id
                    && entry.sha256 == candidate.config.roms[0].sha256
                    && entry.size == candidate.config.roms[0].size
            })
        {
            writeln!(
                output,
                "{}: ambiguous catalogue identities; select --for CATALOGUE-ID",
                candidate.catalogue_id
            )?;
        } else {
            eligible.push(candidate);
        }
    }
    if eligible.is_empty() {
        if command.dry_run {
            writeln!(output, "dry run; no blobs or tag written")?;
        }
        return Ok(());
    }
    let store = if command.dry_run && !command.store.join("store.toml").exists() {
        None
    } else if command.create && !command.dry_run {
        Some(Store::open(&command.store)?)
    } else {
        Some(Store::open_existing(&command.store)?)
    };
    let extras = command
        .extras
        .into_iter()
        .map(|(path, dos_name)| Attachment { path, dos_name })
        .collect::<Vec<_>>();
    for extra in &extras {
        anyhow::ensure!(
            std::fs::metadata(&extra.path)?.is_file(),
            "extra must be a regular file"
        );
        if let Some(name) = &extra.dos_name {
            use image_store::media_set::MediaFormats;
            pc_image::media_set::PcMediaFormats.validate_name(name)?;
        }
    }
    let mut states = Vec::new();
    for candidate in &eligible {
        let manifest = firmware_import::preview_manifest(candidate, &extras)?;
        states.push(match &store {
            Some(store) => {
                firmware_import::already_imported(store, &candidate.tag(), &manifest, catalogue)?
            }
            None => false,
        });
    }
    for (c, existing) in eligible.into_iter().zip(states) {
        if existing {
            writeln!(output, "already imported\t{}", c.tag())?;
            continue;
        }
        writeln!(output, "complete\t{}\t{}", c.tag(), c.config.roms[0].sha256)?;
        if !command.dry_run {
            let d = firmware_import::publish(
                store
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("store unavailable"))?,
                &c,
                &extras,
                catalogue,
            )?;
            anyhow::ensure!(
                d.media_type == media_type::MANIFEST,
                "invalid package descriptor"
            );
            writeln!(output, "imported\t{}\t{}", c.tag(), d.digest)?;
        }
    }
    if command.dry_run {
        writeln!(output, "dry run; no blobs or tag written")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn entry(id: &'static str, byte: u8) -> assembly::FirmwareEntry {
        let hash = Box::leak(
            image_store::Digest::of(&vec![byte; 32768])
                .hex()
                .to_owned()
                .into_boxed_str(),
        );
        assembly::FirmwareEntry {
            id,
            sha256: hash,
            size: 32768,
            distribution: assembly::FirmwareDistribution::UserSupplied,
            ..assembly::KISELEV_XI8088
        }
    }
    fn command(root: &Path, files: &Path) -> FirmwareCommand {
        FirmwareCommand {
            action: "import".into(),
            store: root.into(),
            files: vec![files.into()],
            tag: String::new(),
            create: true,
            dry_run: false,
            rescan: false,
            selected: None,
            extras: Vec::new(),
        }
    }
    #[test]
    fn folder_reimport_keeps_existing_packages_and_publishes_new_ones() {
        let temp = assembly::test_support::TempDir::new("firmware-folder-reimport");
        let root = temp.path();
        let files = root.join("roms");
        std::fs::create_dir(&files).unwrap();
        let a = files.join("a");
        std::fs::write(&a, vec![0x11; 32768]).unwrap();
        std::fs::write(files.join("b"), vec![0x22; 32768]).unwrap();
        let cat = [
            entry("synthetic-first", 0x11),
            entry("synthetic-second", 0x22),
        ];
        let store = Store::open(root.join("store")).unwrap();
        let scan = firmware_import::scan(&[a], &cat).unwrap();
        firmware_import::publish(&store, &scan.candidates[0], &[], &cat).unwrap();
        let mut out = Vec::new();
        import(command(store.root(), &files), &mut out, &cat).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("already imported\tfirmware/synthetic-first:1"));
        assert!(out.contains("imported\tfirmware/synthetic-second:1"));
        assert_eq!(store.list_tags(None, 10).unwrap().tags.len(), 2);
    }
    #[test]
    fn ambiguity_is_skipped_before_any_existing_tag_precheck() {
        let temp = assembly::test_support::TempDir::new("firmware-ambiguity-precheck");
        let root = temp.path();
        let file = root.join("rom");
        std::fs::write(&file, vec![0x11; 32768]).unwrap();
        let cat = [
            entry("synthetic-first", 0x11),
            entry("synthetic-alias", 0x11),
        ];
        let store = Store::open(root.join("store")).unwrap();
        let scan = firmware_import::scan(std::slice::from_ref(&file), &cat[..1]).unwrap();
        firmware_import::publish(&store, &scan.candidates[0], &[], &cat).unwrap();
        let mut out = Vec::new();
        import(command(store.root(), &file), &mut out, &cat).unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("ambiguous catalogue identities"));
        assert_eq!(store.list_tags(None, 10).unwrap().tags.len(), 1);
    }
}
