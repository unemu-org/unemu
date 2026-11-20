// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host journal ownership for NEC µPD765 media (WRITE DATA, N=2), ATA-2 §7
//! WRITE SECTORS / WRITE MULTIPLE (512-byte sectors), and
//! Motorola MC146818 general RAM (0Eh–3Fh, excluding the transient AT shutdown
//! byte 0Fh). See `backlog/docs/design/persistent-media.md`, §Integration and
//! §Host errors and determinism. Only frontend boundaries observe host errors.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use device::{SectorMedia, SectorWrite};
use media::{
    JournalMedia, SectorSource, SinkFailure, SinkStatus, SparseOverlay, VerificationReport,
    VerifiedBase,
};

#[cfg(test)]
use crate::mount::FileMount;
use crate::mount::{DiskBase, Mount, MountError, Store};
use crate::{ConfigError, ImageSource, MachineConfig, WriteMode};

/// What a removable medium needs to open its journal again at each insert:
/// the machine's mount, the journal's key in it and the medium's base.
struct Remount {
    mount: Rc<dyn Mount>,
    path: PathBuf,
    base: DiskBase,
}

enum Session {
    Active(Box<JournalMedia>),
    Finished(SparseOverlay, SinkStatus),
    Empty,
}

/// One medium's persistence failure, identifying a drive or CMOS RAM.
#[derive(Debug, thiserror::Error)]
#[error("{} persistence: {source}", if *.nvram { "NVRAM".into() } else { format!("drive {}", .drive_index) })]
pub struct PersistenceError {
    /// Drive configuration index, or RTC device configuration index for NVRAM.
    pub drive_index: usize,
    /// This failure belongs to CMOS rather than a drive.
    pub nvram: bool,
    #[source]
    pub source: SinkFailure,
}

/// Frontend ownership of a journal session. Call `finish` only after successful
/// execution; dropping this handle never writes a clean generation marker.
#[must_use = "journal sessions must be checked and explicitly finished"]
pub struct PersistenceHandle {
    /// Drive configuration index, or RTC device configuration index for NVRAM.
    pub drive_index: usize,
    /// Validated closed-generation retention setting (including the anchor).
    pub keep_generations: u32,
    pub(crate) ata_index: Option<usize>,
    session: Rc<RefCell<Session>>,
    status: SinkStatus,
    maintenance: Option<media::MaintenanceFailure>,
    report: VerificationReport,
    nvram: Option<devices::cmos_rtc::NvramStore>,
    remount: Option<Remount>,
    ejected: bool,
    pub(crate) floppy: Option<(crate::DeviceId, u8, usize)>,
}

impl PersistenceHandle {
    /// Open the journal at host `path` over `base`, a test's shortcut.
    #[cfg(test)]
    pub(crate) fn mount(
        drive_index: usize,
        path: &Path,
        base: &[u8],
        geometry_length: u64,
        keep_generations: u32,
        base_metadata: Option<&std::fs::Metadata>,
    ) -> Result<(Self, Box<dyn SectorMedia>), crate::AssemblyError> {
        let base =
            VerifiedBase::from_bytes(base).map_err(|source| crate::AssemblyError::Journal {
                drive_index,
                source: source.into(),
            })?;
        Self::mount_journal(
            drive_index,
            &FileMount::new(),
            path,
            &DiskBase::from(base).with_origin(base_metadata.cloned()),
            geometry_length,
            keep_generations,
        )
    }

    /// Open the journal that `mount` keeps under `path` over `base`, and
    /// replay it. A file journal opens through the journal's path
    /// constructors, as it always has; a record opens through
    /// [`JournalMedia::open_record`].
    pub(crate) fn mount_journal(
        drive_index: usize,
        mount: &dyn Mount,
        path: &Path,
        base: &DiskBase,
        geometry_length: u64,
        keep_generations: u32,
    ) -> Result<(Self, Box<dyn SectorMedia>), crate::AssemblyError> {
        let status = SinkStatus::default();
        let store = match mount.journal(path) {
            Ok(store) => store.into_store(),
            Err(MountError::Config(error)) => return Err(error.into()),
            Err(MountError::Host(error)) => {
                return Err(crate::AssemblyError::Journal {
                    drive_index,
                    source: media::OpenError::Host(SinkFailure {
                        operation: "open journal record",
                        message: error.to_string(),
                        last_confirmed_sequence: 0,
                    }),
                })
            }
        };
        let journal = match store {
            Store::File(path) => JournalMedia::open_verified_with_metadata(
                &path,
                base.verified(),
                geometry_length,
                status.clone(),
                base.origin(),
            ),
            Store::Record(record) => {
                JournalMedia::open_record(record, base.verified(), geometry_length, status.clone())
            }
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
                ata_index: None,
                session,
                status,
                maintenance: None,
                report,
                nvram: None,
                remount: None,
                ejected: false,
                floppy: None,
            },
            adapter,
        ))
    }

    /// A removable medium whose journal at host `path` opens at insert.
    #[cfg(test)]
    pub(crate) fn prepare_removable(
        drive_index: usize,
        path: &Path,
        base: VerifiedBase,
        keep_generations: u32,
    ) -> (Self, Box<dyn SectorMedia>) {
        Self::prepare_mounted(
            drive_index,
            Rc::new(FileMount::new()),
            path,
            base.into(),
            keep_generations,
        )
    }

    /// A removable medium, ejected: its journal, which `mount` keeps under
    /// `path`, opens at each insert and closes at each eject. The handle
    /// keeps the mount for that.
    pub(crate) fn prepare_mounted(
        drive_index: usize,
        mount: Rc<dyn Mount>,
        path: &Path,
        base: DiskBase,
        keep_generations: u32,
    ) -> (Self, Box<dyn SectorMedia>) {
        let session = Rc::new(RefCell::new(Session::Empty));
        let status = SinkStatus::default();
        let adapter = Box::new(JournalAdapter(session.clone()));
        let handle = Self {
            drive_index,
            keep_generations,
            ata_index: None,
            session,
            status,
            maintenance: None,
            report: media::scan(&[], &[], 0).report,
            nvram: None,
            remount: Some(Remount {
                mount,
                path: path.into(),
                base,
            }),
            ejected: true,
            floppy: None,
        };
        (handle, adapter)
    }

    pub(crate) fn insert(&mut self) -> Result<(), crate::AssemblyError> {
        let Some(remount) = &self.remount else {
            return Ok(());
        };
        self.check().map_err(|e| {
            crate::AssemblyError::Config(ConfigError::Journal {
                drive_index: self.drive_index,
                message: e.to_string(),
            })
        })?;
        let (mounted, _) = Self::mount_journal(
            self.drive_index,
            remount.mount.as_ref(),
            &remount.path,
            &remount.base,
            remount.base.len(),
            self.keep_generations,
        )?;
        let state = std::mem::replace(&mut *mounted.session.borrow_mut(), Session::Empty);
        *self.session.borrow_mut() = state;
        self.maintenance = self.maintenance_failure();
        self.status = mounted.status;
        self.report = mounted.report;
        self.ejected = false;
        Ok(())
    }

    pub(crate) fn eject(
        &mut self,
        machine: &mut machine::Machine<cpu::X86JitCpu>,
    ) -> Result<(), PersistenceError> {
        self.finish(machine)?;
        self.ejected = true;
        Ok(())
    }

    pub(crate) fn mount_nvram(
        device_index: usize,
        mount: &dyn Mount,
        path: &Path,
        base: &[u8; 64],
        keep_generations: u32,
        origin: Option<&std::fs::Metadata>,
        rtc: &mut devices::CmosRtc,
    ) -> Result<Self, crate::AssemblyError> {
        let (mut handle, _) = VerifiedBase::from_bytes(base)
            .map_err(|source| crate::AssemblyError::Journal {
                drive_index: device_index,
                source: source.into(),
            })
            .and_then(|base| {
                Self::mount_journal(
                    device_index,
                    mount,
                    path,
                    &DiskBase::from(base).with_origin(origin.cloned()),
                    64,
                    keep_generations,
                )
            })
            .map_err(|error| match error {
                crate::AssemblyError::Journal { source, .. } => {
                    crate::AssemblyError::NvramJournal { source }
                }
                other => other,
            })?;
        let mut bytes = [0; 64];
        if let Session::Active(journal) = &*handle.session.borrow() {
            bytes[14..].copy_from_slice(&journal.read_sector(0)[14..64]);
        }
        // Clock, alarm and control registers belong to the RTC model on each run.
        *rtc = std::mem::take(rtc).with_general_nvram(std::array::from_fn(|i| bytes[14 + i]));
        handle.nvram = Some(rtc.nvram_store());
        Ok(handle)
    }

    /// Human-readable owner used by frontend diagnostics.
    pub fn owner(&self) -> String {
        if self.nvram.is_some() {
            "NVRAM".into()
        } else {
            format!("drive {}", self.drive_index)
        }
    }

    /// The IBM PC AT Technical Reference, CMOS shutdown status byte, assigns
    /// 0Fh to the live reset protocol; it must not become persistent setup data.
    fn persist_nvram(&self) -> Result<(), PersistenceError> {
        let Some(store) = &self.nvram else {
            return Ok(());
        };
        let bytes = store.snapshot();
        if let Session::Active(journal) = &mut *self.session.borrow_mut() {
            if journal.read_sector(0)[14] != bytes[0]
                || journal.read_sector(0)[16..64] != bytes[2..]
            {
                let mut block = journal.read_sector(0);
                block[14] = bytes[0];
                block[16..64].copy_from_slice(&bytes[2..]);
                journal.write_batch(&[(0, block)]);
            }
        }
        self.check()
    }

    /// Inject host write failures while retaining real in-memory media updates.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_output(&mut self) {
        let mut session = self.session.borrow_mut();
        let Session::Active(journal) = std::mem::replace(&mut *session, Session::Empty) else {
            panic!("active session required")
        };
        *session = Session::Active(Box::new((*journal).with_failing_output()));
    }

    /// Inject retention publication failures without altering guest commands.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_retention(&mut self, operation: &'static str) {
        let mut session = self.session.borrow_mut();
        let Session::Active(journal) = std::mem::replace(&mut *session, Session::Empty) else {
            panic!("active session required")
        };
        *session = Session::Active(Box::new((*journal).with_failing_retention(operation)));
    }

    /// First latched host error; the guest never observes this channel.
    pub fn status(&self) -> Option<SinkFailure> {
        match &*self.session.borrow() {
            Session::Active(journal) => journal.check_base(),
            Session::Finished(overlay, status) => {
                if let Err(error) = overlay.check_unchanged() {
                    status.latch_host_error("base changed", error.to_string());
                }
            }
            Session::Empty => {}
        }
        self.status.failure()
    }

    /// First maintenance diagnostic, retained across removable remounts and
    /// separate from the guest-command error latch.
    pub fn maintenance_failure(&self) -> Option<media::MaintenanceFailure> {
        self.maintenance
            .clone()
            .or_else(|| self.status.maintenance_failure())
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
                nvram: self.nvram.is_some(),
                source,
            })
        })
    }

    pub(crate) fn check_running(&self) -> Result<(), PersistenceError> {
        self.check()?;
        if !self.ejected && matches!(*self.session.borrow(), Session::Finished(..)) {
            return Err(PersistenceError {
                drive_index: self.drive_index,
                nvram: self.nvram.is_some(),
                source: SinkFailure {
                    operation: "run",
                    message: "session already finished".into(),
                    last_confirmed_sequence: 0,
                },
            });
        }
        Ok(())
    }

    /// Append changed CMOS RAM before the guest CPU reset, without closing its
    /// session generation. Host-paced run boundaries only check error latches.
    pub fn before_cpu_reset(&self) -> Result<(), PersistenceError> {
        self.check_running()?;
        self.persist_nvram()
    }

    /// Flush this ATA controller's completed sectors before closing a generation.
    /// Supply the owning assembly's machine; ordinary run stops must not flush.
    /// For CMOS, only validate the session: its generation closes at finish.
    pub fn checkpoint(
        &mut self,
        machine: &mut machine::Machine<cpu::X86JitCpu>,
    ) -> Result<(), PersistenceError> {
        flush_ata(machine, std::slice::from_ref(self));
        self.check()?;
        if self.nvram.is_some() && matches!(*self.session.borrow(), Session::Active(_)) {
            return Ok(());
        }
        match &mut *self.session.borrow_mut() {
            Session::Active(journal) => journal.checkpoint().map_err(|source| PersistenceError {
                drive_index: self.drive_index,
                nvram: self.nvram.is_some(),
                source,
            }),
            Session::Finished(..) | Session::Empty => Err(PersistenceError {
                drive_index: self.drive_index,
                nvram: self.nvram.is_some(),
                source: SinkFailure {
                    operation: "checkpoint",
                    message: "session already finished".into(),
                    last_confirmed_sequence: 0,
                },
            }),
        }
    }

    /// Flush this ATA controller's completed sectors and explicitly finish once.
    /// Supply the owning assembly's machine, after successful execution only.
    /// Readable bytes remain available for capture; the sink releases its lock
    /// only after attempting the closing marker. Partial sectors are excluded.
    pub fn finish(
        &mut self,
        machine: &mut machine::Machine<cpu::X86JitCpu>,
    ) -> Result<(), PersistenceError> {
        flush_ata(machine, std::slice::from_ref(self));
        self.check()?;
        self.ejected = false;
        self.persist_nvram()?;
        let mut session = self.session.borrow_mut();
        if !matches!(*session, Session::Active(_)) {
            return Ok(());
        }
        let old = std::mem::replace(&mut *session, Session::Empty);
        if let Session::Active(journal) = old {
            let (overlay, result) = (*journal).finish_readable();
            *session = Session::Finished(overlay, self.status.clone());
            result.map_err(|source| PersistenceError {
                drive_index: self.drive_index,
                nvram: self.nvram.is_some(),
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
            Session::Active(journal) => journal.byte_len() / 512,
            Session::Finished(image, _) => image.byte_len() / 512,
            Session::Empty => 0,
        }
    }

    fn read_sector(&self, lba: u64) -> [u8; 512] {
        match &*self.0.borrow() {
            Session::Active(journal) => journal.read_sector(lba),
            Session::Finished(image, status) => image.read_sector(lba).unwrap_or_else(|error| {
                status.latch_host_error("base read", error.to_string());
                [0; 512]
            }),
            Session::Empty => [0; 512],
        }
    }

    fn write_batch(&mut self, sectors: &[SectorWrite]) {
        match &mut *self.0.borrow_mut() {
            Session::Active(journal) => {
                journal.write_batch(&sectors.iter().map(|s| (s.lba, s.bytes)).collect::<Vec<_>>())
            }
            Session::Finished(..) | Session::Empty => {}
        }
    }

    fn write_protected(&self) -> bool {
        false
    }
}

/// ATA-2 §8.4 commits sectors after their final word. An explicit persistence
/// boundary submits that prefix; ordinary host pacing must not split commands.
pub(crate) fn flush_ata(
    machine: &mut machine::Machine<cpu::X86JitCpu>,
    handles: &[PersistenceHandle],
) {
    for index in handles.iter().filter_map(|handle| handle.ata_index) {
        let ata = machine
            .bus_mut()
            .device_mut(index)
            .and_then(|device| device.as_any_mut())
            .and_then(|device| device.downcast_mut::<devices::Ata>())
            .expect("journal ATA controller must exist and expose Ata");
        ata.flush_pending();
    }
}

/// Validate every output before opening any journal or creating a lock file.
/// Canonical names cover symlinks and parent aliases; existing inode identity
/// also catches hard links, including the stable lock sidecars.
pub(crate) fn validate_paths(config: &MachineConfig) -> Result<(), ConfigError> {
    validate_media_paths(config).map_err(|error| match error {
        ConfigError::Journal {
            drive_index,
            message,
        } if drive_index >= config.drives.len() => ConfigError::Nvram { message },
        other => other,
    })
}

fn path_error(config: &MachineConfig, index: usize, message: String) -> ConfigError {
    if index >= config.drives.len() {
        ConfigError::Nvram { message }
    } else {
        ConfigError::Journal {
            drive_index: index,
            message,
        }
    }
}

/// NVRAM journals keep at least one closed generation, whatever holds them.
pub(crate) fn check_nvram_retention(config: &MachineConfig) -> Result<(), ConfigError> {
    let zero = config.devices.iter().any(|device| {
        matches!(
            &device.kind,
            crate::DeviceKind::Rtc(crate::CmosCfg {
                nvram: crate::NvramSource::Journal {
                    keep_generations: 0,
                    ..
                },
                ..
            })
        )
    });
    if zero {
        return Err(ConfigError::Nvram {
            message: "keep_generations must be positive".into(),
        });
    }
    Ok(())
}

/// No two journal outputs (fixed drives, catalogue entries, NVRAM journals)
/// name the same key, whatever the mount: two media would share one journal.
/// `config` has one drive per catalogue entry.
pub(crate) fn check_distinct_journals(config: &MachineConfig) -> Result<(), ConfigError> {
    let drives = config.drives.iter().map(|drive| match &drive.write {
        WriteMode::Journal { path, .. } => Some(path),
        _ => None,
    });
    let nvram = config.devices.iter().map(|device| match &device.kind {
        crate::DeviceKind::Rtc(crate::CmosCfg {
            nvram: crate::NvramSource::Journal { journal, .. },
            ..
        }) => Some(journal),
        _ => None,
    });
    let mut seen = std::collections::BTreeSet::new();
    for (index, path) in drives.chain(nvram).enumerate() {
        if let Some(path) = path {
            if !seen.insert(path) {
                return Err(path_error(
                    config,
                    index,
                    format!(
                        "journal {} is also another medium's journal",
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_media_paths(config: &MachineConfig) -> Result<(), ConfigError> {
    let nvram: Vec<_> = config
        .devices
        .iter()
        .enumerate()
        .filter_map(|(index, d)| match &d.kind {
            crate::DeviceKind::Rtc(crate::CmosCfg {
                nvram:
                    crate::NvramSource::Journal {
                        image,
                        journal,
                        keep_generations,
                        ..
                    },
                ..
            }) => Some((
                config.drives.len() + index,
                image.as_path(),
                journal.as_path(),
                *keep_generations,
            )),
            _ => None,
        })
        .collect();
    for (_, image, _, keep) in &nvram {
        if *keep == 0 {
            return Err(ConfigError::Nvram {
                message: "keep_generations must be positive".into(),
            });
        }
        if JournalMedia::is_retention_temp_name(image)
            || normalized(image)
                .ok()
                .is_some_and(|p| JournalMedia::is_retention_temp_name(&p))
        {
            return Err(ConfigError::Nvram {
                message: "image uses a reserved retention-temporary name".into(),
            });
        }
    }
    let mut inputs: Vec<_> = config
        .drives
        .iter()
        .map(|d| &d.image)
        .chain(config.images.iter().map(|i| &i.source))
        .flat_map(|source| match source {
            ImageSource::File(path) => vec![path.as_path()],
            ImageSource::Interleaved(parts) => parts.iter().map(|p| p.path.as_path()).collect(),
            _ => Vec::new(),
        })
        .collect();
    inputs.extend(nvram.iter().map(|(_, image, _, _)| *image));
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
        let paths = match &image.source {
            ImageSource::File(path) => vec![path],
            ImageSource::Interleaved(parts) => parts.iter().map(|p| &p.path).collect(),
            _ => Vec::new(),
        };
        for path in paths {
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
        normalized(path).map_err(|error| {
            path_error(
                config,
                drive_index,
                format!("{role} path {}: {error}", path.display()),
            )
        })
    };
    let mut journals = Vec::new();
    for (drive_index, path) in config
        .drives
        .iter()
        .enumerate()
        .filter_map(|(i, drive)| {
            if let WriteMode::Journal { path, .. } = &drive.write {
                Some((i, path.as_path()))
            } else {
                None
            }
        })
        .chain(nvram.iter().map(|(i, _, path, _)| (*i, *path)))
    {
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
    fn floppy_media_journal_mock_bus_seals_releases_lock_and_reopens_overlay() {
        use device::{Device, DmaOp, DmaOutcome};
        use devices::{upd765::Drive, FloppyGeometry, Upd765};
        let mut machine = crate::assemble(&crate::at_boot(vec![0xf4]))
            .unwrap()
            .machine;
        let temp = crate::test_support::TempDir::new("floppy-eject-journal");
        let path = temp.path().join("disk.upfj");
        let base = vec![0x11; 1_474_560];
        let (mut handle, media) = PersistenceHandle::prepare_removable(
            0,
            &path,
            VerifiedBase::from_bytes(&base).unwrap(),
            10,
        );
        assert!(!path.exists(), "inactive journal must not be opened");
        handle.check_running().unwrap();
        handle.insert().unwrap();
        let mut fdc = Upd765::with_drives([
            Some(Drive::with_media(FloppyGeometry::HD1440, media)),
            None,
            None,
            None,
        ]);
        fdc.port_write(0x3f2, 1, 0x1c).unwrap();
        let mut bus = testkit::MockBus::new(Box::new(fdc));
        for value in [5, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            bus.port_write(0x3f5, 1, value).unwrap();
        }
        assert!(matches!(
            bus.device_mut().take_dma_request().unwrap().op,
            DmaOp::FromMemory { len: 512 }
        ));
        bus.device_mut().dma_complete(DmaOutcome::FromMemory {
            bytes: vec![0x77; 512],
            terminal_count: true,
        });
        for _ in 0..7 {
            bus.port_read(0x3f5, 1).unwrap();
        }
        handle.eject(&mut machine).unwrap();
        bus.device_mut()
            .as_any_mut()
            .unwrap()
            .downcast_mut::<Upd765>()
            .unwrap()
            .eject(0)
            .unwrap();
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(
            scan.report.generations[0].reason,
            media::ClosingReason::SessionEnd
        );
        assert_eq!(&scan.image.unwrap()[..512], &[0x77; 512]);
        let other =
            JournalMedia::open(&path, &base, base.len() as u64, SinkStatus::default()).unwrap();
        drop(other);
        handle.check_running().unwrap();
        handle.insert().unwrap();
        bus.device_mut()
            .as_any_mut()
            .unwrap()
            .downcast_mut::<Upd765>()
            .unwrap()
            .insert(0, 0)
            .unwrap();
        for value in [6, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            bus.port_write(0x3f5, 1, value).unwrap();
        }
        let DmaOp::ToMemory(bytes) = bus.device_mut().take_dma_request().unwrap().op else {
            panic!("READ DATA");
        };
        assert_eq!(bytes, vec![0x77; 512]);
        bus.device_mut().dma_complete(DmaOutcome::ToMemory {
            transferred: 512,
            terminal_count: true,
        });
        handle.finish(&mut machine).unwrap();
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
        assert_eq!(scan.report.generations.len(), 2);
        assert_eq!(base, vec![0x11; 1_474_560]);
    }

    #[test]
    fn floppy_media_reinsertion_preserves_retention_warning_from_eject() {
        let mut machine = crate::assemble(&crate::at_boot(vec![0xf4]))
            .unwrap()
            .machine;
        let temp = crate::test_support::TempDir::new("removable-retention-warning");
        let path = temp.path().join("disk.upfj");
        let base = vec![0; 512];
        let (mut handle, mut media) = PersistenceHandle::prepare_removable(
            0,
            &path,
            VerifiedBase::from_bytes(&base).unwrap(),
            1,
        );
        handle.insert().unwrap();
        media.write_batch(&[SectorWrite {
            lba: 0,
            bytes: [0x11; 512],
        }]);
        handle.eject(&mut machine).unwrap();
        handle.insert().unwrap();
        media.write_batch(&[SectorWrite {
            lba: 0,
            bytes: [0x22; 512],
        }]);
        handle.fail_retention("temporary write");
        handle.eject(&mut machine).unwrap();
        let warning = handle
            .maintenance_failure()
            .expect("retention warning latched on eject");
        handle.insert().unwrap();
        assert_eq!(
            handle.maintenance_failure(),
            Some(warning.clone()),
            "reinsertion must retain the warning until the frontend can print it"
        );
        assert_eq!(media.read_sector(0), [0x22; 512]);
        handle.eject(&mut machine).unwrap();
        assert_eq!(handle.maintenance_failure(), Some(warning));
    }

    #[test]
    fn floppy_media_journal_failed_eject_never_writes_a_clean_close() {
        let mut machine = crate::assemble(&crate::at_boot(vec![0xf4]))
            .unwrap()
            .machine;
        let temp = crate::test_support::TempDir::new("floppy-eject-error");
        let path = temp.path().join("disk.upfj");
        let base = vec![0; 1_474_560];
        let (mut handle, mut media) = PersistenceHandle::prepare_removable(
            0,
            &path,
            VerifiedBase::from_bytes(&base).unwrap(),
            10,
        );
        handle.insert().unwrap();
        handle.fail_output();
        media.write_batch(&[SectorWrite {
            lba: 0,
            bytes: [0x77; 512],
        }]);
        assert!(handle.eject(&mut machine).is_err());
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
        assert!(scan.report.generations.is_empty());
        assert_eq!(media.read_sector(0), [0x77; 512]);
    }

    fn nvram_config(dir: &Path, program: Vec<u8>) -> (MachineConfig, [u8; 64], PathBuf) {
        use sha2::{Digest, Sha256};
        let base = std::array::from_fn(|i| i as u8);
        let image = dir.join("cmos.bin");
        let journal = dir.join("cmos.upfj");
        std::fs::write(&image, base).unwrap();
        let mut config = crate::at_rtc(program, 100, 10);
        let rtc = config
            .devices
            .iter_mut()
            .find(|d| matches!(d.kind, crate::DeviceKind::Rtc(_)))
            .unwrap();
        let crate::DeviceKind::Rtc(c) = &mut rtc.kind else {
            unreachable!()
        };
        c.nvram = crate::NvramSource::Journal {
            image,
            expected_sha256: Sha256::digest(base).into(),
            journal: journal.clone(),
            keep_generations: 2,
        };
        (config, base, journal)
    }

    #[cfg(unix)]
    #[test]
    fn logical_validation_ignores_host_paths_but_checks_journal_keys_and_retention() {
        let temp = crate::test_support::TempDir::new("logical-validation");
        let (mut config, _, journal) = nvram_config(temp.path(), vec![0xf4]);
        std::os::unix::fs::symlink("absent", &journal).unwrap();
        assert!(config.validate().is_err());
        config.validate_logical().unwrap();

        let rtc = config
            .devices
            .iter_mut()
            .find_map(|d| match &mut d.kind {
                crate::DeviceKind::Rtc(c) => Some(c),
                _ => None,
            })
            .unwrap();
        let crate::NvramSource::Journal {
            keep_generations, ..
        } = &mut rtc.nvram
        else {
            unreachable!()
        };
        *keep_generations = 0;
        assert!(config
            .validate_logical()
            .unwrap_err()
            .to_string()
            .contains("keep_generations must be positive"));
        let (nvram, _, _) = nvram_config(temp.path(), vec![0xf4]);
        let rtc = nvram
            .devices
            .iter()
            .find(|d| matches!(d.kind, crate::DeviceKind::Rtc(_)))
            .unwrap()
            .clone();
        let mut config = crate::at_fdc_write(vec![0xf4], vec![0; 512], 0x1000);
        config
            .devices
            .retain(|d| !matches!(d.kind, crate::DeviceKind::Rtc(_)));
        config.devices.push(rtc);
        config.drives[0].image = ImageSource::File(temp.path().join("disk.img"));
        config.drives[0].write = WriteMode::journal(&journal);
        let error = config.validate_logical().unwrap_err().to_string();
        assert!(error.contains("also another medium's journal"), "{error}");
    }

    fn write_cmos(machine: &mut crate::AssembledMachine, index: u8, value: u8) {
        machine
            .machine
            .bus_mut()
            .port_write(0x70, 1, u32::from(index))
            .unwrap();
        machine
            .machine
            .bus_mut()
            .port_write(0x71, 1, u32::from(value))
            .unwrap();
    }

    fn read_cmos(machine: &mut crate::AssembledMachine, index: u8) -> u8 {
        machine
            .machine
            .bus_mut()
            .port_write(0x70, 1, u32::from(index))
            .unwrap();
        machine.machine.bus_mut().port_read(0x71, 1).unwrap() as u8
    }

    #[test]
    fn nvram_run_stops_and_prompt_polls_do_not_append() {
        let temp = crate::test_support::TempDir::new("nvram-polls");
        let (config, base, path) = nvram_config(temp.path(), vec![0x90, 0xEB, 0xFD]);
        let mut machine = crate::assemble_with_observation(
            &config,
            &crate::ObservationPolicy {
                routes: vec![crate::ObservationRequest {
                    device: crate::DeviceId::Vga,
                    output: crate::OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..Default::default()
            },
        )
        .unwrap();
        write_cmos(&mut machine, 0x10, 0xA5);
        machine
            .run_session(&crate::RunPolicy::natural(Some(5), None))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            media::HEADER_LEN as u64
        );
        let mut policy = crate::RunPolicy::natural(Some(20), None);
        policy.stop = crate::StopCondition::PromptThen {
            console: crate::ConsoleProbe {
                device: crate::DeviceId::Vga,
            },
            needle: "never present".into(),
            poll_step: 2,
            then_budget: 0,
        };
        machine.run_session(&policy).unwrap();
        machine.check_persistence().unwrap();
        machine.persistence[0]
            .checkpoint(&mut machine.machine)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            media::HEADER_LEN as u64
        );
        machine.finish_persistence().unwrap();
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(
            scan.report.generations[0].reason,
            media::ClosingReason::SessionEnd
        );
        assert_eq!(scan.report.generations[0].commands, 1);
    }

    #[test]
    fn nvram_finish_coalesces_replays_and_closes_one_session_generation() {
        let temp = crate::test_support::TempDir::new("nvram-session");
        let (config, base, path) = nvram_config(
            temp.path(),
            vec![0xFA, 0xB0, 0x0E, 0xE6, 0x70, 0xB0, 0xA5, 0xE6, 0x71, 0xF4],
        );
        let mut first = crate::assemble(&config).unwrap();
        assert_eq!(read_cmos(&mut first, 0x0B), 2);
        first
            .run_session(&crate::RunPolicy::natural(Some(100), None))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            media::HEADER_LEN as u64
        );
        first.finish_persistence().unwrap();
        let length = std::fs::metadata(&path).unwrap().len();
        first.finish_persistence().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
        assert_eq!(
            first.persistence[0]
                .checkpoint(&mut first.machine)
                .unwrap_err()
                .source
                .message,
            "session already finished"
        );
        drop(first);
        let mut second = crate::assemble(&config).unwrap();
        assert_eq!(read_cmos(&mut second, 14), 0xA5);
        assert_eq!(read_cmos(&mut second, 63), base[63]);
        assert_eq!(read_cmos(&mut second, 0x0B), 2);
        for index in 0..14 {
            write_cmos(&mut second, index, 0x12);
        }
        write_cmos(&mut second, 63, 0x55);
        write_cmos(&mut second, 63, base[63]);
        write_cmos(&mut second, 15, 9);
        write_cmos(&mut second, 16, 0x67);
        write_cmos(&mut second, 50, 0x21);
        second.finish_persistence().unwrap();
        drop(second);
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 2);
        assert!(scan
            .report
            .generations
            .iter()
            .all(|g| g.reason == media::ClosingReason::SessionEnd && g.commands == 1));
        let image = scan.image.unwrap();
        assert_eq!(&image[..14], &base[..14]);
        assert_eq!(image[15], base[15]);
        assert_eq!(image[16], 0x67);
        assert_eq!(image[50], 0x21);
        let mut third = crate::assemble(&config).unwrap();
        assert_eq!(read_cmos(&mut third, 15), base[15]);
        assert_eq!(read_cmos(&mut third, 50), 0x21);
        assert_eq!(read_cmos(&mut third, 0), 0);
        third.finish_persistence().unwrap();
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
        let last = scan.report.generations.last().unwrap();
        assert_eq!(last.reason, media::ClosingReason::SessionEnd);
        assert_eq!(
            last.commands, 0,
            "unchanged session has only its end marker"
        );
    }

    fn reset_config(config: &mut MachineConfig, reset_program: Vec<u8>) {
        config.memory.address_space_bytes = 0x100000;
        config.images.push(crate::LoadImage {
            base: 0xFFFF0,
            source: ImageSource::Inline(reset_program),
            kind: crate::ImageKind::Rom,
            provides: vec![],
        });
    }

    #[test]
    fn nvram_shutdown_only_reset_never_appends_ram() {
        for index in [0x0F, 0x10] {
            let temp = crate::test_support::TempDir::new("nvram-shutdown");
            let (mut config, base, path) = nvram_config(
                temp.path(),
                vec![
                    0xFA, 0xB0, index, 0xE6, 0x70, 0xB0, 9, 0xE6, 0x71, 0xB0, 0xFE, 0xE6, 0x64,
                ],
            );
            reset_config(&mut config, vec![0xF4]);
            let mut machine = crate::assemble(&config).unwrap();
            machine
                .run_session(&crate::RunPolicy::natural(Some(100), None))
                .unwrap();
            assert_eq!(
                read_cmos(&mut machine, index),
                9,
                "CPU reset preserves live CMOS"
            );
            let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
            assert!(scan.report.generations.is_empty());
            if index == 15 {
                assert!(scan.report.unsealed_tip.is_none());
                assert_eq!(
                    std::fs::metadata(&path).unwrap().len(),
                    media::HEADER_LEN as u64
                );
            } else {
                assert_eq!(scan.report.unsealed_tip.as_ref().unwrap().commands, 1);
                assert_eq!(scan.image.as_ref().unwrap()[16], 9);
                assert_eq!(scan.image.as_ref().unwrap()[15], base[15]);
            }
            machine.finish_persistence().unwrap();
            let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
            assert_eq!(scan.report.generations.len(), 1);
            assert_eq!(
                scan.report.generations[0].reason,
                media::ClosingReason::SessionEnd
            );
            assert_eq!(scan.report.generations[0].commands, u64::from(index == 16));
        }
    }

    #[test]
    fn nvram_execution_error_leaves_reset_records_open_for_recovery() {
        let temp = crate::test_support::TempDir::new("nvram-error");
        let (mut config, base, path) = nvram_config(
            temp.path(),
            vec![
                0xFA, 0xB0, 0x10, 0xE6, 0x70, 0xB0, 9, 0xE6, 0x71, 0xB0, 0xFE, 0xE6, 0x64,
            ],
        );
        // A processor exception now vectors in the guest. Fetching beyond the
        // rig's physical map still tests host-error recovery after CPU reset.
        reset_config(&mut config, vec![0xEA, 0xFF, 0xFF, 0xFF, 0xFF]);
        let mut machine = crate::assemble(&config).unwrap();
        let error = machine
            .run_session(&crate::RunPolicy::natural(Some(100), None))
            .unwrap_err();
        assert!(matches!(
            error,
            crate::RunError::Machine(machine::MachineError::Cpu(cpu::CpuError::Unmapped {
                addr: 0x10FFEF
            }))
        ));
        drop(machine);
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert!(scan.report.generations.is_empty());
        assert_eq!(scan.report.unsealed_tip.as_ref().unwrap().commands, 1);
        let mut next = crate::assemble(&config).unwrap();
        assert!(next.persistence[0].mount_report().unsealed_tip.is_some());
        assert_eq!(read_cmos(&mut next, 16), 9);
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(
            scan.report.generations[0].reason,
            media::ClosingReason::Recovered
        );
        next.finish_persistence().unwrap();
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 2);
        assert_eq!(
            scan.report.generations[1].reason,
            media::ClosingReason::SessionEnd
        );
    }

    #[test]
    fn nvram_resets_share_a_generation_and_retention_waits_for_finish() {
        let temp = crate::test_support::TempDir::new("nvram-retention");
        let (mut config, base, path) = nvram_config(temp.path(), vec![0xF4]);
        for device in &mut config.devices {
            if let crate::DeviceKind::Rtc(crate::CmosCfg {
                nvram:
                    crate::NvramSource::Journal {
                        keep_generations, ..
                    },
                ..
            }) = &mut device.kind
            {
                *keep_generations = 1;
            }
        }
        let mut first = crate::assemble(&config).unwrap();
        write_cmos(&mut first, 16, 0x55);
        first.finish_persistence().unwrap();
        drop(first);
        let mut second = crate::assemble(&config).unwrap();
        for value in [0x66, 0x77] {
            write_cmos(&mut second, 16, value);
            second.before_cpu_reset().unwrap();
            let before = std::fs::read(&path).unwrap();
            second.before_cpu_reset().unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "unchanged reset adds no record"
            );
            let scan = media::scan(&before, &base, 64);
            assert_eq!(scan.report.header.unwrap().anchor_generation, 0);
            assert_eq!(scan.report.generations.len(), 1);
            assert_eq!(scan.report.unsealed_tip.unwrap().generation, 2);
        }
        second.finish_persistence().unwrap();
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
        assert_eq!(scan.report.generations.len(), 1);
        assert_eq!(scan.report.header.unwrap().anchor_generation, 2);
        assert_eq!(scan.image.unwrap()[16], 0x77);
        let error = second.persistence[0]
            .checkpoint(&mut second.machine)
            .unwrap_err();
        assert_eq!(error.source.operation, "checkpoint");
        assert_eq!(error.source.message, "session already finished");
    }

    #[test]
    fn nvram_tail_damage_is_reported_and_base_pin_precedes_outputs() {
        use std::io::Write;
        let temp = crate::test_support::TempDir::new("nvram-tail");
        let (config, base, path) = nvram_config(temp.path(), vec![0xF4]);
        let mut machine = crate::assemble(&config).unwrap();
        write_cmos(&mut machine, 63, 0x99);
        machine.finish_persistence().unwrap();
        drop(machine);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0x55; 27])
            .unwrap();
        let mut machine = crate::assemble(&config).unwrap();
        assert_eq!(machine.persistence[0].mount_report().discarded_bytes, 27);
        assert_eq!(read_cmos(&mut machine, 63), 0x99);
        machine.finish_persistence().unwrap();
        drop(machine);
        let mut changed = base;
        changed[0] ^= 1;
        std::fs::write(temp.path().join("cmos.bin"), changed).unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = crate::assemble(&config).err().unwrap().to_string();
        assert!(error.contains("NVRAM: image sha256 mismatch"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(&path).unwrap();
        let lock = path.with_file_name("cmos.upfj.lock");
        std::fs::remove_file(&lock).unwrap();
        let error = crate::assemble(&config).err().unwrap().to_string();
        assert!(error.contains("NVRAM: image sha256 mismatch"), "{error}");
        assert!(!path.exists());
        assert!(!lock.exists());
    }

    #[test]
    fn nvram_output_failure_stops_before_cpu_reset() {
        let temp = crate::test_support::TempDir::new("nvram-reset");
        let program = vec![
            0xFA, 0xB0, 0x3F, 0xE6, 0x70, 0xB0, 0xA5, 0xE6, 0x71, 0xB0, 0xFE, 0xE6, 0x64, 0xF4,
        ];
        let (mut config, base, path) = nvram_config(temp.path(), program);
        config.memory.address_space_bytes = 0x100000;
        config.images.push(crate::LoadImage {
            base: 0xFFFF0,
            source: ImageSource::Inline(vec![0xF4]),
            kind: crate::ImageKind::Rom,
            provides: vec![],
        });
        for fail in [true, false] {
            let mut machine = crate::assemble(&config).unwrap();
            if fail {
                machine.persistence[0].fail_output();
            }
            let result = machine.run_session(&crate::RunPolicy::natural(Some(100), None));
            if fail {
                assert!(matches!(result, Err(crate::RunError::Persistence(_))));
                assert_eq!(machine.machine.cpu().code_segment(), 0);
                assert!(machine
                    .check_persistence()
                    .unwrap_err()
                    .to_string()
                    .starts_with("NVRAM persistence:"));
            } else {
                assert_eq!(result.unwrap(), machine::RunOutcome::Halted);
                assert_eq!(machine.machine.cpu().code_segment(), 0xF000);
                let scan = media::scan(&std::fs::read(&path).unwrap(), &base, 64);
                assert_eq!(
                    scan.report.generations.len(),
                    0,
                    "reset boundary must keep the session open"
                );
                assert_eq!(scan.report.unsealed_tip.as_ref().unwrap().commands, 1);
                assert_eq!(scan.image.as_ref().unwrap()[63], 0xA5);
                machine.finish_persistence().unwrap();
            }
        }
    }

    #[test]
    #[should_panic(expected = "journal ATA controller must exist and expose Ata")]
    fn ide_journal_invalid_controller_index_is_an_assembly_invariant() {
        let temp = crate::test_support::TempDir::new("ide-controller-invariant");
        let (config, _, _) = nvram_config(temp.path(), vec![0xF4]);
        let mut machine = crate::assemble(&config).unwrap();
        machine.persistence[0].ata_index = Some(machine.device_index[&crate::DeviceId::Rtc]);
        flush_ata(&mut machine.machine, &machine.persistence);
    }

    #[test]
    fn ide_journal_sink_failure_keeps_guest_writes_and_recovers_unsealed_prefix() {
        let temp = crate::test_support::TempDir::new("ide-sink-failure");
        let base = vec![0; 32768];
        let base_path = temp.path().join("base.img");
        let path = temp.path().join("ide.upfj");
        std::fs::write(&base_path, &base).unwrap();
        let mut config = crate::at_boot(vec![0xFA, 0xF4]);
        let cfg = crate::IdeCfg {
            model: crate::AtaControllerModel::Ata,
            base: 0x1F0,
            control: 0x3F6,
        };
        config.devices.push(
            crate::DeviceConfig::new(
                crate::DeviceId::Ide,
                crate::DeviceKind::Ide(cfg),
                cfg.port_windows(),
            )
            .with_irq(cfg.irq()),
        );
        config.drives.push(crate::DriveConfig {
            media: None,
            controller: crate::DeviceId::Ide,
            unit: 0,
            kind: crate::DriveKind::HardDisk {
                geometry: crate::HardDiskGeometry {
                    cylinders: 8,
                    heads: 2,
                    sectors_per_track: 4,
                    bytes_per_sector: 512,
                },
            },
            image: ImageSource::File(base_path.clone()),
            expected_sha256: None,
            write: WriteMode::journal(&path),
        });
        let mut machine = crate::assemble(&config).unwrap();
        for (lba, value) in [(1, 0x1234), (2, 0xABCD)] {
            if lba == 2 {
                machine.persistence[0].fail_output();
            }
            let bus = machine.machine.bus_mut();
            for (port, byte) in [
                (0x1F6, 0xE0),
                (0x1F2, 1),
                (0x1F3, lba),
                (0x1F4, 0),
                (0x1F5, 0),
                (0x1F7, 0x30),
            ] {
                bus.port_write(port, 1, byte).unwrap();
            }
            for _ in 0..256 {
                bus.port_write(0x1F0, 2, value).unwrap();
            }
            assert_eq!(bus.port_read(0x1F7, 1).unwrap(), 0x50);
            if lba == 1 {
                machine.check_persistence().unwrap();
            }
        }
        let index = machine.device_index[&crate::DeviceId::Ide];
        let before = machine.machine.bus().device(index).unwrap().save();
        assert!(machine
            .check_persistence()
            .unwrap_err()
            .to_string()
            .starts_with("drive 0 persistence:"));
        let device = machine.machine.bus_mut().device_mut(index).unwrap();
        assert_eq!(device.save(), before);
        let ata = device
            .as_any_mut()
            .unwrap()
            .downcast_mut::<devices::Ata>()
            .unwrap();
        assert_eq!(
            &ata.drive(0).unwrap().image()[1024..1536],
            &0xABCDu16.to_le_bytes().repeat(256)
        );
        assert!(machine.finish_persistence().is_err());
        drop(machine);
        let scan = media::scan(&std::fs::read(&path).unwrap(), &base, base.len() as u64);
        assert!(scan.report.generations.is_empty());
        assert_eq!(scan.report.unsealed_tip.unwrap().commands, 1);
        assert_eq!(
            &scan.image.unwrap()[512..1024],
            &0x1234u16.to_le_bytes().repeat(256)
        );
        let mut fresh = crate::assemble(&config).unwrap();
        assert!(fresh.persistence[0].mount_report().unsealed_tip.is_some());
        fresh.finish_persistence().unwrap();
        assert_eq!(std::fs::read(base_path).unwrap(), base);
    }

    #[test]
    fn failing_sink_preserves_guest_prefix_and_stops_at_frontend_boundary() {
        let temp = crate::test_support::TempDir::new("journal-test");
        let path = temp.path();
        let out = |program: &mut Vec<u8>, port: u16, value: u8| {
            program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
        };
        let mut program = vec![0xFA, 0xFC];
        out(&mut program, 0x3F2, 0x1C);
        out(&mut program, 0x3F7, 2);
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
            out(&mut program, 0x3F7, 2);
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
            machine.persistence[0]
                .checkpoint(&mut machine.machine)
                .unwrap();
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
            machine.persistence[0]
                .checkpoint(&mut machine.machine)
                .unwrap();
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
