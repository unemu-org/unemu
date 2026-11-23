// SPDX-License-Identifier: AGPL-3.0-or-later
//! Offline exports over the core contract; providers retain store snapshots.

mod sink;
pub use sink::FileSink;

use crate::{CliPlatform, HostSources};
use image_store::Store;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};
use unemu_core_api::{
    ConfigDocument, ContentReader, Core, Error, ErrorKind, ExportMedium, ExportReport,
    ExportRequest, ExportScope, MediaDescription, SlotId, SlotKind,
};

/// Store-owned config and current media, retained under workspace ownership.
pub trait Snapshot {
    fn config(&self) -> ConfigDocument<'_>;
    fn baseline(&self) -> anyhow::Result<Vec<MediaDescription>>;
    fn current(&self, label: &str) -> anyhow::Result<Box<dyn ContentReader + '_>>;
}

#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub store: Option<PathBuf>,
    pub format: Option<String>,
    pub slot: Option<String>,
    pub to: Option<PathBuf>,
}

pub fn parse(args: &[OsString]) -> Result<Command, &'static str> {
    let id = args
        .first()
        .and_then(|a| a.to_str())
        .filter(|s| !s.is_empty() && !s.starts_with("--"))
        .ok_or("export requires a machine ID; use unemu image machine ls")?
        .to_owned();
    let mut command = Command {
        id,
        store: None,
        format: None,
        slot: None,
        to: None,
    };
    let mut words = args[1..].iter();
    while let Some(option) = words.next() {
        let value = words
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing export option value")?;
        match option.to_str() {
            Some("--store") if command.store.is_none() => command.store = Some(value.into()),
            Some("--to") if command.to.is_none() => command.to = Some(value.into()),
            Some("--format") if command.format.is_none() => {
                command.format = Some(value.to_str().ok_or("invalid format")?.into())
            }
            Some("--slot") if command.slot.is_none() => {
                command.slot = Some(value.to_str().ok_or("invalid slot")?.into())
            }
            _ => return Err("unknown or duplicate export option"),
        }
    }
    if command.format.is_some() != command.to.is_some()
        || (command.slot.is_some() && command.format.is_none())
    {
        return Err("export requires --format FMT and --to DIR together");
    }
    Ok(command)
}

fn usage(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Usage, message)
}
fn unsupported(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Unsupported, message)
}

fn known_scope(scope: ExportScope) -> Option<&'static str> {
    match scope {
        ExportScope::Slot => Some("slot"),
        ExportScope::Machine => Some("machine"),
        _ => None,
    }
}

fn descriptions(
    core: &dyn Core,
    platform: &str,
    snapshot: &dyn Snapshot,
) -> anyhow::Result<Vec<MediaDescription>> {
    match core.describe_media(platform, snapshot.config()) {
        Ok(media) => Ok(media),
        Err(error) if error.kind == ErrorKind::Unsupported => snapshot.baseline(),
        Err(error) => Err(error.into()),
    }
}

/// Listing never creates a machine or interprets display-only detail text.
pub fn list(
    core: &dyn Core,
    platform: &str,
    snapshot: &dyn Snapshot,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    list_with_scopes(core, platform, snapshot, out, known_scope)
}

fn list_with_scopes(
    core: &dyn Core,
    platform: &str,
    snapshot: &dyn Snapshot,
    out: &mut dyn Write,
    scope_name: fn(ExportScope) -> Option<&'static str>,
) -> anyhow::Result<()> {
    let description = core
        .description()
        .platforms
        .iter()
        .find(|p| image_store::platform::same(p.id, platform))
        .ok_or_else(|| unsupported("export platform unavailable"))?;
    if description.export_formats.is_empty() {
        writeln!(out, "formats: unavailable")?;
    }
    for format in description.export_formats {
        let scope = scope_name(format.scope);
        let available = match scope {
            None => false,
            Some(_) => match core.export_slots(platform, snapshot.config(), format.id) {
                Ok(_) => true,
                Err(error) if error.kind == ErrorKind::Unsupported => false,
                Err(error) => return Err(error.into()),
            },
        };
        writeln!(
            out,
            "format:{}\t{}\t.{}\t{}\t{}",
            format.id,
            format.name,
            format.extension,
            scope.unwrap_or("unknown"),
            if available {
                "available"
            } else {
                "unavailable"
            }
        )?;
    }
    for media in descriptions(core, platform, snapshot)? {
        let kind = match media.slot.kind {
            SlotKind::Floppy => "floppy",
            SlotKind::FixedDisk => "fixed",
            SlotKind::Optical => "optical",
            SlotKind::Cartridge => "cartridge",
            SlotKind::Tape => "tape",
            _ => "unknown",
        };
        let size = media
            .size
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".into());
        writeln!(
            out,
            "{}\t{kind}\t{size}\t{}",
            media.slot.label, media.detail
        )?;
    }
    Ok(())
}

pub fn selected(
    core: &dyn Core,
    platform: &str,
    snapshot: &dyn Snapshot,
    format: &str,
    slot: Option<&str>,
) -> anyhow::Result<Vec<SlotId>> {
    let platform_description = core
        .description()
        .platforms
        .iter()
        .find(|p| image_store::platform::same(p.id, platform))
        .ok_or_else(|| unsupported("export platform unavailable"))?;
    if platform_description.export_formats.is_empty() {
        return Err(unsupported("export formats unavailable").into());
    }
    let format = platform_description
        .export_formats
        .iter()
        .find(|f| f.id == format)
        .ok_or_else(|| usage("unknown export format"))?;
    known_scope(format.scope).ok_or_else(|| unsupported("export scope unavailable"))?;
    let slots = core.export_slots(platform, snapshot.config(), format.id)?;
    if slots.iter().collect::<BTreeSet<_>>().len() != slots.len() {
        return Err(usage("core returned duplicate export slots").into());
    }
    match (format.scope, slot) {
        (ExportScope::Slot, Some(label)) => {
            let id = descriptions(core, platform, snapshot)?
                .into_iter()
                .find(|m| m.slot.label == label)
                .map(|m| m.slot.id)
                .ok_or_else(|| usage(format!("no slot {label}; omit --slot to list media")))?;
            if !slots.contains(&id) {
                return Err(usage("slot is unavailable for this format").into());
            }
            Ok(vec![id])
        }
        (ExportScope::Machine, None) => Ok(slots),
        _ => Err(usage("slot formats require --slot NAME; machine formats reject --slot").into()),
    }
}

/// Stop writes and validate receipts before publishing final names.
pub fn export(
    core: &dyn Core,
    platform: &str,
    snapshot: &dyn Snapshot,
    format: &str,
    slot: Option<&str>,
    sink: &mut FileSink<'_>,
) -> anyhow::Result<ExportReport> {
    let result: anyhow::Result<ExportReport> = (|| {
        let slots = selected(core, platform, snapshot, format, slot)?;
        let descriptions = descriptions(core, platform, snapshot)?;
        let readers = slots
            .iter()
            .map(|&slot| {
                let description = descriptions
                    .iter()
                    .find(|m| m.slot.id == slot)
                    .ok_or_else(|| usage("export slot is absent from media description"))?;
                snapshot.current(&description.slot.label)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let media: Vec<_> = slots
            .iter()
            .zip(&readers)
            .map(|(&slot, reader)| ExportMedium::new(slot, reader.size(), reader.as_ref()))
            .collect();
        Ok(core.export(
            platform,
            snapshot.config(),
            ExportRequest::new(format, &media),
            sink,
        )?)
    })();
    sink.close();
    let result = result.and_then(|report| {
        sink.validate(&report)?;
        sink.publish(&report)?;
        Ok(report)
    });
    match result {
        Ok(report) => Ok(report),
        Err(error) => match sink.cleanup() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(format!("export cleanup failed: {cleanup}"))),
        },
    }
}

pub fn print(report: &ExportReport, out: &mut dyn Write) -> std::io::Result<()> {
    for receipt in &report.outputs {
        let hash: String = receipt.sha256.iter().map(|b| format!("{b:02x}")).collect();
        writeln!(
            out,
            "{}\tbytes:{}\tsha256:{hash}",
            receipt.name, receipt.size
        )?;
    }
    for note in &report.notes {
        writeln!(out, "note:{note}")?;
    }
    Ok(())
}

fn destination(to: &Path, store: &Store) -> anyhow::Result<PathBuf> {
    let mut ancestor = to;
    let mut missing = Vec::new();
    while !ancestor.try_exists()? {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| usage("invalid export directory"))?,
        );
        ancestor = ancestor
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
    }
    let mut directory = ancestor.canonicalize()?;
    for name in missing.into_iter().rev() {
        directory.push(name);
    }
    if directory.starts_with(store.root().canonicalize()?) {
        return Err(usage("export destination must be outside the read-only store").into());
    }
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

pub(super) fn exit_code(error: &anyhow::Error) -> u8 {
    match error.downcast_ref::<Error>().map(|e| e.kind) {
        Some(ErrorKind::Usage | ErrorKind::Config) => 2,
        _ => 1,
    }
}

pub fn execute(
    command: Command,
    out: &mut dyn Write,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
    cores: &core_launch::Cores,
) -> anyhow::Result<()> {
    let root = command
        .store
        .as_deref()
        .or(host.store_root.as_deref())
        .ok_or_else(|| anyhow::anyhow!("default store unavailable; use --store DIR"))?;
    let store = Store::open_existing(root)?.with_compatibility(host.compatibility.clone());
    if store.read_machines()?.get(&command.id).is_none() {
        return Err(usage(format!(
            "no machine {}; use unemu image machine ls to list machine IDs",
            command.id
        ))
        .into());
    }
    for provider in platforms {
        let Some(snapshot) = provider.export_snapshot(&store, &command.id)? else {
            continue;
        };
        let platform = provider.platform();
        let core = cores
            .for_platform(platform)
            .ok_or_else(|| unsupported("export core unavailable"))?;
        let Some(to) = &command.to else {
            return list(core, platform, snapshot.as_ref(), out);
        };
        let format = command
            .format
            .as_deref()
            .ok_or_else(|| usage("export requires --format FMT"))?;
        selected(
            core,
            platform,
            snapshot.as_ref(),
            format,
            command.slot.as_deref(),
        )?;
        let directory = destination(to, &store)?;
        let mut sink = FileSink::new(&directory)?;
        sink.refuse_store(store.root())?;
        let report = export(
            core,
            platform,
            snapshot.as_ref(),
            format,
            command.slot.as_deref(),
            &mut sink,
        )?;
        print(&report, out)?;
        return Ok(());
    }
    Err(unsupported("machine export unavailable").into())
}

#[cfg(test)]
mod tests;
