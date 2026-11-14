// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host floppy input at deterministic run boundaries. IBM PC AT Technical
//! Reference, Fixed Disk and Diskette Drive Adapter, Digital Input Register
//! (3F7h) and Interface Lines; `backlog/docs/design/spec.md` §7.8 and
//! `backlog/docs/design/persistent-media.md` § Media changes.

use crate::{AssembledMachine, DeviceId, RunPolicy};
use devices::Upd765;
use machine::RunOutcome;

/// One host input, never delivered through the guest keyboard controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloppyAction {
    /// Open the drive door.
    Eject,
    /// Insert a zero-based catalogue entry.
    Insert(usize),
    /// Select the next entry, wrapping to the first; an empty drive selects zero.
    Next,
}

/// A floppy event scheduled in instruction-equivalent virtual time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloppyEvent {
    /// Earliest completed virtual-time boundary; halted PIT clocks may round up.
    pub at: u64,
    /// Stable controller role.
    pub controller: DeviceId,
    /// Controller-local drive-select unit.
    pub unit: u8,
    /// Door operation.
    pub action: FloppyAction,
}

/// Host-side failure applying a media input; never becomes a guest disk error.
#[derive(Debug, thiserror::Error)]
pub enum FloppyError {
    #[error(transparent)]
    Config(#[from] crate::ConfigError),
    #[error("floppy controller or drive is unavailable")]
    Unavailable,
    #[error("floppy event times must be ordered and within the run interval")]
    Schedule,
    #[error(transparent)]
    Device(#[from] devices::upd765::MediaChangeError),
    #[error(transparent)]
    Persistence(#[from] crate::PersistenceError),
    #[error(transparent)]
    Assembly(#[from] crate::AssemblyError),
    #[error(transparent)]
    Run(#[from] crate::RunError),
}

impl AssembledMachine {
    /// Append a fully verified immutable base at a completed run boundary.
    /// Selection still uses `change_floppy`, so journal finish/remount and the
    /// drive's change line have exactly the same semantics as catalogue cycling.
    /// The caller retains the live configuration; successful appends update it
    /// so shared validation protects every mounted input and journal path.
    /// See persistent-media.md § Media changes and § Memory and hashing.
    pub fn append_floppy(
        &mut self,
        controller: DeviceId,
        unit: u8,
        config: &mut crate::MachineConfig,
        medium: &crate::FloppyMedium,
    ) -> Result<usize, FloppyError> {
        use crate::{ConfigError, ImageSource, WriteMode};
        use media::SectorSource;
        self.check_persistence()?;
        let drive_index = config
            .drives
            .iter()
            .position(|drive| drive.controller == controller && drive.unit == unit)
            .ok_or(FloppyError::Unavailable)?;
        let crate::DriveKind::Floppy { geometry } = config.drives[drive_index].kind else {
            return Err(FloppyError::Unavailable);
        };
        let mut candidate = config.clone();
        let drive = &mut candidate.drives[drive_index];
        if drive.media.is_none() {
            drive.media = Some(crate::FloppyMediaList {
                empty: false,
                entries: vec![crate::FloppyMedium {
                    image: drive.image.clone(),
                    expected_sha256: drive.expected_sha256,
                    write: drive.write.clone(),
                    name: self.floppy_name(controller, unit),
                }],
            });
        }
        if let Some(list) = &mut drive.media {
            list.entries.push(medium.clone());
        }
        candidate.validate()?;
        let device_geometry = devices::FloppyGeometry {
            cylinders: geometry.cylinders,
            heads: geometry.heads,
            sectors_per_track: geometry.sectors_per_track,
        };
        self.floppy(controller)?
            .check_append_medium(usize::from(unit), device_geometry)?;
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
        let base = media::VerifiedBase::open(path).map_err(|error| ConfigError::ImageRead {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if geometry.bytes_per_sector != 512 || base.byte_len() != geometry.image_bytes() {
            return Err(ConfigError::ImageGeometryMismatch {
                drive_index,
                expected: geometry.image_bytes(),
                actual: base.byte_len(),
            }
            .into());
        }
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
                let (mut handle, media) = crate::PersistenceHandle::prepare_removable(
                    drive_index,
                    path,
                    base,
                    *keep_generations,
                );
                handle.floppy = Some((controller, unit, count));
                persistence = Some(handle);
                devices::Drive::with_journaled_media(device_geometry, media)
            }
            WriteMode::ReadOnly => {
                let mut bytes = vec![0; base.byte_len() as usize];
                base.read_range(0, &mut bytes)
                    .map_err(|error| ConfigError::ImageRead {
                        path: path.clone(),
                        message: error.to_string(),
                    })?;
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
        *config = candidate;
        Ok(index)
    }

    fn floppy(&mut self, controller: DeviceId) -> Result<&mut Upd765, FloppyError> {
        let index = *self
            .device_index
            .get(&controller)
            .ok_or(FloppyError::Unavailable)?;
        self.machine
            .bus_mut()
            .device_mut(index)
            .and_then(|d| d.as_any_mut())
            .and_then(|d| d.downcast_mut::<Upd765>())
            .ok_or(FloppyError::Unavailable)
    }

    /// Apply an input before advancing the CPU again. Eject seals the current
    /// journal using the session finish API; insertion reopens its immutable base
    /// and overlay. An insertion failure leaves the drive empty and stops the host.
    pub fn change_floppy(
        &mut self,
        controller: DeviceId,
        unit: u8,
        action: FloppyAction,
    ) -> Result<(), FloppyError> {
        self.check_persistence()?;
        let count = self
            .floppy_names
            .get(&(controller, unit))
            .ok_or(FloppyError::Unavailable)?
            .len();
        let fdc = self.floppy(controller)?;
        fdc.check_media_boundary()?;
        let old = fdc.inserted_medium(usize::from(unit));
        let next = match action {
            FloppyAction::Eject => None,
            FloppyAction::Insert(index) => Some(index),
            FloppyAction::Next if count == 0 => return Ok(()),
            FloppyAction::Next => Some(old.map_or(0, |i| (i + 1) % count)),
        };
        if next.is_some_and(|i| i >= count) {
            return Err(devices::upd765::MediaChangeError::UnknownMedium.into());
        }
        if let Some(index) = old {
            for handle in &mut self.persistence {
                if handle.floppy == Some((controller, unit, index)) {
                    handle.eject(&mut self.machine)?;
                }
            }
        }
        self.floppy(controller)?.eject(usize::from(unit))?;
        if let Some(index) = next {
            for handle in &mut self.persistence {
                if handle.floppy == Some((controller, unit, index)) {
                    handle.insert()?;
                }
            }
            self.floppy(controller)?.insert(usize::from(unit), index)?;
        }
        Ok(())
    }

    /// Supply a host label when lowering retained inline bytes without a path.
    /// Labels never enter the guest or affect the event schedule.
    pub fn set_floppy_name(&mut self, controller: DeviceId, unit: u8, medium: usize, name: String) {
        if let Some(label) = self
            .floppy_names
            .get_mut(&(controller, unit))
            .and_then(|names| names.get_mut(medium))
        {
            *label = name;
        }
    }

    /// Name of the inserted medium for host presentation, or `empty`.
    pub fn floppy_name(&mut self, controller: DeviceId, unit: u8) -> String {
        let index = self
            .floppy(controller)
            .ok()
            .and_then(|fdc| fdc.inserted_medium(usize::from(unit)));
        index
            .and_then(|i| {
                self.floppy_names
                    .get(&(controller, unit))
                    .and_then(|names| names.get(i))
            })
            .cloned()
            .unwrap_or_else(|| "empty".into())
    }

    /// Headless input schedule. Events at the same time retain caller order;
    /// completed caps are checked through the usual session run path. The
    /// returned outcome identifies an early halt, so later events are not applied.
    /// Active caps are exact; halted caps use the machine's whole-PIT-clock rounding.
    pub fn run_floppy_schedule(
        &mut self,
        cap: u64,
        idle_limit: Option<u64>,
        events: &[FloppyEvent],
    ) -> Result<RunOutcome, FloppyError> {
        let mut previous = self.machine.virtual_time();
        for event in events {
            if event.at < previous || event.at > cap {
                return Err(FloppyError::Schedule);
            }
            previous = event.at;
        }
        if cap < previous {
            return Err(FloppyError::Schedule);
        }
        for event in events {
            if event.at > self.machine.virtual_time() {
                let outcome = self.run_session(&RunPolicy::natural(Some(event.at), idle_limit))?;
                if outcome != RunOutcome::RetiredCapReached {
                    return Ok(outcome);
                }
            }
            self.change_floppy(event.controller, event.unit, event.action)?;
        }
        Ok(self.run_session(&RunPolicy::natural(Some(cap), idle_limit))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FloppyMediaList, FloppyMedium, ImageSource, WriteMode};

    #[test]
    fn append_floppy_verifies_pins_and_paths_before_changing_the_catalogue() {
        use sha2::{Digest, Sha256};
        let temp = crate::test_support::TempDir::new("append-floppy");
        let path = temp.path().join("extra.img");
        let base = vec![0x33; 1_474_560];
        std::fs::write(&path, &base).unwrap();
        let mut config = crate::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; base.len()]);
        let mut assembled = crate::assemble(&config).unwrap();
        let mut medium = FloppyMedium {
            image: ImageSource::File(path.clone()),
            expected_sha256: Some([0; 32]),
            write: WriteMode::ReadOnly,
            name: "extra.img".into(),
        };
        let original = assembled.floppy_name(DeviceId::Fdc, 0);
        assert!(assembled
            .append_floppy(DeviceId::Fdc, 0, &mut config, &medium)
            .unwrap_err()
            .to_string()
            .contains("SHA-256"));
        assert_eq!(assembled.floppy_names[&(DeviceId::Fdc, 0)].len(), 1);
        medium.expected_sha256 = Some(Sha256::digest(&base).into());
        medium.write = WriteMode::journal(&path);
        assert!(assembled
            .append_floppy(DeviceId::Fdc, 0, &mut config, &medium)
            .unwrap_err()
            .to_string()
            .contains("aliases"));
        medium.write = WriteMode::ReadOnly;
        assert_eq!(
            assembled
                .append_floppy(DeviceId::Fdc, 0, &mut config, &medium)
                .unwrap(),
            1
        );
        assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), original);
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(1))
            .unwrap();
        assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "extra.img");
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
            .unwrap();
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Next)
            .unwrap();
        assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), original);
        assert_eq!(std::fs::read(path).unwrap(), base);
        assembled.finish_persistence().unwrap();
    }

    #[test]
    fn appended_journal_keeps_guest_writes_separate_and_reopens_the_overlay() {
        let temp = crate::test_support::TempDir::new("append-journal");
        let path = temp.path().join("extra.img");
        let journal = temp.path().join("extra.upfj");
        let base = vec![0x33; 1_474_560];
        std::fs::write(&path, &base).unwrap();
        let mut config = crate::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; base.len()]);
        let mut assembled = crate::assemble(&config).unwrap();
        let medium = FloppyMedium {
            image: ImageSource::File(path.clone()),
            expected_sha256: None,
            write: WriteMode::journal(&journal),
            name: "extra.img".into(),
        };
        let index = assembled
            .append_floppy(DeviceId::Fdc, 0, &mut config, &medium)
            .unwrap();
        assert!(!journal.exists());
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(index))
            .unwrap();
        let fdc_index = assembled.device_index[&DeviceId::Fdc];
        let fdc = assembled.machine.bus_mut().device_mut(fdc_index).unwrap();
        fdc.port_write(0x3f2, 1, 0x1c).unwrap();
        for byte in [5, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        assert!(fdc.take_dma_request().is_some());
        fdc.dma_complete(device::DmaOutcome::FromMemory {
            bytes: vec![0x77; 512],
            terminal_count: true,
        });
        for _ in 0..7 {
            fdc.port_read(0x3f5, 1).unwrap();
        }
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), base);
        let scan = media::scan(&std::fs::read(&journal).unwrap(), &base, base.len() as u64);
        assert_eq!(&scan.image.unwrap()[..512], &[0x77; 512]);
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(index))
            .unwrap();
        let fdc = assembled.machine.bus_mut().device_mut(fdc_index).unwrap();
        for byte in [6, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        let device::DmaOp::ToMemory(bytes) = fdc.take_dma_request().unwrap().op else {
            panic!("READ DATA");
        };
        assert_eq!(bytes, vec![0x77; 512]);
        fdc.dma_complete(device::DmaOutcome::ToMemory {
            transferred: 512,
            terminal_count: true,
        });
        assembled.finish_persistence().unwrap();
    }

    #[test]
    fn floppy_media_schedule_is_ordered_and_applies_only_at_the_requested_boundary() {
        let mut config = crate::floppy_boot(vec![0xfa, 0xeb, 0xfe], vec![0; 1_474_560]);
        config.drives[0].media = Some(FloppyMediaList {
            empty: false,
            entries: [0x11, 0x22]
                .into_iter()
                .map(|byte| FloppyMedium {
                    image: ImageSource::Inline(vec![byte; 1_474_560]),
                    expected_sha256: None,
                    write: WriteMode::ReadOnly,
                    name: format!("{byte}.img"),
                })
                .collect(),
        });
        let mut assembled = crate::assemble(&config).unwrap();
        let event = FloppyEvent {
            at: 20,
            controller: DeviceId::Fdc,
            unit: 0,
            action: FloppyAction::Next,
        };
        assembled.run_floppy_schedule(19, None, &[]).unwrap();
        assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "17.img");
        assembled.run_floppy_schedule(20, None, &[event]).unwrap();
        assert_eq!(assembled.machine.virtual_time(), 20);
        assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "34.img");
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x3f7, 1).unwrap(),
            0x80
        );
        assert!(assembled
            .run_floppy_schedule(30, None, &[event, FloppyEvent { at: 19, ..event }])
            .is_err());
        assert_eq!(assembled.machine.virtual_time(), 20);
        assembled
            .run_floppy_schedule(
                20,
                None,
                &[
                    FloppyEvent {
                        action: FloppyAction::Eject,
                        ..event
                    },
                    FloppyEvent {
                        action: FloppyAction::Next,
                        ..event
                    },
                ],
            )
            .unwrap();
        assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "17.img");
        assembled.finish_persistence().unwrap();
    }
    #[test]
    fn floppy_media_runtime_eject_releases_journal_and_reinsertion_replays_writes() {
        let temp = crate::test_support::TempDir::new("floppy-runtime");
        let base_path = temp.path().join("base.img");
        let journal_path = temp.path().join("disk.upfj");
        let base = vec![0x11; 1_474_560];
        std::fs::write(&base_path, &base).unwrap();
        let mut config = crate::floppy_boot(vec![0xfa, 0xeb, 0xfe], base.clone());
        config.drives[0].media = Some(FloppyMediaList {
            empty: false,
            entries: vec![
                FloppyMedium {
                    image: ImageSource::Inline(base.clone()),
                    expected_sha256: None,
                    write: WriteMode::ReadOnly,
                    name: "first.img".into(),
                },
                FloppyMedium {
                    image: ImageSource::File(base_path.clone()),
                    expected_sha256: None,
                    write: WriteMode::journal(&journal_path),
                    name: "second.img".into(),
                },
            ],
        });
        let mut assembled = crate::assemble(&config).unwrap();
        assert!(!journal_path.exists());
        let index = assembled.device_index[&DeviceId::Fdc];
        let boot_snapshot = assembled.machine.bus().device(index).unwrap().save();
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Next)
            .unwrap();
        assert!(journal_path.exists());
        let fdc = assembled.machine.bus_mut().device_mut(index).unwrap();
        let journal_snapshot = fdc.save();
        fdc.restore(&journal_snapshot).unwrap();
        let error = fdc.restore(&boot_snapshot).unwrap_err();
        assert!(error.to_string().contains("journaled medium"));
        assert_eq!(fdc.save(), journal_snapshot);
        assert_eq!(
            std::fs::read(&journal_path).unwrap().len(),
            media::HEADER_LEN
        );
        fdc.port_write(0x3f2, 1, 0x1c).unwrap();
        for byte in [5, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        assert!(fdc.take_dma_request().is_some());
        fdc.dma_complete(device::DmaOutcome::FromMemory {
            bytes: vec![0x77; 512],
            terminal_count: true,
        });
        for _ in 0..7 {
            fdc.port_read(0x3f5, 1).unwrap();
        }
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
            .unwrap();
        assert_eq!(assembled.floppy_name(DeviceId::Fdc, 0), "empty");
        let scan = media::scan(
            &std::fs::read(&journal_path).unwrap(),
            &base,
            base.len() as u64,
        );
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(&scan.image.unwrap()[..512], &[0x77; 512]);
        assembled.run_floppy_schedule(10, None, &[]).unwrap();
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Insert(1))
            .unwrap();
        let fdc = assembled.machine.bus_mut().device_mut(index).unwrap();
        for byte in [6, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        let device::DmaOp::ToMemory(bytes) = fdc.take_dma_request().unwrap().op else {
            panic!("READ DATA");
        };
        assert_eq!(bytes, vec![0x77; 512]);
        fdc.dma_complete(device::DmaOutcome::ToMemory {
            transferred: 512,
            terminal_count: true,
        });
        assembled.finish_persistence().unwrap();
        let scan = media::scan(
            &std::fs::read(&journal_path).unwrap(),
            &base,
            base.len() as u64,
        );
        assert_eq!(scan.report.generations.len(), 2);
        assert_eq!(std::fs::read(base_path).unwrap(), base);
    }

    #[test]
    fn floppy_media_eject_keeps_in_flight_ide_sectors_buffered_until_finish() {
        use crate::{
            AtaControllerModel, DeviceConfig, DeviceKind, DriveConfig, DriveKind, HardDiskGeometry,
            IdeCfg,
        };
        let temp = crate::test_support::TempDir::new("floppy-ide-boundary");
        let floppy_base = vec![0; crate::FloppyGeometry::DD_360K.image_bytes() as usize];
        let ide_base = vec![0x11; 32768];
        let floppy_image = temp.path().join("floppy.img");
        let ide_image = temp.path().join("ide.img");
        let floppy_journal = temp.path().join("floppy.upfj");
        let ide_journal = temp.path().join("ide.upfj");
        std::fs::write(&floppy_image, &floppy_base).unwrap();
        std::fs::write(&ide_image, &ide_base).unwrap();
        let mut config = crate::at_fdc_write(vec![0xfa, 0xf4], vec![0; 512], 0x600);
        config.drives[0].image = ImageSource::File(floppy_image);
        config.drives[0].write = WriteMode::journal(&floppy_journal);
        let ide = IdeCfg {
            model: AtaControllerModel::Ata,
            base: 0x170,
            control: 0x376,
        };
        config.devices.push(
            DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(ide), ide.port_windows())
                .with_irq(ide.irq()),
        );
        config.drives.push(DriveConfig {
            media: None,
            controller: DeviceId::Ide,
            unit: 0,
            kind: DriveKind::HardDisk {
                geometry: HardDiskGeometry {
                    cylinders: 8,
                    heads: 2,
                    sectors_per_track: 4,
                    bytes_per_sector: 512,
                },
            },
            image: ImageSource::File(ide_image.clone()),
            expected_sha256: None,
            write: WriteMode::journal(&ide_journal),
        });
        let mut assembled = crate::assemble(&config).unwrap();
        let bus = assembled.machine.bus_mut();
        for (port, value) in [
            (0x176, 0xe0),
            (0x172, 2),
            (0x173, 0),
            (0x174, 0),
            (0x175, 0),
            (0x177, 0x30),
        ] {
            bus.port_write(port, 1, value).unwrap();
        }
        for _ in 0..256 {
            bus.port_write(0x170, 2, 0x1234).unwrap();
        }
        for _ in 0..17 {
            bus.port_write(0x170, 2, 0x5678).unwrap();
        }
        let ide_index = assembled.device_index[&DeviceId::Ide];
        let state = assembled.machine.bus().device(ide_index).unwrap().save();
        let before = std::fs::read(&ide_journal).unwrap();
        assert_eq!(before.len(), media::HEADER_LEN);
        assembled
            .change_floppy(DeviceId::Fdc, 0, FloppyAction::Eject)
            .unwrap();
        assert_eq!(std::fs::read(&ide_journal).unwrap(), before);
        assert_eq!(
            assembled.machine.bus().device(ide_index).unwrap().save(),
            state
        );
        let floppy = media::scan(
            &std::fs::read(&floppy_journal).unwrap(),
            &floppy_base,
            floppy_base.len() as u64,
        );
        assert_eq!(floppy.report.generations.len(), 1);
        assert_eq!(
            floppy.report.generations[0].reason,
            media::ClosingReason::SessionEnd
        );
        assert_eq!(assembled.machine.virtual_time(), 0);
        assembled.finish_persistence().unwrap();
        let ide = media::scan(
            &std::fs::read(&ide_journal).unwrap(),
            &ide_base,
            ide_base.len() as u64,
        );
        assert_eq!(ide.report.generations.len(), 1);
        let mut expected = ide_base.clone();
        expected[..512].copy_from_slice(&0x1234u16.to_le_bytes().repeat(256));
        assert_eq!(ide.image.unwrap(), expected);
        assert_eq!(std::fs::read(ide_image).unwrap(), ide_base);
    }
}
