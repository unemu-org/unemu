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
            "unemupc-offline-{}-{}",
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
    fn write_temporary(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        self.step("temporary write", target_kind(file)?)?;
        HostIo.write_temporary(file, bytes)
    }
    fn publish_temporary(&mut self, temp: &Path, path: &Path, replace: bool) -> io::Result<()> {
        self.step(
            if replace {
                "replace"
            } else {
                "no-clobber publish"
            },
            TargetKind::File,
        )?;
        HostIo.publish_temporary(temp, path, replace)
    }
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
    fn recover(
        &mut self,
        path: &Path,
        base: &VerifiedBase,
        lock: unemu_core_files::PathLock,
        base_metadata: &std::fs::Metadata,
    ) -> Result<JournalMedia, OpenError> {
        self.calls.push(("journal recovery", TargetKind::File));
        HostIo.recover(path, base, lock, base_metadata)
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

impl RecordingIo {
    fn publication_calls(&self) -> Vec<Event> {
        self.calls
            .iter()
            .copied()
            .filter(|(role, _)| {
                matches!(
                    *role,
                    "temporary write"
                        | "temporary sync"
                        | "no-clobber publish"
                        | "publication directory sync"
                )
            })
            .collect()
    }
}

#[test]
fn commit_records_real_write_verification_and_publication_barriers() {
    let f = Fixture::new();
    let original = std::fs::read(&f.journal).unwrap();
    let mut tool = JournalTool::open(&f.base, &f.journal, Access::Inspect).unwrap();
    let mut io = RecordingIo::default();
    let output = f.dir.join("export.img");
    tool.commit_with(&mut io, 1, &output).unwrap();
    let mut expected = f.bytes.clone();
    expected[..512].fill(0x11);
    assert_eq!(std::fs::read(&output).unwrap(), expected);
    assert_eq!(std::fs::read(&f.base).unwrap(), f.bytes);
    assert_eq!(std::fs::read(&f.journal).unwrap(), original);
    assert_eq!(
        io.publication_calls(),
        [
            ("temporary write", TargetKind::File),
            ("temporary sync", TargetKind::File),
            ("no-clobber publish", TargetKind::File),
            ("publication directory sync", TargetKind::Directory),
        ]
    );
}

#[test]
fn export_publication_failures_preserve_inputs_and_only_publish_verified_bytes() {
    for fail in [
        "temporary write",
        "temporary sync",
        "no-clobber publish",
        "publication directory sync",
    ] {
        let f = Fixture::new();
        let original = std::fs::read(&f.journal).unwrap();
        let output = f.dir.join("export.img");
        let mut io = RecordingIo {
            fail: Some(fail),
            ..RecordingIo::default()
        };
        assert!(f.tool().commit_with(&mut io, 2, &output).is_err());
        if output.exists() {
            let mut expected = f.bytes.clone();
            expected[..512].fill(0x22);
            assert_eq!(std::fs::read(&output).unwrap(), expected);
            assert_eq!(fail, "publication directory sync");
        } else {
            assert_ne!(fail, "publication directory sync");
        }
        assert_eq!(std::fs::read(&f.journal).unwrap(), original);
        assert_eq!(std::fs::read(&f.base).unwrap(), f.bytes);
    }
}

#[test]
fn export_checks_temporary_size_and_digest_before_any_barrier_or_publication() {
    struct CorruptingIo {
        truncate: bool,
    }
    impl ToolIo for CorruptingIo {
        fn write_temporary(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
            HostIo.write_temporary(file, bytes)?;
            if self.truncate {
                file.set_len(bytes.len() as u64 - 1)
            } else {
                file.seek(SeekFrom::Start(0))?;
                file.write_all(&[bytes[0] ^ 1])
            }
        }
        fn sync(&mut self, _: &File, _: &'static str) -> io::Result<()> {
            panic!("unverified export synchronized")
        }
        fn publish_temporary(&mut self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            panic!("unverified export published")
        }
    }
    for truncate in [false, true] {
        let f = Fixture::new();
        let output = f.dir.join("export.img");
        assert!(f
            .tool()
            .commit_with(&mut CorruptingIo { truncate }, 1, &output)
            .unwrap_err()
            .to_string()
            .contains("size/digest"));
        assert!(!output.exists());
    }
}

#[test]
fn export_no_clobber_handles_destination_created_at_publication() {
    struct RacingIo;
    impl ToolIo for RacingIo {
        fn publish_temporary(
            &mut self,
            temporary: &Path,
            path: &Path,
            replace: bool,
        ) -> io::Result<()> {
            assert!(!replace);
            std::fs::write(path, b"existing destination")?;
            HostIo.publish_temporary(temporary, path, replace)
        }
    }
    let f = Fixture::new();
    let output = f.dir.join("export.img");
    assert!(matches!(
        f.tool().commit_with(&mut RacingIo, 1, &output),
        Err(ToolError::Config(_))
    ));
    assert_eq!(std::fs::read(output).unwrap(), b"existing destination");
}

#[test]
fn export_skips_occupied_temporary_aliases_without_truncating_inputs() {
    let f = Fixture::new();
    let original = std::fs::read(&f.journal).unwrap();
    let output = f.dir.join("export.img");
    let occupied = f.dir.join("export.img.export-0.tmp");
    std::fs::hard_link(&f.base, &occupied).unwrap();
    f.tool().commit(1, &output).unwrap();
    assert_eq!(std::fs::read(&f.base).unwrap(), f.bytes);
    assert_eq!(std::fs::read(occupied).unwrap(), f.bytes);
    assert_eq!(std::fs::read(&f.journal).unwrap(), original);
    assert_eq!(&std::fs::read(output).unwrap()[..512], &[0x11; 512]);
}

#[test]
fn export_errors_distinguish_unpublished_and_uncertain_with_exact_messages() {
    struct Fault {
        phase: &'static str,
        code: Option<i32>,
    }
    impl Fault {
        fn error(&self) -> io::Error {
            self.code.map_or_else(
                || io::Error::other("injected export failure"),
                io::Error::from_raw_os_error,
            )
        }
    }
    impl ToolIo for Fault {
        fn write_temporary(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
            if self.phase == "temporary write" {
                return Err(self.error());
            }
            file.write_all(bytes)?;
            if self.phase == "digest" {
                file.seek(SeekFrom::Start(0))?;
                file.write_all(&[bytes[0] ^ 1])?;
            }
            Ok(())
        }
        fn sync(&mut self, file: &File, role: &'static str) -> io::Result<()> {
            if self.phase == role {
                return Err(self.error());
            }
            HostIo.sync(file, role)
        }
        fn publish_temporary(&mut self, temp: &Path, path: &Path, replace: bool) -> io::Result<()> {
            if self.phase == "publish" {
                return Err(self.error());
            }
            HostIo.publish_temporary(temp, path, replace)
        }
    }
    let cases = [
        (
            "temporary write",
            None,
            "export failed; nothing was published; output",
        ),
        (
            "digest",
            None,
            "export failed; nothing was published; output",
        ),
        (
            "temporary sync",
            None,
            "export failed; nothing was published; output",
        ),
        ("publish", None, "export publication uncertain; output"),
        (
            "publication directory sync",
            None,
            "export publication uncertain; output",
        ),
    ];
    for (phase, code, role) in cases {
        let f = Fixture::new();
        let output = f.dir.join("export.img");
        let error = f
            .tool()
            .commit_with(&mut Fault { phase, code }, 1, &output)
            .unwrap_err();
        let detail = if phase == "digest" {
            "export size/digest verification failed"
        } else {
            "injected export failure"
        };
        assert_eq!(
            error.to_string(),
            format!("{role} {}: {detail}", output.display())
        );
        assert_eq!(output.exists(), phase == "publication directory sync");
    }
    #[cfg(unix)]
    for code in [libc::EPERM, libc::ENOTSUP, libc::EXDEV] {
        let f = Fixture::new();
        let output = f.dir.join("export.img");
        let error = f
            .tool()
            .commit_with(
                &mut Fault {
                    phase: "publish",
                    code: Some(code),
                },
                1,
                &output,
            )
            .unwrap_err();
        assert_eq!(error.to_string(),format!("export needs hard-link support on the output filesystem; nothing was published; output {}: {}",output.display(),io::Error::from_raw_os_error(code)));
        assert!(!output.exists());
    }
    #[cfg(unix)]
    {
        let f = Fixture::new();
        let output = f.dir.join("export.img");
        let error = f
            .tool()
            .commit_with(
                &mut Fault {
                    phase: "publication directory sync",
                    code: Some(libc::EPERM),
                },
                1,
                &output,
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "export publication uncertain; output {}: {}",
                output.display(),
                io::Error::from_raw_os_error(libc::EPERM)
            )
        );
        assert!(output.exists());
    }
}

#[test]
fn nvram_offline_retention_export_rollback_and_tail_repair() {
    let fixture = Fixture::new();
    let base: [u8; 64] = std::array::from_fn(|i| i as u8);
    let base_path = fixture.dir.join("cmos.bin");
    let journal_path = fixture.dir.join("cmos.upfj");
    std::fs::write(&base_path, base).unwrap();
    let mut journal = JournalMedia::open(&journal_path, &base, 64, SinkStatus::default())
        .unwrap()
        .with_retention(2);
    for value in 1..=4 {
        let mut block = journal.read_sector(0);
        assert_eq!(&block[..14], &base[..14]);
        assert!(block[64..].iter().all(|&b| b == 0));
        block[63] = value;
        journal.write_batch(&[(0, block)]);
        journal.checkpoint().unwrap();
    }
    drop(journal);
    let mut tool = JournalTool::open(&base_path, &journal_path, Access::Mutate).unwrap();
    let scan = tool.verify().unwrap();
    assert_eq!(
        scan.report
            .generations
            .iter()
            .map(|g| g.id)
            .collect::<Vec<_>>(),
        [3, 4]
    );
    assert_eq!(scan.overlay.as_ref().unwrap().byte_len(), 64);
    assert_eq!(
        scan.overlay.as_ref().unwrap().read_sector(0).unwrap()[63],
        4
    );
    assert_eq!(scan.report.header.unwrap().anchor_generation, 3);
    let exported = fixture.dir.join("exported.bin");
    tool.commit(3, &exported).unwrap();
    let bytes = std::fs::read(exported).unwrap();
    assert_eq!(bytes.len(), 64);
    assert_eq!(&bytes[..63], &base[..63]);
    assert_eq!(bytes[63], 3);
    let rollback = tool
        .rollback(3, &fixture.dir.join("rollback.upfj"))
        .unwrap();
    assert_eq!(
        rollback.overlay.as_ref().unwrap().read_sector(0).unwrap()[63],
        3
    );
    drop(tool);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&journal_path)
        .unwrap()
        .write_all(&[0xAA; 9])
        .unwrap();
    let mut tool = JournalTool::open(&base_path, &journal_path, Access::Mutate).unwrap();
    assert_eq!(tool.verify().unwrap().report.discarded_bytes, 9);
    let repaired = tool.repair_tail(&fixture.dir.join("repair.upfj")).unwrap();
    assert_eq!(
        repaired.overlay.as_ref().unwrap().read_sector(0).unwrap()[63],
        3
    );
    assert!(!repaired.report.needs_recovery());
    assert_eq!(std::fs::read(base_path).unwrap(), base);
}
