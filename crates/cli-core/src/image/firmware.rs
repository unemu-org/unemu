// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image firmware ls` and `show`: the firmware packages of a store, and one
//! package in full; machine-images §5.3. A package is read with its
//! platform's validator, and the push line is the guard's own
//! classification with the platform's restrictions
//! ([`super::super::CliPlatform`]). Without `--store`, the store is the
//! default the executable supplies; library callers that supply none get an
//! error instead of a host location.

use super::super::{CliPlatform, HostSources};
use image_registry::guard;
use image_store::{
    firmware::{Config, ProvenanceKind},
    license, media_type,
    summary::{ArtifactKind, SummaryCache},
    Store,
};
use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum FirmwareCommand {
    /// `store` is `None` without `--store`.
    List {
        store: Option<PathBuf>,
    },
    Show {
        store: Option<PathBuf>,
        tag: String,
    },
}

/// Parse the words after `image firmware`: `Ok(None)` for `import` and
/// `edit`, a platform's.
pub fn parse(args: &[OsString]) -> Result<Option<FirmwareCommand>, &'static str> {
    let action = args
        .first()
        .and_then(|s| s.to_str())
        .filter(|s| matches!(*s, "import" | "ls" | "show" | "edit"))
        .ok_or("expected image firmware import, edit, ls or show")?;
    if matches!(action, "import" | "edit") {
        return Ok(None);
    }
    let (mut store, mut tag) = (None, None);
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        if arg.is_empty() {
            return Err("empty firmware argument");
        }
        if !arg.as_encoded_bytes().starts_with(b"--") {
            if action == "show" && tag.is_none() {
                tag = Some(arg.to_str().ok_or("invalid firmware tag")?.to_owned());
            } else {
                return Err("unexpected firmware argument");
            }
            continue;
        }
        let value = args
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing firmware option value")?;
        match arg.to_str() {
            Some("--store") if store.is_none() => store = Some(PathBuf::from(value)),
            _ => return Err("unknown or duplicate firmware option"),
        }
    }
    Ok(Some(match (action, tag) {
        ("show", None) => return Err("firmware show and edit require NAME:TAG"),
        ("show", Some(tag)) => FirmwareCommand::Show { store, tag },
        _ => FirmwareCommand::List { store },
    }))
}

pub fn execute(
    command: FirmwareCommand,
    output: &mut impl Write,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
) -> anyhow::Result<()> {
    let (store, tag) = match command {
        FirmwareCommand::List { store } => (store, None),
        FirmwareCommand::Show { store, tag } => (store, Some(tag)),
    };
    let store = match store {
        Some(store) => store,
        None => host
            .store_root
            .clone()
            .ok_or_else(|| anyhow::anyhow!("default store unavailable; use --store DIR"))?,
    };
    let store = Store::open_existing(&store)?;
    if let Some(tag) = tag {
        show(&store, &tag, output, platforms)?;
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
                    "{}\t{}\t{:?}\t{:?}{}",
                    tag.name,
                    tag.manifest.digest,
                    row.name,
                    row.firmware.as_ref().map(Listed),
                    super::requirement_columns(&store, &tag.manifest)
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

/// One package in full: identity, descriptive fields, ROM bindings, every
/// layer's licence and mark, and where its own marks allow it to be pushed.
fn show(
    store: &Store,
    tag: &str,
    output: &mut impl Write,
    platforms: &[&dyn CliPlatform],
) -> anyhow::Result<()> {
    let platform = super::super::platform_of(platforms)?;
    let validator = platform
        .validators()
        .into_iter()
        .find(|v| v.artifact_type() == media_type::FIRMWARE)
        .ok_or_else(|| anyhow::anyhow!("{} reads no firmware packages", platform.platform()))?;
    let image = store.resolve(tag, validator)?;
    let config = Config::decode(&image.config)?;
    writeln!(output, "{}\t{}\t{:?}", tag, image.image.digest, config.name)?;
    super::write_min_app_version(output, &image.manifest)?;
    writeln!(output, "fits\t{:?}", config.fits)?;
    for rom in &config.roms {
        writeln!(
            output,
            "{}\t{}\t{}\t{}",
            rom.id, rom.role, rom.size, rom.sha256
        )?;
        if !rom.requires.is_empty() {
            writeln!(output, "requires\t{}\t{:?}", rom.id, rom.requires)?;
        }
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
    for (key, value) in [
        ("vendor", &config.vendor),
        ("version", &config.version),
        ("date", &config.date),
        ("description", &config.description),
        ("notes", &config.notes),
    ] {
        if let Some(value) = value {
            writeln!(output, "{key}\t{value:?}")?;
        }
    }
    if !config.labels.is_empty() {
        writeln!(output, "labels\t{:?}", config.labels)?;
    }
    if let Some(p) = &config.provenance {
        writeln!(
            output,
            "provenance\t{}\t{:?}\trevision:{}\turl:{}\tdate:{}",
            p.kind.map_or("unstated", ProvenanceKind::as_str),
            p.from,
            p.revision.as_deref().unwrap_or("-"),
            p.url.as_deref().unwrap_or("-"),
            p.date.as_deref().unwrap_or("-")
        )?;
    }
    for source in &config.sources {
        writeln!(
            output,
            "source\t{:?}\t{}\t{}\t{}\t{:?}",
            source.filename,
            source.format.as_str(),
            source.size,
            source.blob,
            source.roms
        )?;
    }
    if let Some(readme) = &config.readme {
        writeln!(output, "readme\t{}\t{}", readme.blob, readme.size)?;
    }
    let class = |d: &image_store::Descriptor| match license::open_license(d) {
        Some(l) => format!("open\t{l}"),
        None => "user-supplied\t-".into(),
    };
    writeln!(
        output,
        "layer\tconfig\t{}\t{}",
        image.manifest.config.digest,
        class(&image.manifest.config)
    )?;
    for d in &image.manifest.layers {
        let kind = match d.decoded_media_type() {
            media_type::FIRMWARE => "rom",
            media_type::TAR | media_type::ZIP => "source",
            media_type::MARKDOWN => "readme",
            _ => "extra",
        };
        writeln!(
            output,
            "layer\t{kind}\t{}\t{}",
            d.decoded_digest()?,
            class(d)
        )?;
    }
    // The guard's own classification: local marks and the catalogue included.
    let marks = super::guard_marks(store, &image.manifest, platforms)?;
    let closure = guard::closure(&image.manifest, &marks);
    writeln!(
        output,
        "push\t{}",
        match guard::reach(&closure) {
            guard::Reach::UserSupplied => {
                "private registries outside unemu/ only: user-supplied layers"
            }
            guard::Reach::CopyleftWithoutSource => {
                "private registries outside unemu/ only: copyleft ROM without its source archive"
            }
            guard::Reach::Anywhere => "any configured registry, including unemu/",
        }
    )?;
    Ok(())
}

/// The `ls` facets column keeps the fields it was introduced with; the summary's
/// descriptive fields, ROM bindings and extras belong to `show`.
struct Listed<'a>(&'a image_store::summary::FirmwareSummary);
impl std::fmt::Debug for Listed<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirmwareSummary")
            .field("roles", &self.0.roles)
            .field("boards", &self.0.boards)
            .field("machine_classes", &self.0.machine_classes)
            .field("catalogue_ids", &self.0.catalogue_ids)
            .field("hashes", &self.0.hashes)
            .finish()
    }
}
