// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host journal ownership and the NEC µPD765 sector-media adapter (WRITE DATA,
//! N=2). See `backlog/docs/design/persistent-media.md`, §Integration and
//! §Host errors and determinism. Only frontend boundaries observe host errors.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use device::{InMemoryMedia, SectorMedia, SectorWrite};
use media::{JournalMedia, SinkFailure, SinkStatus, VerificationReport};

use crate::{ConfigError, ImageSource, MachineConfig, WriteMode};

enum Session {
    Active(Box<JournalMedia>),
    Finished(InMemoryMedia),
}

/// One drive's persistence failure, including its configuration index.
#[derive(Debug, thiserror::Error)]
#[error("drive {drive_index} persistence: {source}")]
pub struct PersistenceError {
    /// Index in the machine configuration's drive list.
    pub drive_index: usize,
    #[source]
    pub source: SinkFailure,
}

/// Frontend ownership of a journal session. Call `finish` only after successful
/// execution; dropping this handle never writes a clean generation marker.
#[must_use = "journal sessions must be checked and explicitly finished"]
pub struct PersistenceHandle {
    /// Index in the machine configuration's drive list.
    pub drive_index: usize,
    /// Validated closed-generation retention setting (including the anchor).
    pub keep_generations: u32,
    session: Rc<RefCell<Session>>,
    status: SinkStatus,
    report: VerificationReport,
}

impl PersistenceHandle {
    pub(crate) fn mount(
        drive_index: usize,
        path: &Path,
        base: &[u8],
        geometry_length: u64,
        keep_generations: u32,
        base_metadata: Option<&std::fs::Metadata>,
    ) -> Result<(Self, Box<dyn SectorMedia>), crate::AssemblyError> {
        let status = SinkStatus::default();
        let journal = match base_metadata {
            Some(metadata) => JournalMedia::open_with_base_metadata(
                path,
                base,
                geometry_length,
                status.clone(),
                metadata,
            ),
            None => JournalMedia::open(path, base, geometry_length, status.clone()),
        }
        .map_err(|source| crate::AssemblyError::Journal {
            drive_index,
            source,
        })?
        .with_retention(keep_generations);
        let report = journal.mount_report().clone();
        let session = Rc::new(RefCell::new(Session::Active(Box::new(journal))));
        let adapter = Box::new(JournalAdapter(session.clone()));
        Ok((
            Self {
                drive_index,
                keep_generations,
                session,
                status,
                report,
            },
            adapter,
        ))
    }

    /// Inject host write failures while retaining real in-memory media updates.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_output(&mut self) {
        let mut session = self.session.borrow_mut();
        let Session::Active(journal) = std::mem::replace(
            &mut *session,
            Session::Finished(InMemoryMedia::new(Vec::new(), 0, false)),
        ) else {
            panic!("active session required")
        };
        *session = Session::Active(Box::new((*journal).with_failing_output()));
    }

    /// Inject retention publication failures without altering guest commands.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_retention(&mut self, operation: &'static str) {
        let mut session = self.session.borrow_mut();
        let Session::Active(journal) = std::mem::replace(
            &mut *session,
            Session::Finished(InMemoryMedia::new(Vec::new(), 0, false)),
        ) else {
            panic!("active session required")
        };
        *session = Session::Active(Box::new((*journal).with_failing_retention(operation)));
    }

    /// First latched host error; the guest never observes this channel.
    pub fn status(&self) -> Option<SinkFailure> {
        self.status.failure()
    }

    /// Maintenance diagnostics are separate from the guest-command error latch.
    pub fn maintenance_failure(&self) -> Option<media::MaintenanceFailure> {
        self.status.maintenance_failure()
    }

    /// Recovery evidence from before mount repaired or sealed the tip.
    pub fn mount_report(&self) -> &VerificationReport {
        &self.report
    }

    /// Report the first latched host error without changing guest-visible state.
    pub fn check(&self) -> Result<(), PersistenceError> {
        self.status().map_or(Ok(()), |source| {
            Err(PersistenceError {
                drive_index: self.drive_index,
                source,
            })
        })
    }

    pub(crate) fn check_running(&self) -> Result<(), PersistenceError> {
        self.check()?;
        if matches!(*self.session.borrow(), Session::Finished(_)) {
            return Err(PersistenceError {
                drive_index: self.drive_index,
                source: SinkFailure {
                    operation: "run",
                    message: "session already finished".into(),
                    last_confirmed_sequence: 0,
                },
            });
        }
        Ok(())
    }

    /// Close a generation at a completed machine service boundary and continue.
    pub fn checkpoint(&mut self) -> Result<(), PersistenceError> {
        self.check()?;
        match &mut *self.session.borrow_mut() {
            Session::Active(journal) => journal.checkpoint().map_err(|source| PersistenceError {
                drive_index: self.drive_index,
                source,
            }),
            Session::Finished(_) => Err(PersistenceError {
                drive_index: self.drive_index,
                source: SinkFailure {
                    operation: "checkpoint",
                    message: "session already finished".into(),
                    last_confirmed_sequence: 0,
                },
            }),
        }
    }

    /// Explicitly finish once. Retain readable bytes for final frontend capture;
    /// the consuming sink API releases the lock only after attempting its marker.
    pub fn finish(&mut self) -> Result<(), PersistenceError> {
        self.check()?;
        let mut session = self.session.borrow_mut();
        let Session::Active(journal) = &*session else {
            return Ok(());
        };
        let image = journal.image().to_vec();
        let sectors = image.len() as u64 / 512;
        let old = std::mem::replace(
            &mut *session,
            Session::Finished(InMemoryMedia::new(image, sectors, false)),
        );
        if let Session::Active(journal) = old {
            (*journal).finish().map_err(|source| PersistenceError {
                drive_index: self.drive_index,
                source,
            })?;
        }
        Ok(())
    }
}

struct JournalAdapter(Rc<RefCell<Session>>);

impl SectorMedia for JournalAdapter {
    fn sector_count(&self) -> u64 {
        match &*self.0.borrow() {
            Session::Active(journal) => journal.image().len() as u64 / 512,
            Session::Finished(image) => image.sector_count(),
        }
    }

    fn read_sector(&self, lba: u64) -> [u8; 512] {
        match &*self.0.borrow() {
            Session::Active(journal) => journal.read_sector(lba),
            Session::Finished(image) => image.read_sector(lba),
        }
    }

    fn write_batch(&mut self, sectors: &[SectorWrite]) {
        match &mut *self.0.borrow_mut() {
            Session::Active(journal) => {
                journal.write_batch(&sectors.iter().map(|s| (s.lba, s.bytes)).collect::<Vec<_>>())
            }
            Session::Finished(image) => image.write_batch(sectors),
        }
    }

    fn write_protected(&self) -> bool {
        false
    }
}

/// Validate every output before opening any journal or creating a lock file.
/// Canonical names cover symlinks and parent aliases; existing inode identity
/// also catches hard links, including the stable lock sidecars.
pub(crate) fn validate_paths(config: &MachineConfig) -> Result<(), ConfigError> {
    let inputs: Vec<_> = config
        .drives
        .iter()
        .map(|d| &d.image)
        .chain(config.images.iter().map(|i| &i.source))
        .filter_map(|source| {
            if let ImageSource::File(path) = source {
                Some(path.as_path())
            } else {
                None
            }
        })
        .collect();
    let reserved = |path: &Path| {
        JournalMedia::is_retention_temp_name(path)
            || normalized(path)
                .ok()
                .is_some_and(|p| JournalMedia::is_retention_temp_name(&p))
    };
    for (drive_index, drive) in config.drives.iter().enumerate() {
        if let ImageSource::File(path) = &drive.image {
            if reserved(path) {
                return Err(ConfigError::DriveInput {
                    drive_index,
                    path: path.clone(),
                    message: "uses a reserved retention-temporary name".into(),
                });
            }
        }
    }
    for (image_index, image) in config.images.iter().enumerate() {
        if let ImageSource::File(path) = &image.source {
            if reserved(path) {
                return Err(ConfigError::ImageInput {
                    image_index,
                    role: if image.kind == crate::ImageKind::Rom {
                        "firmware"
                    } else {
                        "RAM-load"
                    },
                    path: path.clone(),
                    message: "uses a reserved retention-temporary name".into(),
                });
            }
        }
    }
    let normalize = |drive_index, path: &Path, role| {
        normalized(path).map_err(|error| ConfigError::Journal {
            drive_index,
            message: format!("{role} path {}: {error}", path.display()),
        })
    };
    let mut journals = Vec::new();
    for (drive_index, drive) in config.drives.iter().enumerate() {
        let WriteMode::Journal { path, .. } = &drive.write else {
            continue;
        };
        if JournalMedia::is_creation_temp_name(path) {
            return Err(ConfigError::Journal {
                drive_index,
                message: format!(
                    "journal path {} uses a reserved creation-temporary name",
                    path.display()
                ),
            });
        }
        let journal = normalize(drive_index, path, "journal")?;
        if JournalMedia::is_creation_temp_name(&journal) {
            return Err(ConfigError::Journal {
                drive_index,
                message: format!(
                    "journal path {} resolves to reserved creation-temporary path {}",
                    path.display(),
                    journal.display()
                ),
            });
        }
        let mut lock = journal.as_os_str().to_os_string();
        lock.push(".lock");
        let lock = normalize(drive_index, Path::new(&lock), "journal lock")?;
        journals.push((drive_index, path, journal, lock));
    }
    let mut outputs: Vec<PathBuf> = Vec::new();
    for (drive_index, path, journal, lock) in &journals {
        let error = |message| ConfigError::Journal {
            drive_index: *drive_index,
            message,
        };
        for input in &inputs {
            // Only successfully resolved inputs can alias an output. Their own
            // image read must report every path error without journal attribution.
            let unresolved = canonical_parent_name(input).ok();
            let resolved = normalized(input).ok();
            if [unresolved.as_ref(), resolved.as_ref()]
                .into_iter()
                .flatten()
                .any(|candidate| {
                    JournalMedia::is_creation_temp(journal, candidate)
                        || candidate.file_name().is_some_and(|n| {
                            n.as_encoded_bytes().windows(8).any(|s| s == b".retain-")
                        })
                })
            {
                return Err(error(format!(
                    "journal {} reserves creation-temporary input path {}",
                    path.display(),
                    input.display()
                )));
            }
            if let Some(input_path) = resolved {
                for output in [journal, lock] {
                    if aliases(output, &input_path) {
                        return Err(error(format!(
                            "output {} aliases mounted input {}",
                            output.display(),
                            input.display()
                        )));
                    }
                }
            }
        }
        for output in [journal, lock] {
            for other in &outputs {
                if aliases(output, other) {
                    return Err(error(format!(
                        "output {} aliases another output {}",
                        output.display(),
                        other.display()
                    )));
                }
            }
            outputs.push(output.clone());
        }
    }
    Ok(())
}

fn canonical_parent_name(path: &Path) -> std::io::Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "input path must have a filename",
        )
    })?;
    Ok(std::fs::canonicalize(parent)?.join(name))
}

fn normalized(path: &Path) -> std::io::Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Refuse dangling links instead of treating them as unused names.
            if std::fs::symlink_metadata(path).is_ok() {
                return Err(error);
            }
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Ok(std::fs::canonicalize(parent)?.join(path.file_name().ok_or(error)?))
        }
        Err(error) => Err(error),
    }
}

fn aliases(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    #[cfg(unix)]
    if let (Ok(a), Ok(b)) = (std::fs::metadata(left), std::fs::metadata(right)) {
        use std::os::unix::fs::MetadataExt;
        return a.dev() == b.dev() && a.ino() == b.ino();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpu::Cpu;

    #[test]
    fn failing_sink_preserves_guest_prefix_and_stops_at_frontend_boundary() {
        let temp = crate::test_support::TempDir::new("journal-test");
        let path = temp.path();
        let out = |program: &mut Vec<u8>, port: u16, value: u8| {
            program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
        };
        let mut program = vec![0xFA, 0xFC];
        out(&mut program, 0x3F2, 0x1C);
        for (operation, write) in [true, true, false].into_iter().enumerate() {
            for (port, value) in [
                (0x0A, 6),
                (0x0C, 0),
                (0x0B, if write { 0x4A } else { 0x46 }),
                (4, 0),
                (
                    4,
                    if !write {
                        10
                    } else if operation == 0 {
                        6
                    } else {
                        8
                    },
                ),
                (0x81, 0),
                (5, 0xFF),
                (5, 1),
                (0x0A, 2),
            ] {
                out(&mut program, port, value);
            }
            for value in [if write { 5 } else { 6 }, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
                out(&mut program, 0x3F5, value);
            }
            program.extend([0xBF, 0, 0x10 + operation as u8]);
            for _ in 0..7 {
                program.extend([0xEC, 0xAA]);
            }
        }
        program.push(0xF4);
        let pattern = vec![0xA7; 512];
        let mut config = crate::at_fdc_write(program, pattern.clone(), 0x600);
        let base = path.join("base.img");
        let journal_path = path.join("disk.upfj");
        std::fs::write(&base, vec![0; config.drives[0].kind.image_bytes() as usize]).unwrap();
        config.drives[0].image = ImageSource::File(base);
        let second_pattern = vec![0x5C; 512];
        let mut second_image = config.images[1].clone();
        second_image.base = 0x800;
        second_image.source = ImageSource::Inline(second_pattern.clone());
        config.images.push(second_image);
        let mut volatile = crate::assemble(&config).unwrap();
        config.drives[0].write = WriteMode::journal(&journal_path);
        let mut failing = crate::assemble(&config).unwrap();
        failing.persistence[0].fail_output();
        let policy = crate::RunPolicy::natural(Some(10_000), None);
        assert_eq!(
            volatile.run_session(&policy).unwrap(),
            machine::RunOutcome::Halted
        );
        assert!(matches!(
            failing.run_session(&policy),
            Err(crate::RunError::Persistence(_))
        ));
        assert_eq!(failing.persistence[0].status().unwrap().operation, "write");
        let mut actual = vec![0; 0x10000];
        let mut expected = actual.clone();
        failing.machine.cpu().read_mem(0, &mut actual).unwrap();
        volatile.machine.cpu().read_mem(0, &mut expected).unwrap();
        assert_eq!(
            actual[0x1101] & 2,
            0,
            "the second WRITE DATA must not become write-protected"
        );
        assert_eq!(&actual[0x1100..0x1107], &expected[0x1100..0x1107]);
        assert!(
            actual[0xA00..0xC00].iter().all(|byte| *byte == 0x5C),
            "the second WRITE DATA must replace the first pattern after the host error"
        );
        assert_ne!(&actual[0xA00..0xC00], pattern);
        assert_eq!(actual, expected);
        assert_eq!(
            failing.machine.virtual_time(),
            volatile.machine.virtual_time()
        );
        assert_eq!(
            failing.machine.cpu().retired(),
            volatile.machine.cpu().retired()
        );
        assert_eq!(failing.machine.clock(), volatile.machine.clock());
        let before = failing.machine.virtual_time();
        assert!(failing
            .run_session(&crate::RunPolicy::natural(Some(20_000), None))
            .is_err());
        assert_eq!(failing.machine.virtual_time(), before);
        assert!(failing.finish_persistence().is_err());
        drop(failing);
        drop(volatile);
        assert_eq!(
            std::fs::metadata(journal_path).unwrap().len(),
            media::HEADER_LEN as u64
        );
    }
    #[test]
    fn retention_failures_preserve_controller_status_dma_irq_and_virtual_time() {
        for operation in [
            "temporary write",
            "temporary sync",
            "replace",
            "publication directory sync",
        ] {
            let temp = crate::test_support::TempDir::new("retention-test");
            let mut program = vec![0xFA, 0xFC];
            let out = |p: &mut Vec<u8>, port: u16, value: u8| {
                p.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
            };
            out(&mut program, 0x3F2, 0x1C);
            for (port, value) in [
                (0x0A, 6),
                (0x0C, 0),
                (0x0B, 0x4A),
                (4, 0),
                (4, 6),
                (0x81, 0),
                (5, 0xFF),
                (5, 1),
                (0x0A, 2),
            ] {
                out(&mut program, port, value);
            }
            for value in [5, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
                out(&mut program, 0x3F5, value);
            }
            program.extend([0xBF, 0, 0x10]);
            for _ in 0..7 {
                program.extend([0xEC, 0xAA]);
            }
            program.push(0xF4);
            let mut config = crate::at_fdc_write(program, vec![0xA7; 512], 0x600);
            let base = temp.path().join("base.img");
            let bytes = vec![0; config.drives[0].kind.image_bytes() as usize];
            std::fs::write(&base, &bytes).unwrap();
            config.drives[0].image = ImageSource::File(base.clone());
            let path = temp.path().join("disk.upfj");
            config.drives[0].write = WriteMode::Journal {
                path: path.clone(),
                keep_generations: 1,
            };
            let mut machine = crate::assemble(&config).unwrap();
            machine.persistence[0].checkpoint().unwrap();
            machine.persistence[0].fail_retention(operation);
            machine
                .run_session(&crate::RunPolicy::natural(Some(10_000), None))
                .unwrap();
            let mut before = vec![0; 0x10000];
            machine.machine.cpu().read_mem(0, &mut before).unwrap();
            let time = machine.machine.virtual_time();
            let retired = machine.machine.cpu().retired();
            let devices: Vec<_> = machine
                .device_index
                .values()
                .map(|&index| {
                    (
                        index,
                        machine.machine.bus().device(index).unwrap().save(),
                        machine.machine.bus().device_irq_line(index),
                    )
                })
                .collect();
            machine.persistence[0].checkpoint().unwrap();
            let failure = machine.persistence[0].maintenance_failure().unwrap();
            assert_eq!(machine.persistence[0].status().is_some(), failure.uncertain);
            assert_eq!(machine.check_persistence().is_err(), failure.uncertain);
            assert_eq!(
                failure.uncertain,
                matches!(operation, "replace" | "publication directory sync")
            );
            let mut after = before.clone();
            machine.machine.cpu().read_mem(0, &mut after).unwrap();
            assert_eq!(after, before);
            assert_eq!(after[0x1001] & 2, 0);
            assert_eq!(machine.machine.virtual_time(), time);
            assert_eq!(machine.machine.cpu().retired(), retired);
            for (index, snapshot, irq) in devices {
                assert_eq!(
                    machine.machine.bus().device(index).unwrap().save(),
                    snapshot
                );
                assert_eq!(machine.machine.bus().device_irq_line(index), irq);
            }
            assert_eq!(std::fs::read(&base).unwrap(), bytes);
            let scan = media::scan(&std::fs::read(&path).unwrap(), &bytes, bytes.len() as u64);
            assert_eq!(scan.report.status, media::Status::Valid);
            assert!(!scan.report.needs_recovery());
            assert_eq!(&scan.image.unwrap()[..512], &[0xA7; 512]);
        }
    }
}
