// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cheap, cancellable desktop readiness projections; machine-images §§3 and 7.
//! These checks never authorize execution: Start still resolves and hashes the
//! entire closure. ATA/ATAPI-4, sector addressing, supplies chain geometry.
use crate::inspection::{ChainMember, ImageDetails, MediumDetails};
use crate::*;
use image_store::{media_type as mt, BlobSource, Manifest, Store, MAX_SMALL_BLOB};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

/// Inspect only small descriptors and file presence/length. Large payloads are
/// neither hashed nor materialized. Cancellation is checked between each read.
pub fn check(
    store: &Store,
    image: &Descriptor,
    workspace: Option<&str>,
    cancelled: impl Fn() -> bool,
) -> Result<ImageDetails> {
    let stop = || {
        if cancelled() {
            Err(invalid("inspection cancelled"))
        } else {
            Ok(())
        }
    };
    stop()?;
    let manifest =
        Manifest::decode_with_types(&store.read_verified(image, MAX_SMALL_BLOB)?, PAYLOAD_TYPES)?;
    if !crate::is_pc(manifest.platform()?) {
        return Err(invalid("unsupported platform"));
    }
    stop()?;
    let bytes = store.read_verified(&manifest.config, MAX_SMALL_BLOB)?;
    let config =
        crate::provider::stored_config(&bytes, &manifest.config.media_type, &manifest.layers)?;
    if config.schema != 1 || !crate::is_pc(&config.platform) {
        return Err(invalid("config version or platform mismatch"));
    }
    machine_file::parse_toml(
        "image.toml",
        &toml::to_string(&config.machine).map_err(|e| invalid(e.to_string()))?,
    )
    .map_err(machine_file::LoadError::from)?;
    for d in manifest
        .layers
        .iter()
        .chain(std::iter::once(&manifest.config))
    {
        stop()?;
        store.blob_file_path(&d.digest, d.size)?;
    }
    for firmware in &config.firmware {
        stop()?;
        descriptor(&manifest.layers, &firmware.blob.store()?, mt::FIRMWARE)?;
    }
    let mut media = Vec::new();
    for m in &config.media {
        stop()?;
        let head_digest = crate::medium_head(m)?.store()?;
        let head = manifest.layers.iter().find(|d| d.names(&head_digest));
        let members = if m.zero.is_some() {
            vec![(head_digest.clone(), 0)]
        } else {
            let head = head.ok_or_else(|| invalid("missing disk head"))?;
            if matches!(
                head.decoded_media_type(),
                mt::DISK_BASE | mt::DISK_LAYER | DISK_ZERO
            ) {
                crate::disk::display_chain(
                    &head_digest,
                    &m.geometry,
                    &manifest.layers,
                    &Cancellable {
                        store,
                        cancelled: &cancelled,
                    },
                )?
            } else {
                if head.decoded_media_type() != mt::FLOPPY {
                    return Err(invalid("unsupported medium"));
                }
                vec![(head.digest.clone(), head.size)]
            }
        };
        let drive = config
            .slots
            .iter()
            .find(|s| s.catalogue.contains(&m.id))
            .map(|s| format!("{}:{}", s.controller, s.unit))
            .unwrap_or_else(|| "Catalogue".into());
        media.push(MediumDetails {
            id: m.id.clone(),
            drive,
            name: m.name.clone().unwrap_or_else(|| m.id.clone()),
            policy: m.policy,
            chain: members
                .into_iter()
                .map(|(digest, size)| {
                    let zero = manifest
                        .layers
                        .iter()
                        .find(|d| d.digest == digest && d.media_type == DISK_ZERO);
                    Ok(ChainMember {
                        blank_bytes: if let Some(zero) = &m.zero {
                            Some(zero.size)
                        } else {
                            zero.map(|d| {
                                crate::zero::ZeroBase::read(d, store).map(|z| z.byte_len())
                            })
                            .transpose()?
                        },
                        digest,
                        size,
                        shared_by: 0,
                    })
                })
                .collect::<Result<_>>()?,
            working_sectors: working_count(store, workspace, &m.id, m.policy)?,
        });
    }
    if let Some(n) = &config.nvram {
        let d = descriptor(&manifest.layers, &n.head.store()?, CMOS)?;
        let policy = if matches!(n.mode, NvramMode::Journal) {
            Policy::Journal
        } else {
            Policy::Volatile
        };
        media.push(MediumDetails {
            id: "cmos".into(),
            drive: "CMOS".into(),
            name: "CMOS settings".into(),
            policy,
            chain: vec![ChainMember {
                digest: n.head.store()?,
                size: d.size,
                shared_by: 0,
                blank_bytes: None,
            }],
            working_sectors: working_count(store, workspace, "cmos", policy)?,
        });
    }
    let blobs: BTreeMap<_, _> = manifest
        .layers
        .iter()
        .chain(std::iter::once(&manifest.config))
        .chain(std::iter::once(image))
        .map(|d| (d.digest.clone(), d.size))
        .collect();
    let stored_bytes = blobs.values().sum();
    let cards = config
        .machine
        .get("card")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("kind").and_then(toml::Value::as_str))
        .map(device_name)
        .collect::<Vec<_>>();
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
                "Board: {} · RAM: {} KiB conventional + {} KiB extended",
                config
                    .machine
                    .get("board")
                    .and_then(|v| v.get("model"))
                    .and_then(toml::Value::as_str)
                    .unwrap_or("unknown"),
                config
                    .machine
                    .get("memory")
                    .and_then(|v| v.get("conventional_kib"))
                    .and_then(toml::Value::as_integer)
                    .unwrap_or(0),
                config
                    .machine
                    .get("memory")
                    .and_then(|v| v.get("extended_kib"))
                    .and_then(toml::Value::as_integer)
                    .unwrap_or(0)
            ),
            format!("Devices: {}", cards.join(", ")),
        ],
        stored_bytes,
        exclusive_bytes: stored_bytes,
        dedup_bytes: 0,
    })
}
struct Cancellable<'a, F> {
    store: &'a Store,
    cancelled: &'a F,
}
impl<F: Fn() -> bool> BlobSource for Cancellable<'_, F> {
    fn read_verified(&self, d: &Descriptor, limit: u64) -> image_store::Result<Vec<u8>> {
        if (self.cancelled)() {
            return Err(image_store::Error::Invalid("inspection cancelled".into()));
        }
        self.store.read_verified(d, limit)
    }
}
fn device_name(kind: &str) -> String {
    match kind {
        "mda" => "Monochrome display",
        "vga" | "vga-text" => "VGA display",
        "sb16" => "Sound Blaster 16",
        "fdc-765" => "Floppy controller",
        "ide" => "IDE controller",
        "uart-16550" => "Serial port",
        "lotech-ems" => "Expanded memory",
        other => other,
    }
    .into()
}
pub(crate) fn working_count(
    store: &Store,
    workspace: Option<&str>,
    id: &str,
    policy: Policy,
) -> Result<Option<usize>> {
    if policy == Policy::Readonly {
        return Ok(Some(0));
    }
    if policy == Policy::Volatile {
        return Ok(None);
    }
    let Some(name) = workspace else {
        return Ok(Some(0));
    };
    let path = store
        .workspace_path(name)?
        .join("media")
        .join(format!("{id}.upfj"));
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(0)),
        Err(e) => return Err(e.into()),
    };
    // Counts are advisory. Publication still uses the full base-bound scanner.
    let mut header = [0; journal_files::HEADER_LEN];
    file.read_exact(&mut header)?;
    journal_files::Header::decode_unbound(&header).map_err(|e| invalid(e.to_string()))?;
    let mut committed = BTreeSet::new();
    let mut command = BTreeSet::new();
    let mut records = 0;
    let mut expected = 1;
    loop {
        let mut bytes = [0; journal_files::FRAME_LEN];
        let mut first = [0; 1];
        if file.read(&mut first)? == 0 {
            break;
        }
        bytes[0] = first[0];
        if file.read_exact(&mut bytes[1..]).is_err() {
            return Ok(None);
        }
        let frame = match journal_files::Frame::decode(&bytes) {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };
        if frame.sequence != expected {
            return Ok(None);
        }
        expected += 1;
        match frame.record {
            journal_files::Record::Sector { lba, .. } => {
                command.insert(lba);
                records += 1;
            }
            journal_files::Record::CommandEnd { sectors } => {
                if sectors != records {
                    return Ok(None);
                }
                committed.append(&mut command);
                records = 0;
            }
            journal_files::Record::GenerationEnd { .. } => {
                if !command.is_empty() {
                    return Ok(None);
                }
            }
        }
    }
    if records != 0 {
        return Ok(None);
    }
    Ok(Some(committed.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advisory_counts_accept_repeated_lbas_and_report_incomplete_commands_as_unknown() {
        use std::{
            fs,
            sync::atomic::{AtomicU64, Ordering},
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = loop {
            let path = std::env::temp_dir().join(format!(
                "desktop-count-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        };
        let store = Store::open(&path)
            .unwrap()
            .with_compatibility(crate::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ));
        let owner = store.workspace("session").unwrap();
        fs::create_dir(owner.path().join("media")).unwrap();
        let journal = owner.path().join("media/disk.upfj");
        let mut bytes = journal_files::Header::new(&[0; 512], 0)
            .unwrap()
            .encode()
            .unwrap()
            .to_vec();
        fs::write(&journal, &bytes).unwrap();
        assert_eq!(
            working_count(&store, Some("session"), "disk", Policy::Journal).unwrap(),
            Some(0)
        );
        for (sequence, record) in [
            (
                1,
                journal_files::Record::Sector {
                    lba: 0,
                    bytes: Box::new([1; 512]),
                },
            ),
            (
                2,
                journal_files::Record::Sector {
                    lba: 0,
                    bytes: Box::new([2; 512]),
                },
            ),
            (3, journal_files::Record::CommandEnd { sectors: 2 }),
        ] {
            bytes.extend(
                journal_files::Frame {
                    sequence,
                    generation: 1,
                    record,
                }
                .encode()
                .unwrap(),
            );
        }
        fs::write(&journal, &bytes).unwrap();
        assert_eq!(
            working_count(&store, Some("session"), "disk", Policy::Journal).unwrap(),
            Some(1)
        );
        bytes.extend(
            journal_files::Frame {
                sequence: 4,
                generation: 1,
                record: journal_files::Record::Sector {
                    lba: 1,
                    bytes: Box::new([3; 512]),
                },
            }
            .encode()
            .unwrap(),
        );
        fs::write(&journal, &bytes).unwrap();
        assert_eq!(
            working_count(&store, Some("session"), "disk", Policy::Journal).unwrap(),
            None
        );
        drop(owner);
        drop(store);
        fs::remove_dir_all(path).unwrap();
    }
}
