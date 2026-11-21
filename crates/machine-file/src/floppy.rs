// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-file floppy append at service boundaries; IBM PC AT Technical Reference,
//! Fixed Disk and Diskette Drive Adapter, Interface Lines; persistent-media.md § Media changes.
use crate::FileMount;
use assembly::{
    AssembledMachine, ConfigError, DeviceId, DiskBase, FloppyError, FloppyMedium, MachineConfig,
};
use std::rc::Rc;
/// Host-file append operations for an assembled PC.
/// IBM PC AT Technical Reference, Fixed Disk and Diskette Drive Adapter,
/// Interface Lines; see persistent-media.md § Media changes.
pub trait FileMachineExt {
    fn append_floppy(
        &mut self,
        controller: DeviceId,
        unit: u8,
        config: &mut MachineConfig,
        medium: &FloppyMedium,
    ) -> Result<usize, FloppyError>;
    fn append_floppy_verified(
        &mut self,
        controller: DeviceId,
        unit: u8,
        config: &mut MachineConfig,
        medium: &FloppyMedium,
        base: journal_files::VerifiedBase,
        readonly_bytes: Option<&[u8]>,
    ) -> Result<usize, FloppyError>;
}
impl FileMachineExt for AssembledMachine {
    /// Append a fully verified immutable base at a completed run boundary.
    /// Selection still uses `change_floppy`, so journal finish/remount and the
    /// drive's change line have exactly the same semantics as catalogue cycling.
    /// The caller retains the live configuration; successful appends update it
    /// so shared validation protects every mounted input and journal path.
    /// The medium is a host file whatever the machine's mount: its journal
    /// opens by path. On a machine assembled over another mount
    /// ([`assembly::assemble_mounted`]) its paths are checked against the other
    /// media appended by path, since the mount's own keys are not host paths.
    /// See persistent-media.md § Media changes and § Memory and hashing.
    fn append_floppy(
        &mut self,
        controller: DeviceId,
        unit: u8,
        config: &mut assembly::MachineConfig,
        medium: &assembly::FloppyMedium,
    ) -> Result<usize, FloppyError> {
        self.append_floppy_receipt(controller, unit, config, medium, None, None)
    }

    /// Reuse a host-prepared full-hash receipt without hashing the disk again.
    /// The captured file checks its metadata on every sector read. Optional read-only
    /// bytes must come from that receipt, so attachment needs no disk read.
    fn append_floppy_verified(
        &mut self,
        controller: DeviceId,
        unit: u8,
        config: &mut assembly::MachineConfig,
        medium: &assembly::FloppyMedium,
        base: journal_files::VerifiedBase,
        readonly_bytes: Option<&[u8]>,
    ) -> Result<usize, FloppyError> {
        self.append_floppy_receipt(controller, unit, config, medium, Some(base), readonly_bytes)
    }
}
impl FileAppend for AssembledMachine {
    fn append_floppy_receipt(
        &mut self,
        controller: DeviceId,
        unit: u8,
        config: &mut assembly::MachineConfig,
        medium: &assembly::FloppyMedium,
        verified: Option<journal_files::VerifiedBase>,
        readonly_bytes: Option<&[u8]>,
    ) -> Result<usize, FloppyError> {
        use assembly::{ImageSource, WriteMode};
        use journal_files::SectorSource;
        self.check_persistence()?;
        let drive_index = config
            .drives
            .iter()
            .position(|drive| drive.controller == controller && drive.unit == unit)
            .ok_or(FloppyError::Unavailable)?;
        let assembly::DriveKind::Floppy { geometry } = config.drives[drive_index].kind else {
            return Err(FloppyError::Unavailable);
        };
        let mut candidate = config.clone();
        let drive = &mut candidate.drives[drive_index];
        if drive.media.is_none() {
            drive.media = Some(assembly::FloppyMediaList {
                empty: false,
                entries: vec![assembly::FloppyMedium {
                    image: drive.image.clone(),
                    expected_sha256: drive.expected_sha256,
                    write: drive.write.clone(),
                    name: self.floppy_name(controller, unit),
                }],
                inserted: 0,
            });
        }
        if let Some(list) = &mut drive.media {
            list.entries.push(medium.clone());
        }
        let host_media = match self.mount.mount().is_file_mount() {
            true => {
                crate::validate(&candidate)?;
                None
            }
            false => {
                candidate.validate_mounted(|media| self.mount.mount().check_outputs(media))?;
                let mut host_media = self.mount.appended_media().to_vec();
                host_media.push((drive_index, medium.clone()));
                check_host_media(&candidate, &host_media)?;
                Some(host_media)
            }
        };
        // The drive's own format passes the type check; the base's size picks
        // the medium's format once it is open.
        self.floppy(controller)?
            .check_append_medium(usize::from(unit), geometry.to_device())?;
        let count = self
            .floppy_names
            .get(&(controller, unit))
            .ok_or(FloppyError::Unavailable)?
            .len();
        let ImageSource::File(path) = &medium.image else {
            return Err(ConfigError::Journal {
                drive_index,
                message: "appended media require a file-backed base".into(),
            }
            .into());
        };
        let base = match verified {
            Some(base) => base,
            None => {
                journal_files::VerifiedBase::open(path).map_err(|error| ConfigError::ImageRead {
                    path: path.clone(),
                    message: error.to_string(),
                })?
            }
        };
        base.read_range(0, &mut [])
            .map_err(|error| ConfigError::ImageRead {
                path: path.clone(),
                message: error.to_string(),
            })?;
        let device_geometry = assembly::medium_geometry(&geometry, base.byte_len(), drive_index)?;
        if medium
            .expected_sha256
            .is_some_and(|expected| expected != base.identity().base_digest)
        {
            return Err(ConfigError::Journal {
                drive_index,
                message: "appended medium SHA-256 changed since selection".into(),
            }
            .into());
        }
        let mut persistence = None;
        let drive = match &medium.write {
            WriteMode::Journal {
                path,
                keep_generations,
            } => {
                if *keep_generations == 0 {
                    return Err(ConfigError::Journal {
                        drive_index,
                        message: "keep_generations must be positive".into(),
                    }
                    .into());
                }
                let (mut handle, media) = assembly::PersistenceHandle::prepare_mounted(
                    drive_index,
                    Rc::new(FileMount::new()),
                    path,
                    DiskBase::from((*base).clone()).with_origin(
                        base.metadata()
                            .cloned()
                            .map(|m| Rc::new(m) as Rc<dyn std::any::Any>),
                    ),
                    *keep_generations,
                );
                handle.floppy = Some((controller, unit, count));
                persistence = Some(handle);
                devices::Drive::with_journaled_media(device_geometry, media)
            }
            WriteMode::ReadOnly => {
                let bytes = if let Some(bytes) = readonly_bytes {
                    if bytes.len() as u64 != base.byte_len() {
                        return Err(ConfigError::ImageGeometryMismatch {
                            drive_index,
                            expected: base.byte_len(),
                            actual: bytes.len() as u64,
                            drive_type: None,
                        }
                        .into());
                    }
                    bytes.to_vec()
                } else {
                    let mut bytes = vec![0; base.byte_len() as usize];
                    base.read_range(0, &mut bytes)
                        .map_err(|error| ConfigError::ImageRead {
                            path: path.clone(),
                            message: error.to_string(),
                        })?;
                    bytes
                };
                devices::Drive::new_read_only(device_geometry, bytes)
            }
            WriteMode::Volatile => {
                return Err(ConfigError::Journal {
                    drive_index,
                    message: "appended media require read-only or journal mode".into(),
                }
                .into())
            }
        };
        let index = self
            .floppy(controller)?
            .append_medium(usize::from(unit), drive)?;
        if let Some(handle) = persistence {
            self.persistence.push(handle);
        }
        if let Some(names) = self.floppy_names.get_mut(&(controller, unit)) {
            names.push(medium.name.clone());
        }
        if let Some(appended) = host_media {
            self.mount.set_appended_media(appended);
        }
        *config = candidate;
        Ok(index)
    }
}
trait FileAppend {
    fn append_floppy_receipt(
        &mut self,
        controller: DeviceId,
        unit: u8,
        config: &mut MachineConfig,
        medium: &FloppyMedium,
        verified: Option<journal_files::VerifiedBase>,
        readonly_bytes: Option<&[u8]>,
    ) -> Result<usize, FloppyError>;
}
/// Check the host paths of the media appended by path to a machine whose
/// own mount is not host files, against each other only: the inputs and
/// journals of those media, with the file rules of
/// [`MachineConfig::validate`]. Errors name the drive of the config.
fn check_host_media(
    config: &MachineConfig,
    media: &[(usize, FloppyMedium)],
) -> Result<(), ConfigError> {
    let mut host = config.clone();
    host.images.clear();
    host.devices.clear();
    host.drives = media
        .iter()
        .map(|(drive_index, medium)| config.drives[*drive_index].with_medium(medium))
        .collect();
    crate::mount_paths::validate_paths(&host).map_err(|error| match error {
        ConfigError::Journal {
            drive_index,
            message,
        } => ConfigError::Journal {
            drive_index: media[drive_index].0,
            message,
        },
        ConfigError::DriveInput {
            drive_index,
            path,
            message,
        } => ConfigError::DriveInput {
            drive_index: media[drive_index].0,
            path,
            message,
        },
        other => other,
    })
}

#[cfg(test)]
#[path = "floppy_tests.rs"]
mod tests;
