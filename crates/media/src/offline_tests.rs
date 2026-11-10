// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real-file traces and failures for persistent-media.md § Offline commands.

use super::*;
use crate::{SinkStatus, FRAME_LEN};
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    dir: PathBuf,
    base: PathBuf,
    journal: PathBuf,
    bytes: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "unemupc98-offline-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let base = dir.join("base.img");
        let journal = dir.join("disk.upfj");
        let bytes = vec![0; 1024];
        std::fs::write(&base, &bytes).unwrap();
        let mut disk = JournalMedia::open(&journal, &bytes, 1024, SinkStatus::default()).unwrap();
        disk.write_batch(&[(0, [0x11; 512])]);
        disk.checkpoint().unwrap();
        disk.write_batch(&[(0, [0x22; 512])]);
        disk.finish().unwrap();
        Self {
            dir,
            base,
            journal,
            bytes,
        }
    }
    fn tool(&self) -> JournalTool {
        JournalTool::open(&self.base, &self.journal, Access::Mutate).unwrap()
    }
    fn backup(&self) -> PathBuf {
        self.dir.join("before.upfj")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetKind {
    File,
    Directory,
    Other,
}

fn target_kind(file: &File) -> io::Result<TargetKind> {
    let metadata = file.metadata()?;
    Ok(if metadata.is_file() {
        TargetKind::File
    } else if metadata.is_dir() {
        TargetKind::Directory
    } else {
        TargetKind::Other
    })
}

type Event = (&'static str, TargetKind);

#[derive(Default)]
struct RecordingIo {
    calls: Vec<Event>,
    fail: Option<&'static str>,
}
impl RecordingIo {
    fn step(&mut self, role: &'static str, target: TargetKind) -> io::Result<()> {
        self.calls.push((role, target));
        if self.fail == Some(role) {
            return Err(io::Error::other("injected offline failure"));
        }
        Ok(())
    }
}
impl ToolIo for RecordingIo {
    fn create_backup(&mut self, path: &Path, permissions: &Permissions) -> io::Result<File> {
        self.step("backup create", TargetKind::File)?;
        HostIo.create_backup(path, permissions)
    }
    fn copy(&mut self, journal: &mut File, backup: &mut File) -> io::Result<()> {
        self.step("copy", target_kind(backup)?)?;
        HostIo.copy(journal, backup)
    }
    fn sync(&mut self, file: &File, role: &'static str) -> io::Result<()> {
        self.step(role, target_kind(file)?)?;
        HostIo.sync(file, role)
    }
    fn truncate(&mut self, file: &File, end: u64) -> io::Result<()> {
        self.step("journal truncate", target_kind(file)?)?;
        HostIo.truncate(file, end)
    }
    fn recover(&mut self, path: &Path, base: &[u8], lock: File) -> Result<JournalMedia, OpenError> {
        self.calls.push(("journal recovery", TargetKind::File));
        HostIo.recover(path, base, lock)
    }
}

const BACKUP: [Event; 4] = [
    ("backup create", TargetKind::File),
    ("copy", TargetKind::File),
    ("backup file sync", TargetKind::File),
    ("backup directory sync", TargetKind::Directory),
];

#[test]
fn rollback_records_production_backup_truncate_and_sync_order() {
    let f = Fixture::new();
    let original = std::fs::read(&f.journal).unwrap();
    let mut io = RecordingIo::default();
    f.tool().rollback_with(&mut io, 1, &f.backup()).unwrap();
    assert_eq!(
        io.calls,
        [
            BACKUP.as_slice(),
            &[
                ("journal truncate", TargetKind::File),
                ("journal sync", TargetKind::File)
            ]
        ]
        .concat()
    );
    assert_eq!(std::fs::read(f.backup()).unwrap(), original);
    assert_eq!(
        std::fs::metadata(&f.journal).unwrap().len(),
        (HEADER_LEN + 3 * FRAME_LEN) as u64
    );
}

#[test]
fn repair_records_production_backup_barriers_before_normal_recovery() {
    let f = Fixture::new();
    let mut disk = JournalMedia::open(&f.journal, &f.bytes, 1024, SinkStatus::default()).unwrap();
    disk.write_batch(&[(1, [0x33; 512])]);
    drop(disk);
    use std::io::Write;
    OpenOptions::new()
        .append(true)
        .open(&f.journal)
        .unwrap()
        .write_all(b"torn")
        .unwrap();
    let original = std::fs::read(&f.journal).unwrap();
    let mut io = RecordingIo::default();
    let scan = f.tool().repair_with(&mut io, &f.backup()).unwrap();
    assert_eq!(
        io.calls,
        [BACKUP.as_slice(), &[("journal recovery", TargetKind::File)]].concat()
    );
    assert_eq!(std::fs::read(f.backup()).unwrap(), original);
    assert!(!scan.report.needs_recovery());
    assert_eq!(
        scan.report.generations.last().unwrap().reason,
        crate::ClosingReason::Recovered
    );
}

#[test]
fn production_backup_failures_leave_declared_invalid_backup_and_no_mutation() {
    for repair in [false, true] {
        for fail in ["copy", "backup file sync", "backup directory sync"] {
            let f = Fixture::new();
            let original = std::fs::read(&f.journal).unwrap();
            let mut io = RecordingIo {
                fail: Some(fail),
                ..RecordingIo::default()
            };
            let result = if repair {
                f.tool().repair_with(&mut io, &f.backup())
            } else {
                f.tool().rollback_with(&mut io, 0, &f.backup())
            };
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("partial, invalid backup was left"),
                "{error}"
            );
            assert!(error.contains(f.backup().to_str().unwrap()), "{error}");
            assert!(f.backup().exists());
            assert_eq!(std::fs::read(&f.journal).unwrap(), original);
            let position = BACKUP.iter().position(|&(role, _)| role == fail).unwrap();
            assert_eq!(io.calls, BACKUP[..=position]);
        }
    }
}

#[test]
fn production_rollback_stops_on_failed_truncate_or_sync_with_journal_context() {
    for fail in ["journal truncate", "journal sync"] {
        let f = Fixture::new();
        let original = std::fs::read(&f.journal).unwrap();
        let mut io = RecordingIo {
            fail: Some(fail),
            ..RecordingIo::default()
        };
        let error = f
            .tool()
            .rollback_with(&mut io, 0, &f.backup())
            .unwrap_err()
            .to_string();
        assert!(error.contains("journal"));
        assert!(error.contains(f.journal.to_str().unwrap()));
        assert_eq!(std::fs::read(f.backup()).unwrap(), original);
        assert_eq!(io.calls.last(), Some(&(fail, TargetKind::File)));
    }
}

#[cfg(unix)]
#[test]
fn post_lock_path_recheck_rejects_a_swapped_symlink_as_io() {
    struct SwappingIo {
        target: PathBuf,
        saved: PathBuf,
        rechecked: bool,
    }
    impl ToolIo for SwappingIo {
        fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
            self.rechecked = true;
            std::fs::rename(path, &self.saved)?;
            std::os::unix::fs::symlink(&self.target, path)?;
            HostIo.canonicalize(path)
        }
    }
    for access in [Access::Inspect, Access::Mutate] {
        let f = Fixture::new();
        let original = std::fs::read(&f.journal).unwrap();
        let target = f.dir.join("target.upfj");
        std::fs::copy(&f.journal, &target).unwrap();
        let mut io = SwappingIo {
            target: target.clone(),
            saved: f.dir.join("saved.upfj"),
            rechecked: false,
        };
        let error = match JournalTool::open_with(&mut io, &f.base, &f.journal, access) {
            Err(error) => error,
            Ok(_) => panic!("accepted a changed journal path"),
        };
        assert!(io.rechecked);
        assert!(matches!(error, ToolError::PathIo { .. }));
        assert!(error
            .to_string()
            .contains("journal canonical path changed after locking"));
        assert!(error.to_string().contains(f.journal.to_str().unwrap()));
        assert_eq!(std::fs::read(&io.saved).unwrap(), original);
        assert_eq!(std::fs::read(target).unwrap(), original);
    }
}

#[cfg(unix)]
#[test]
fn backup_is_created_with_private_mode_before_copy_and_keeps_exact_permissions() {
    use std::os::unix::fs::PermissionsExt;
    struct ModeIo {
        created: Option<u32>,
        copied: Option<u32>,
    }
    impl ToolIo for ModeIo {
        fn create_backup(&mut self, path: &Path, permissions: &Permissions) -> io::Result<File> {
            let file = HostIo.create_backup(path, permissions)?;
            self.created = Some(file.metadata()?.permissions().mode() & 0o777);
            // Simulate a restrictive umask without changing process-global state.
            file.set_permissions(Permissions::from_mode(0o400))?;
            Ok(file)
        }
        fn copy(&mut self, journal: &mut File, backup: &mut File) -> io::Result<()> {
            self.copied = Some(backup.metadata()?.permissions().mode() & 0o777);
            HostIo.copy(journal, backup)
        }
    }
    let f = Fixture::new();
    std::fs::set_permissions(&f.journal, Permissions::from_mode(0o600)).unwrap();
    let mut io = ModeIo {
        created: None,
        copied: None,
    };
    f.tool().rollback_with(&mut io, 1, &f.backup()).unwrap();
    assert_eq!(io.created.unwrap() & !0o600, 0);
    assert_eq!(io.copied, Some(0o600));
    assert_eq!(
        std::fs::metadata(f.backup()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
