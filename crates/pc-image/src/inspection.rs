// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read-only desktop projections of PC image chains; machine-images §§4 and 7.
//! ATA/ATAPI-4, sector addressing: fixed disks retain their ordered parent chain.

use crate::*;
use image_store::{media_type as mt, BlobSource, Manifest, Store, MAX_SMALL_BLOB};
use std::collections::{BTreeMap, BTreeSet};

/// One immutable member of a drive's base-to-head chain.
#[derive(Clone, Debug)]
pub struct ChainMember {
    pub digest: Digest,
    pub size: u64,
    pub shared_by: usize,
}
/// Catalogue media retain their own chains and persistence intent.
#[derive(Clone, Debug)]
pub struct MediumDetails {
    pub id: String,
    pub drive: String,
    pub name: String,
    pub policy: Policy,
    pub chain: Vec<ChainMember>,
    pub working_sectors: Option<usize>,
}
/// Whole-machine storage, firmware and catalogue information for the desktop.
#[derive(Clone, Debug)]
pub struct ImageDetails {
    pub media: Vec<MediumDetails>,
    pub slots: Vec<String>,
    pub firmware: Vec<String>,
    pub hardware: Vec<String>,
    pub stored_bytes: u64,
    pub exclusive_bytes: u64,
    pub dedup_bytes: u64,
}

/// Verify the selected image completely. Other images contribute only manifest
/// descriptors to sharing counts; their payloads are never opened for this view.
pub fn inspect(
    store: &Store,
    image: &Descriptor,
    workspace_name: Option<&str>,
) -> Result<ImageDetails> {
    // Inspection verifies but never mounts: a base is prepared only when a
    // working journal has to be read against it.
    let prepared = crate::provider::load_pinned(store, image, false, false)?;
    let config = &prepared.config;
    let manifest = &prepared.image.manifest;
    let selected: BTreeSet<_> = manifest
        .layers
        .iter()
        .chain(std::iter::once(&manifest.config))
        .map(|d| &d.digest)
        .collect();
    let mut uses: BTreeMap<Digest, usize> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut after = None;
    loop {
        let page = store.list_tags(after.as_deref(), 1000)?;
        for tag in page.tags {
            if !seen.insert(tag.manifest.digest.clone()) {
                continue;
            }
            if let Ok(bytes) = store.read_verified(&tag.manifest, MAX_SMALL_BLOB) {
                if let Ok(other) = Manifest::decode_for_summary(&bytes) {
                    let ids: BTreeSet<_> = other
                        .layers
                        .iter()
                        .chain(std::iter::once(&other.config))
                        .map(|d| d.digest.clone())
                        .collect();
                    for id in ids {
                        if selected.contains(&id) {
                            *uses.entry(id).or_default() += 1;
                        }
                    }
                }
            }
        }
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    let mut media = Vec::new();
    for m in &config.media {
        let mut chain = Vec::new();
        let mut head = m.head.clone();
        loop {
            let d = manifest
                .layers
                .iter()
                .find(|d| d.names(&head))
                .ok_or_else(|| invalid("missing disk chain member"))?;
            let mut size = d.size;
            let parent = if d.media_type == mt::DISK_LAYER {
                let layer =
                    media::layer::Descriptor::decode(&store.read_verified(d, MAX_SMALL_BLOB)?)?;
                let pack: Digest = layer.sectors.parse()?;
                size += manifest
                    .layers
                    .iter()
                    .find(|d| d.names(&pack))
                    .ok_or_else(|| invalid("missing sector pack"))?
                    .size;
                Some(layer.parent.parse()?)
            } else {
                None
            };
            chain.push(ChainMember {
                digest: head,
                size,
                shared_by: uses.get(&d.digest).copied().unwrap_or(0),
            });
            if let Some(parent) = parent {
                head = parent;
            } else {
                break;
            }
        }
        chain.reverse();
        let working_sectors = match m.policy {
            Policy::Readonly => Some(0),
            Policy::Volatile => None,
            Policy::Journal => working(store, workspace_name, &m.id, || {
                Ok(crate::disk::base_path(store, m, &prepared.image)?.base)
            })?,
        };
        media.push(MediumDetails {
            id: m.id.clone(),
            drive: config
                .slots
                .iter()
                .zip(&prepared.machine.drives)
                .find(|(slot, _)| slot.catalogue.contains(&m.id))
                .map(|(slot, drive)| {
                    let letter = match drive.kind {
                        assembly::DriveKind::Floppy { .. } => char::from(b'A' + slot.unit),
                        assembly::DriveKind::HardDisk { .. } => char::from(b'C' + slot.unit),
                    };
                    format!("{letter}: {}:{}", slot.controller, slot.unit)
                })
                .unwrap_or_else(|| "Catalogue".into()),
            name: m.name.clone().unwrap_or_else(|| m.id.clone()),
            policy: m.policy,
            chain,
            working_sectors,
        });
    }
    if let Some(nvram) = &config.nvram {
        let descriptor = manifest
            .layers
            .iter()
            .find(|d| d.digest == nvram.head)
            .ok_or_else(|| invalid("missing CMOS blob"))?;
        let journaled = matches!(nvram.mode, NvramMode::Journal);
        media.push(MediumDetails {
            id: "cmos".into(),
            drive: "CMOS".into(),
            name: "CMOS settings".into(),
            policy: if journaled {
                Policy::Journal
            } else {
                Policy::Volatile
            },
            chain: vec![ChainMember {
                digest: nvram.head.clone(),
                size: descriptor.size,
                shared_by: uses.get(&nvram.head).copied().unwrap_or(0),
            }],
            working_sectors: if journaled {
                working(store, workspace_name, "cmos", || {
                    Ok(crate::disk::nvram(store, nvram, &prepared.image)?.base)
                })?
            } else {
                None
            },
        });
    }
    let descriptors: BTreeMap<_, _> = manifest
        .layers
        .iter()
        .chain(std::iter::once(&manifest.config))
        .map(|d| (d.digest.clone(), d.size))
        .collect();
    let stored_bytes = descriptors.values().sum::<u64>() + image.size;
    let exclusive_bytes = descriptors
        .iter()
        .filter(|(id, _)| uses.get(id).copied().unwrap_or(0) <= 1)
        .map(|(_, size)| size)
        .sum::<u64>()
        + image.size;
    Ok(ImageDetails {
        media,
        slots: config
            .slots
            .iter()
            .map(|s| {
                format!(
                    "{}:{} catalogue: {} · inserted: {}",
                    s.controller,
                    s.unit,
                    s.catalogue.join(", "),
                    s.inserted.as_deref().unwrap_or("empty")
                )
            })
            .collect(),
        firmware: config
            .firmware
            .iter()
            .map(|f| format!("{} · {}", f.key, &f.blob.hex()[..12]))
            .collect(),
        hardware: vec![
            format!(
                "Board: {} · RAM: {} bytes",
                prepared.machine.board.id(),
                prepared
                    .machine
                    .memory
                    .ram
                    .iter()
                    .map(|r| r.size)
                    .sum::<u64>()
            ),
            format!(
                "Devices: {}",
                prepared
                    .machine
                    .devices
                    .iter()
                    .map(|d| device_label(d.id).to_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ],
        stored_bytes,
        exclusive_bytes,
        dedup_bytes: stored_bytes - exclusive_bytes,
    })
}

fn working(
    store: &Store,
    workspace_name: Option<&str>,
    id: &str,
    base: impl FnOnce() -> Result<media::VerifiedBase>,
) -> Result<Option<usize>> {
    let Some(name) = workspace_name else {
        return Ok(Some(0));
    };
    let path = store
        .workspace_path(name)?
        .join("media")
        .join(format!("{id}.upfj"));
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(0)),
        Err(e) => return Err(e.into()),
    };
    let base = base()?;
    let scan = media::scan_overlay_reader(&mut file, &base, media::SectorSource::byte_len(&base))?;
    Ok(Some(
        scan.overlay
            .ok_or_else(|| invalid("working journal requires repair"))?
            .written_sectors(),
    ))
}

fn device_label(id: assembly::DeviceId) -> &'static str {
    use assembly::DeviceId::*;
    match id {
        Uart => "Serial port",
        Pit => "Programmable timer",
        MasterPic => "Master interrupt controller",
        SlavePic => "Slave interrupt controller",
        Keyboard => "Keyboard controller",
        Rtc => "Real-time clock",
        Vga => "VGA display",
        Mda => "Monochrome display",
        SysControl => "System control",
        SysCtrlPortA => "System control A",
        Post => "POST diagnostics",
        DebugConsole => "Debug console",
        FwCfg => "Firmware configuration",
        Fdc => "Floppy controller",
        Sb16 => "Sound Blaster 16",
        Ide => "IDE controller",
        Dma => "DMA controller",
        DmaWord => "Word DMA controller",
        Coprocessor => "Coprocessor socket",
        LotechEms => "Expanded memory",
    }
}
