// SPDX-License-Identifier: AGPL-3.0-or-later
//! Offline floppy journal contracts from persistent-media.md § Offline commands.
//! All images are generated; temporary directories live outside the repository.

use cli::command::{parse, run, Command};
use media::offline::{Access, JournalTool};
use media::{
    ClosingReason, Frame, Header, JournalMedia, Record, SinkStatus, FRAME_LEN, HEADER_LEN,
};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
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
            "unemupc-media-tools-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let base = dir.join("base.img");
        let journal = dir.join("disk.upfj");
        let bytes = vec![0; 1024];
        fs::write(&base, &bytes).unwrap();
        Self {
            dir,
            base,
            journal,
            bytes,
        }
    }

    fn mount(&self) -> JournalMedia {
        JournalMedia::open(&self.journal, &self.bytes, 1024, SinkStatus::default()).unwrap()
    }

    fn two_generations(&self) {
        let mut disk = self.mount();
        disk.write_batch(&[(0, [0x11; 512])]);
        disk.checkpoint().unwrap();
        disk.write_batch(&[(0, [0x22; 512]), (1, [0x33; 512])]);
        disk.finish().unwrap();
    }

    fn invoke(&self, operation: &str, extra: &[&str]) -> (u8, String, String) {
        let mut args: Vec<OsString> = vec![
            "media".into(),
            operation.into(),
            "--base".into(),
            self.base.clone().into(),
            "--journal".into(),
            self.journal.clone().into(),
        ];
        args.extend(extra.iter().map(OsString::from));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn backup(&self) -> PathBuf {
        self.dir.join("backup.upfj")
    }

    fn rollback(&self, generation: &str) -> (u8, String, String) {
        self.invoke(
            "rollback",
            &[
                "--to",
                generation,
                "--backup",
                self.backup().to_str().unwrap(),
            ],
        )
    }

    fn repair(&self) -> (u8, String, String) {
        self.invoke(
            "verify",
            &["--repair-tail", "--backup", self.backup().to_str().unwrap()],
        )
    }

    fn append_torn(&self) {
        OpenOptions::new()
            .append(true)
            .open(&self.journal)
            .unwrap()
            .write_all(b"torn")
            .unwrap();
    }

    fn corrupt_interior(&self) {
        let mut bytes = fs::read(&self.journal).unwrap();
        bytes[HEADER_LEN + 64] ^= 1;
        fs::write(&self.journal, bytes).unwrap();
    }

    fn anchor(&self, complete: bool) {
        let mut bytes = Header::new(&self.bytes, 7)
            .unwrap()
            .encode()
            .unwrap()
            .to_vec();
        for frame in [
            Frame {
                sequence: 1,
                generation: 7,
                record: Record::Sector {
                    lba: 0,
                    bytes: Box::new([0x77; 512]),
                },
            },
            Frame {
                sequence: 2,
                generation: 7,
                record: Record::CommandEnd { sectors: 1 },
            },
            Frame {
                sequence: 3,
                generation: 7,
                record: Record::GenerationEnd {
                    reason: ClosingReason::RetentionAnchor,
                },
            },
        ]
        .iter()
        .take(if complete { 3 } else { 2 })
        {
            bytes.extend_from_slice(&frame.encode().unwrap());
        }
        fs::write(&self.journal, bytes).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}

#[test]
fn rollback_remount_reappend_reuses_next_generation_and_discards_tip() {
    let f = Fixture::new();
    f.two_generations();
    let mut disk = f.mount();
    disk.write_batch(&[(1, [0x44; 512])]);
    drop(disk);
    let original = fs::read(&f.journal).unwrap();
    assert_eq!(f.rollback("1").0, 0);
    assert_eq!(fs::read(f.backup()).unwrap(), original);
    assert_eq!(
        fs::metadata(&f.journal).unwrap().len(),
        (HEADER_LEN + 3 * FRAME_LEN) as u64
    );
    let mut disk = f.mount();
    let mut expected = f.bytes.clone();
    expected[..512].fill(0x11);
    assert_eq!(disk.image(), expected);
    disk.write_batch(&[(0, [0x55; 512])]);
    disk.finish().unwrap();
    let mut tool = JournalTool::open(&f.base, &f.journal, Access::Inspect).unwrap();
    let scan = tool.verify().unwrap();
    assert_eq!(
        scan.report
            .generations
            .iter()
            .map(|g| g.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    expected[..512].fill(0x55);
    assert_eq!(scan.overlay.unwrap().materialize().unwrap(), expected);
    assert_eq!(f.invoke("verify", &[]).0, 0);
    assert_eq!(fs::read(&f.base).unwrap(), f.bytes);
}

#[test]
fn rollback_zero_is_exactly_the_header() {
    let f = Fixture::new();
    f.two_generations();
    assert_eq!(f.rollback("0").0, 0);
    assert_eq!(fs::metadata(&f.journal).unwrap().len(), 128);
    let disk = f.mount();
    assert_eq!(disk.image(), f.bytes);
    assert_eq!(disk.mount_report().last_closed_generation, Some(0));
}

#[test]
fn rollback_unavailable_ids_are_config_errors_without_backup() {
    let f = Fixture::new();
    f.two_generations();
    let original = fs::read(&f.journal).unwrap();
    assert_eq!(f.rollback("99").0, 2);
    assert!(!f.backup().exists());
    assert_eq!(fs::read(&f.journal).unwrap(), original);
    f.anchor(true);
    for id in ["0", "1", "6", "8"] {
        assert_eq!(f.rollback(id).0, 2, "{id}");
        assert!(!f.backup().exists());
    }
    assert_eq!(f.rollback("7").0, 0);
    assert!(f
        .invoke("generations", &[])
        .1
        .contains("retained anchor: 7"));
}

#[test]
fn verify_exit_contract_covers_clean_tail_tip_damage_base_and_missing() {
    let f = Fixture::new();
    assert_eq!(f.invoke("verify", &[]).0, 1);
    assert!(!f.journal.exists());
    f.two_generations();
    assert_eq!(f.invoke("verify", &[]).0, 0);
    let clean = fs::read(&f.journal).unwrap();
    f.append_torn();
    let torn = fs::read(&f.journal).unwrap();
    let (code, output, _) = f.invoke("verify", &[]);
    assert_eq!(code, 3);
    assert!(output.contains("recoverable-tail bytes: 4"));
    assert!(output.contains("discarded bytes are uncertain"));
    assert_eq!(fs::read(&f.journal).unwrap(), torn);
    fs::write(&f.journal, &clean).unwrap();
    let mut disk = f.mount();
    disk.write_batch(&[(1, [0x44; 512])]);
    drop(disk);
    let unsealed = fs::read(&f.journal).unwrap();
    let (code, output, _) = f.invoke("verify", &[]);
    assert_eq!(code, 3);
    assert!(output.contains("unsealed generation 3: commands 1, sectors 1"));
    assert!(output.contains("generation 3 (unsealed)"));
    assert_eq!(fs::read(&f.journal).unwrap(), unsealed);
    fs::write(&f.journal, &clean).unwrap();
    f.corrupt_interior();
    let damaged = fs::read(&f.journal).unwrap();
    let (code, output, _) = f.invoke("verify", &[]);
    assert_eq!(code, 1);
    assert!(output.contains("interior damage:"));
    assert!(output.contains("latest reconstructible state: unavailable"));
    assert_eq!(fs::read(&f.journal).unwrap(), damaged);
    fs::write(&f.journal, clean).unwrap();
    fs::write(&f.base, vec![1; 1024]).unwrap();
    assert_eq!(f.invoke("verify", &[]).0, 1);
}

#[test]
fn repair_backs_up_full_torn_tip_then_seals_as_interrupted() {
    let f = Fixture::new();
    f.two_generations();
    let mut disk = f.mount();
    disk.write_batch(&[(1, [0x66; 512])]);
    drop(disk);
    f.append_torn();
    let original = fs::read(&f.journal).unwrap();
    let (code, output, errors) = f.repair();
    assert_eq!(code, 0, "{errors}");
    assert_eq!(fs::read(f.backup()).unwrap(), original);
    assert!(output.contains("generation 3: recovered interrupted session"));
    assert!(!output.contains("generation 4:"));
    assert_eq!(f.invoke("verify", &[]).0, 0);
    let disk = f.mount();
    assert_eq!(disk.read_sector(1), [0x66; 512]);
    assert_eq!(disk.mount_report().last_closed_generation, Some(3));
}

#[test]
fn repair_refuses_existing_backup_interior_damage_and_incomplete_anchor() {
    let f = Fixture::new();
    f.two_generations();
    f.append_torn();
    let original = fs::read(&f.journal).unwrap();
    fs::write(f.backup(), b"keep this backup").unwrap();
    assert_eq!(f.repair().0, 2);
    assert_eq!(fs::read(f.backup()).unwrap(), b"keep this backup");
    assert_eq!(fs::read(&f.journal).unwrap(), original);
    fs::remove_file(f.backup()).unwrap();
    f.corrupt_interior();
    let damaged = fs::read(&f.journal).unwrap();
    assert_eq!(f.repair().0, 1);
    assert_eq!(f.rollback("0").0, 1);
    assert_eq!(fs::read(&f.journal).unwrap(), damaged);
    assert!(!f.backup().exists());
    f.anchor(false);
    let anchor = fs::read(&f.journal).unwrap();
    assert_eq!(f.invoke("verify", &[]).0, 1);
    assert_eq!(f.repair().0, 1);
    assert_eq!(f.rollback("0").0, 1);
    assert_eq!(fs::read(&f.journal).unwrap(), anchor);
    assert!(!f.backup().exists());
}

#[test]
fn backup_aliases_and_reserved_names_are_refused_without_changing_inputs() {
    let f = Fixture::new();
    f.two_generations();
    let journal = fs::read(&f.journal).unwrap();
    let linked = f.dir.join("linked");
    fs::hard_link(&f.journal, &linked).unwrap();
    for path in [
        &f.base,
        &f.journal,
        &linked,
        &f.dir.join("disk.upfj.create-3.tmp"),
    ] {
        for operation in ["verify", "rollback"] {
            let mut extra = vec!["--backup", path.to_str().unwrap()];
            if operation == "verify" {
                extra.push("--repair-tail");
            } else {
                extra.extend(["--to", "0"]);
            }
            assert_eq!(f.invoke(operation, &extra).0, 2);
            assert_eq!(fs::read(&f.journal).unwrap(), journal);
            assert_eq!(fs::read(&f.base).unwrap(), f.bytes);
        }
    }
}

#[test]
fn exclusive_session_excludes_all_tools_and_shared_inspections_coexist() {
    let f = Fixture::new();
    f.two_generations();
    let disk = f.mount();
    let journal = fs::read(&f.journal).unwrap();
    for result in [
        f.rollback("1"),
        f.repair(),
        f.invoke("verify", &[]),
        f.invoke("generations", &[]),
    ] {
        assert_eq!(result.0, 1);
        assert!(result.2.contains("journal is locked"));
    }
    assert_eq!(fs::read(&f.journal).unwrap(), journal);
    assert!(!f.backup().exists());
    drop(disk);
    let mut first = JournalTool::open(&f.base, &f.journal, Access::Inspect).unwrap();
    assert_eq!(f.invoke("verify", &[]).0, 0);
    assert_eq!(f.invoke("generations", &[]).0, 0);
    assert_eq!(f.rollback("1").0, 1);
    assert!(first.rollback(0, &f.backup()).is_err());
    assert!(first.repair_tail(&f.backup()).is_err());
}

#[cfg(unix)]
#[test]
fn journal_symlink_uses_the_sessions_canonical_lock() {
    let f = Fixture::new();
    f.two_generations();
    let alias = f.dir.join("alias.upfj");
    std::os::unix::fs::symlink(&f.journal, &alias).unwrap();
    let disk = f.mount();
    for access in [Access::Inspect, Access::Mutate] {
        assert!(JournalTool::open(&f.base, &alias, access).is_err());
    }
    drop(disk);
    assert!(JournalTool::open(&f.base, &alias, Access::Inspect).is_ok());
    assert!(!f.dir.join("alias.upfj.lock").exists());
}

#[test]
fn generations_text_is_stable_and_reports_recovered_history_and_open_tip() {
    let f = Fixture::new();
    f.two_generations();
    let mut disk = f.mount();
    disk.write_batch(&[(0, [0x44; 512])]);
    drop(disk);
    drop(f.mount());
    let mut disk = f.mount();
    disk.write_batch(&[(1, [0x55; 512])]);
    drop(disk);
    let (code, output, errors) = f.invoke("generations", &[]);
    assert_eq!(code, 3, "{errors}");
    assert_eq!(
        output,
        concat!(
            "base sha256: 5f70bf18a086007016e948b04aed3b82103a36bea41755b6cddfaf10ace3c6ef\n",
            "retained anchor: 0\n",
            "generation 0: immutable base, end 128, commands 0, sectors 0\n",
            "generation 1: explicit checkpoint, end 2048, commands 1, sectors 1\n",
            "generation 2: session end, end 4608, commands 1, sectors 2\n",
            "generation 3: recovered interrupted session, end 6528, commands 1, sectors 1\n",
            "unsealed generation 4: commands 1, sectors 1\n",
            "recoverable-tail bytes: 0\n",
            "latest reconstructible state: generation 4 (unsealed), end 7808\n",
            "status: recovery required\n",
            "closed markers do not establish filesystem health or bootability\n",
        )
    );
}

#[test]
fn usage_errors_are_two_and_boot_still_runs() {
    for arguments in [
        vec!["--unknown"],
        vec!["--boot", "extra"],
        vec!["media"],
        vec!["media", "commit"],
        vec!["media", "compact"],
        vec!["media", "verify"],
        vec!["media", "verify", "--base"],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--journal",
            "b",
            "--repair-tail",
        ],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--journal",
            "b",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "0",
        ],
        vec![
            "media",
            "generations",
            "--base",
            "a",
            "--journal",
            "b",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "generations",
            "--base",
            "a",
            "--journal",
            "b",
            "--repair-tail",
        ],
        vec![
            "media",
            "rollback",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "-1",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "rollback",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "+1",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "rollback",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "18446744073709551616",
            "--backup",
            "c",
        ],
        vec![
            "media",
            "rollback",
            "--base",
            "a",
            "--journal",
            "b",
            "--to",
            "0",
        ],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--base",
            "b",
            "--journal",
            "c",
        ],
        vec![
            "media",
            "verify",
            "--base",
            "a",
            "--journal",
            "b",
            "--repair-tail",
            "--repair-tail",
            "--backup",
            "c",
        ],
    ] {
        let args = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run(&args, &mut out, &mut err), 2, "{arguments:?}");
        assert!(out.is_empty());
        assert!(String::from_utf8(err).unwrap().contains("usage:"));
    }
    assert_eq!(parse(&["--boot".into()]).unwrap(), Command::Boot);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(run(&["--boot".into()], &mut out, &mut err), 0);
    assert_eq!(out, b".....");
    assert!(err.is_empty());
    out.clear();
    assert_eq!(run(&[], &mut out, &mut err), 0);
    assert!(String::from_utf8(out).unwrap().contains("media verify"));
}

#[cfg(unix)]
#[test]
fn parser_preserves_non_utf8_paths() {
    use std::os::unix::ffi::OsStringExt;
    let base = OsString::from_vec(vec![b'i', 0xff]);
    let command = parse(&[
        "media".into(),
        "verify".into(),
        "--base".into(),
        base.clone(),
        "--journal".into(),
        "j".into(),
    ])
    .unwrap();
    let Command::Media(command) = command else {
        panic!("expected media command")
    };
    assert_eq!(command.base, Path::new(&base));
}

#[test]
fn rollback_refuses_an_unsealed_target_and_backs_up_a_torn_suffix() {
    let f = Fixture::new();
    f.two_generations();
    let mut disk = f.mount();
    disk.write_batch(&[(0, [0x44; 512])]);
    drop(disk);
    assert_eq!(f.rollback("3").0, 2);
    assert!(!f.backup().exists());
    f.append_torn();
    let original = fs::read(&f.journal).unwrap();
    assert_eq!(f.rollback("2").0, 0);
    assert_eq!(fs::read(f.backup()).unwrap(), original);
    assert_eq!(f.invoke("verify", &[]).0, 0);
}

#[test]
fn backup_creation_failure_prevents_both_mutations() {
    let f = Fixture::new();
    f.two_generations();
    f.append_torn();
    let original = fs::read(&f.journal).unwrap();
    let missing = f.dir.join("missing").join("backup");
    assert_eq!(
        f.invoke(
            "verify",
            &["--repair-tail", "--backup", missing.to_str().unwrap()]
        )
        .0,
        1
    );
    assert_eq!(
        f.invoke(
            "rollback",
            &["--to", "0", "--backup", missing.to_str().unwrap()]
        )
        .0,
        1
    );
    assert_eq!(fs::read(&f.journal).unwrap(), original);
    assert!(!missing.exists());
}

#[cfg(unix)]
#[test]
fn inspection_keeps_read_only_inputs_and_creation_leftovers_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.two_generations();
    f.append_torn();
    let original = fs::read(&f.journal).unwrap();
    let temporary = f.dir.join("disk.upfj.create-0.tmp");
    fs::write(&temporary, []).unwrap();
    fs::set_permissions(&f.base, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&f.journal, fs::Permissions::from_mode(0o444)).unwrap();
    for operation in ["verify", "generations"] {
        assert_eq!(f.invoke(operation, &[]).0, 3);
        assert_eq!(fs::read(&f.journal).unwrap(), original);
        assert_eq!(fs::read(&f.base).unwrap(), f.bytes);
        assert!(temporary.exists());
    }
}

#[cfg(unix)]
#[test]
fn dangling_journal_and_backup_symlinks_are_refused() {
    let f = Fixture::new();
    std::os::unix::fs::symlink(f.dir.join("missing"), &f.journal).unwrap();
    assert_eq!(f.invoke("verify", &[]).0, 1);
    assert!(!f.dir.join("disk.upfj.lock").exists());
    fs::remove_file(&f.journal).unwrap();
    f.two_generations();
    let original = fs::read(&f.journal).unwrap();
    for target in [&f.base, &f.journal, &f.dir.join("missing")] {
        let backup = f.dir.join(format!(
            "backup-{}",
            target.file_name().unwrap().to_str().unwrap()
        ));
        std::os::unix::fs::symlink(target, &backup).unwrap();
        assert_eq!(
            f.invoke(
                "verify",
                &["--repair-tail", "--backup", backup.to_str().unwrap()]
            )
            .0,
            2
        );
        assert_eq!(fs::read(&f.journal).unwrap(), original);
    }
}

#[test]
fn missing_inputs_and_backup_directory_name_their_role_and_path() {
    let f = Fixture::new();
    let (code, _, error) = f.invoke("verify", &[]);
    assert_eq!(code, 1);
    assert!(error.contains("journal"));
    assert!(error.contains(f.journal.to_str().unwrap()));
    assert!(!f.dir.join("disk.upfj.lock").exists());
    fs::remove_file(&f.base).unwrap();
    let (code, _, error) = f.invoke("verify", &[]);
    assert_eq!(code, 1);
    assert!(error.contains("base"));
    assert!(error.contains(f.base.to_str().unwrap()));
    assert!(!f.dir.join("disk.upfj.lock").exists());
    fs::write(&f.base, &f.bytes).unwrap();
    f.two_generations();
    fs::remove_file(&f.base).unwrap();
    fs::remove_file(f.dir.join("disk.upfj.lock")).unwrap();
    let (code, _, error) = f.invoke("verify", &[]);
    assert_eq!(code, 1);
    assert!(error.contains("base"));
    assert!(error.contains(f.base.to_str().unwrap()));
    assert!(!f.dir.join("disk.upfj.lock").exists());
    fs::write(&f.base, &f.bytes).unwrap();
    let backup = f.dir.join("missing-directory").join("before.upfj");
    for operation in ["verify", "rollback"] {
        let mut extra = vec!["--backup", backup.to_str().unwrap()];
        if operation == "verify" {
            extra.push("--repair-tail");
        } else {
            extra.extend(["--to", "0"]);
        }
        let (code, _, error) = f.invoke(operation, &extra);
        assert_eq!(code, 1);
        assert!(error.contains("backup"), "{error}");
        assert!(error.contains(backup.to_str().unwrap()), "{error}");
        assert!(!error.contains("partial, invalid"));
    }
}

#[test]
fn rollback_warns_only_for_a_valid_target() {
    let f = Fixture::new();
    f.two_generations();
    let (code, output, _) = f.rollback("99");
    assert_eq!(code, 2);
    assert!(!output.contains("will be removed"));
    let (code, output, _) = f.rollback("1");
    assert_eq!(code, 0);
    assert!(output.starts_with(
        "rollback to generation 1: later generations and any open tip will be removed\n"
    ));
}

#[test]
fn pending_sector_tail_golden_names_the_actual_truncation_point() {
    let f = Fixture::new();
    f.two_generations();
    let mut bytes = fs::read(&f.journal).unwrap();
    bytes.extend_from_slice(
        &Frame {
            sequence: 8,
            generation: 3,
            record: Record::Sector {
                lba: 0,
                bytes: Box::new([0x44; 512]),
            },
        }
        .encode()
        .unwrap(),
    );
    fs::write(&f.journal, bytes).unwrap();
    let (code, output, _) = f.invoke("verify", &[]);
    assert_eq!(code, 3);
    assert_eq!(output, concat!(
        "base sha256: 5f70bf18a086007016e948b04aed3b82103a36bea41755b6cddfaf10ace3c6ef\n",
        "retained anchor: 0\n",
        "generation 0: immutable base, end 128, commands 0, sectors 0\n",
        "generation 1: explicit checkpoint, end 2048, commands 1, sectors 1\n",
        "generation 2: session end, end 4608, commands 1, sectors 2\n",
        "recoverable-tail bytes: 640\n",
        "recoverable tail: truncate at 4608, discard 640 bytes; failure at 5248: final command has no COMMAND_END\n",
        "tail classification assumes append-crash damage; discarded bytes are uncertain\n",
        "latest reconstructible state: generation 2 (closed), end 4608\n",
        "status: recovery required\n",
        "closed markers do not establish filesystem health or bootability\n",
    ));
}

#[cfg(unix)]
fn permissions_are_enforced(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let probe = dir.join("permission-probe");
    fs::write(&probe, []).unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o000)).unwrap();
    let enforced = fs::File::open(&probe).is_err();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(probe).unwrap();
    enforced
}

#[cfg(unix)]
#[test]
fn inspection_takes_a_shared_lock_on_a_read_only_lock_file() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    if !permissions_are_enforced(&f.dir) {
        eprintln!("skipped permission test: host bypasses permission bits (for example, root)");
        return;
    }
    f.two_generations();
    let lock = f.dir.join("disk.upfj.lock");
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o444)).unwrap();
    let first = JournalTool::open(&f.base, &f.journal, Access::Inspect).unwrap();
    assert!(first.has_lock());
    let (code, output, error) = f.invoke("verify", &[]);
    assert_eq!(code, 0, "{error}");
    assert!(!output.contains("no lock was taken"));
    assert!(JournalTool::open(&f.base, &f.journal, Access::Mutate).is_err());
}

#[cfg(unix)]
#[test]
fn inspection_reports_no_lock_when_read_only_directory_has_none() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    if !permissions_are_enforced(&f.dir) {
        eprintln!("skipped permission test: host bypasses permission bits (for example, root)");
        return;
    }
    f.two_generations();
    let lock = f.dir.join("disk.upfj.lock");
    fs::remove_file(&lock).unwrap();
    fs::set_permissions(&f.dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = f.invoke("verify", &[]);
    let mutation = f.rollback("0");
    let exists = lock.exists();
    fs::set_permissions(&f.dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(result.0, 0, "{}", result.2);
    assert!(result.1.starts_with(
        "no lock was taken: the lock file is absent and its directory is read-only\n"
    ));
    assert!(!exists);
    assert_eq!(mutation.0, 1);
    assert!(mutation.2.starts_with("lock "));
    assert!(!f.backup().exists());
}

#[cfg(unix)]
#[test]
fn private_journal_backups_keep_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    for repair in [false, true] {
        let f = Fixture::new();
        f.two_generations();
        fs::set_permissions(&f.journal, fs::Permissions::from_mode(0o600)).unwrap();
        let (code, _, error) = if repair { f.repair() } else { f.rollback("1") };
        assert_eq!(code, 0, "{error}");
        assert_eq!(
            fs::metadata(f.backup()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn reserved_journal_name_is_a_configuration_failure() {
    for marker in ["create", "retain"] {
        for alias in ["direct", "given", "resolved"] {
            let mut f = Fixture::new();
            f.two_generations();
            let reserved = f.dir.join(format!("disk.upfj.{marker}-0.tmp"));
            match alias {
                "direct" => {
                    fs::rename(&f.journal, &reserved).unwrap();
                    f.journal = reserved.clone();
                }
                "given" => {
                    std::os::unix::fs::symlink(&f.journal, &reserved).unwrap();
                    f.journal = reserved.clone();
                }
                _ => {
                    fs::rename(&f.journal, &reserved).unwrap();
                    std::os::unix::fs::symlink(&reserved, &f.journal).unwrap();
                }
            }
            let original = fs::read(&f.journal).unwrap();
            let (code, _, error) = f.invoke("verify", &[]);
            assert_eq!(code, 2, "{error}");
            assert!(
                error.contains("journal") && error.contains("reserved journal temporary name"),
                "{error}"
            );
            assert_eq!(fs::read(&f.journal).unwrap(), original);
            assert!(!f
                .dir
                .join(format!("disk.upfj.{marker}-0.tmp.lock"))
                .exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn unreadable_existing_lock_is_never_bypassed_and_names_the_lock() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    if !permissions_are_enforced(&f.dir) {
        eprintln!("skipped permission test: host bypasses permission bits (for example, root)");
        return;
    }
    f.two_generations();
    let lock = f.dir.canonicalize().unwrap().join("disk.upfj.lock");
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).unwrap();
    for operation in ["verify", "generations"] {
        let (code, output, error) = f.invoke(operation, &[]);
        assert_eq!(code, 1, "{error}");
        assert!(!output.contains("no lock was taken"));
        assert!(
            error.starts_with(&format!("lock {}:", lock.display())),
            "{error}"
        );
    }
    for (code, output, error) in [f.rollback("0"), f.repair()] {
        assert_eq!(code, 1, "{error}");
        assert!(!output.contains("no lock was taken"));
        assert!(
            error.starts_with(&format!("lock {}:", lock.display())),
            "{error}"
        );
    }
    assert!(!f.backup().exists());
}

#[cfg(unix)]
#[test]
fn dangling_lock_symlink_error_names_the_lock() {
    let f = Fixture::new();
    f.two_generations();
    let lock = f.dir.canonicalize().unwrap().join("disk.upfj.lock");
    fs::remove_file(&lock).unwrap();
    let missing = f.dir.join("missing-lock-target");
    std::os::unix::fs::symlink(&missing, &lock).unwrap();
    for operation in ["verify", "generations"] {
        let (code, output, error) = f.invoke(operation, &[]);
        assert_eq!(code, 1, "{error}");
        assert!(!output.contains("no lock was taken"));
        assert!(
            error.starts_with(&format!("lock {}:", lock.display())),
            "{error}"
        );
    }
    assert!(f.journal.exists());
    assert!(!missing.exists());
}

#[test]
fn commit_and_compact_export_each_closed_generation_without_changing_inputs() {
    let f = Fixture::new();
    f.two_generations();
    let original = fs::read(&f.journal).unwrap();
    for (operation, generation) in [("commit", 0), ("commit", 1), ("compact", 2)] {
        let output = f.dir.join(format!("export-{generation}.img"));
        let (code, out, error) = f.invoke(
            operation,
            &[
                "--to",
                &generation.to_string(),
                "--output",
                output.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 0, "{error}");
        assert!(out.contains(&format!("exported generation {generation}")));
        let mut expected = f.bytes.clone();
        if generation >= 1 {
            expected[..512].fill(if generation == 1 { 0x11 } else { 0x22 });
        }
        if generation == 2 {
            expected[512..].fill(0x33);
        }
        assert_eq!(fs::read(&output).unwrap(), expected);
        assert_eq!(fs::read(&f.base).unwrap(), f.bytes);
        assert_eq!(fs::read(&f.journal).unwrap(), original);
        assert_eq!(
            f.invoke(
                operation,
                &[
                    "--to",
                    &generation.to_string(),
                    "--output",
                    output.to_str().unwrap()
                ]
            )
            .0,
            2
        );
        assert_eq!(fs::read(&output).unwrap(), expected);
    }
}

#[test]
fn commit_refuses_input_aliases_open_tips_and_unavailable_targets() {
    let f = Fixture::new();
    f.two_generations();
    let mut destinations = vec![f.base.clone(), f.journal.clone()];
    let alias = f.dir.join("hardlink.img");
    fs::hard_link(&f.base, &alias).unwrap();
    destinations.push(alias);
    #[cfg(unix)]
    {
        let alias = f.dir.join("symlink.img");
        std::os::unix::fs::symlink(&f.journal, &alias).unwrap();
        destinations.push(alias);
    }
    let original = fs::read(&f.journal).unwrap();
    for output in destinations {
        assert_eq!(
            f.invoke(
                "commit",
                &["--to", "1", "--output", output.to_str().unwrap()]
            )
            .0,
            2
        );
    }
    let output = f.dir.join("export.img");
    assert_eq!(
        f.invoke(
            "commit",
            &["--to", "99", "--output", output.to_str().unwrap()]
        )
        .0,
        2
    );
    assert!(!output.exists());
    assert_eq!(fs::read(&f.journal).unwrap(), original);
    let mut disk = f.mount();
    disk.write_batch(&[(0, [0xA5; 512])]);
    drop(disk);
    let unsealed = fs::read(&f.journal).unwrap();
    let (code, _, error) = f.invoke(
        "commit",
        &["--to", "1", "--output", output.to_str().unwrap()],
    );
    assert_eq!(code, 2);
    assert!(error.contains("sealed"));
    assert!(!output.exists());
    assert_eq!(fs::read(&f.base).unwrap(), f.bytes);
    assert_eq!(fs::read(&f.journal).unwrap(), unsealed);
}

#[test]
fn retained_journal_verifies_and_exports_all_surviving_states() {
    let f = Fixture::new();
    let mut disk = f.mount().with_retention(2);
    for id in 1..=5 {
        disk.write_batch(&[(0, [id; 512])]);
        if id < 5 {
            disk.checkpoint().unwrap();
        }
    }
    disk.finish().unwrap();
    assert_eq!(f.invoke("verify", &[]).0, 0);
    let (code, listing, _) = f.invoke("generations", &[]);
    assert_eq!(code, 0);
    assert!(listing.contains("retained anchor: 4"));
    for id in 4..=5 {
        let output = f.dir.join(format!("retained-{id}.img"));
        assert_eq!(
            f.invoke(
                "commit",
                &[
                    "--to",
                    &id.to_string(),
                    "--output",
                    output.to_str().unwrap()
                ]
            )
            .0,
            0
        );
        assert_eq!(&fs::read(output).unwrap()[..512], &[id; 512]);
    }
    let output = f.dir.join("pruned.img");
    for id in [0, 1, 2, 3] {
        assert_eq!(
            f.invoke(
                "commit",
                &[
                    "--to",
                    &id.to_string(),
                    "--output",
                    output.to_str().unwrap()
                ]
            )
            .0,
            2
        );
    }
    assert!(!output.exists());
    assert_eq!(fs::read(&f.base).unwrap(), f.bytes);
}

#[test]
fn commit_parser_requires_exact_export_options() {
    let f = Fixture::new();
    for extra in [
        vec![],
        vec!["--to", "1"],
        vec!["--output", "out.img"],
        vec![
            "--to",
            "1",
            "--output",
            "out.img",
            "--backup",
            "backup.upfj",
        ],
        vec!["--to", "-1", "--output", "out.img"],
    ] {
        assert_eq!(f.invoke("commit", &extra).0, 2);
    }
    assert_eq!(f.invoke("verify", &["--output", "out.img"]).0, 2);
}

#[test]
fn commit_uses_shared_lock_and_refuses_live_writer_corruption_and_wrong_base() {
    let f = Fixture::new();
    f.two_generations();
    let output = f.dir.join("export.img");
    let args = ["--to", "1", "--output", output.to_str().unwrap()];
    let inspector = JournalTool::open(&f.base, &f.journal, Access::Inspect).unwrap();
    assert_eq!(f.invoke("commit", &args).0, 0);
    drop(inspector);
    fs::remove_file(&output).unwrap();
    let disk = f.mount();
    assert_eq!(f.invoke("commit", &args).0, 1);
    assert!(!output.exists());
    drop(disk);
    fs::write(&f.base, vec![0xFF; 1024]).unwrap();
    assert_eq!(f.invoke("commit", &args).0, 1);
    assert!(!output.exists());
    fs::write(&f.base, &f.bytes).unwrap();
    let mut corrupt = fs::read(&f.journal).unwrap();
    corrupt[HEADER_LEN + 64] ^= 1;
    fs::write(&f.journal, corrupt).unwrap();
    assert_eq!(f.invoke("commit", &args).0, 1);
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn offline_tools_refuse_reserved_base_names_before_repair_or_rollback() {
    for marker in ["create", "retain"] {
        for alias in ["direct", "given", "resolved"] {
            let mut f = Fixture::new();
            f.two_generations();
            f.append_torn();
            let reserved = f.dir.join(format!("disk.upfj.{marker}-1.tmp"));
            match alias {
                "direct" => {
                    fs::rename(&f.base, &reserved).unwrap();
                    f.base = reserved.clone();
                }
                "given" => {
                    std::os::unix::fs::symlink(&f.base, &reserved).unwrap();
                    f.base = reserved.clone();
                }
                _ => {
                    fs::rename(&f.base, &reserved).unwrap();
                    std::os::unix::fs::symlink(&reserved, &f.base).unwrap();
                }
            }
            let journal = fs::read(&f.journal).unwrap();
            let before: Vec<_> = fs::read_dir(&f.dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            let backup = f.backup();
            let output = f.dir.join("export.img");
            for (operation, extra) in [
                (
                    "verify",
                    vec!["--repair-tail", "--backup", backup.to_str().unwrap()],
                ),
                (
                    "rollback",
                    vec!["--to", "1", "--backup", backup.to_str().unwrap()],
                ),
                ("verify", vec![]),
                ("generations", vec![]),
                (
                    "commit",
                    vec!["--to", "1", "--output", output.to_str().unwrap()],
                ),
                (
                    "compact",
                    vec!["--to", "1", "--output", output.to_str().unwrap()],
                ),
            ] {
                let (code, _, error) = f.invoke(operation, &extra);
                assert_eq!(code, 2, "{marker} {alias} {operation}: {error}");
                assert!(
                    error.contains("base") && error.contains("reserved journal temporary name"),
                    "{error}"
                );
                assert_eq!(fs::read(&f.base).unwrap(), f.bytes);
                assert_eq!(fs::read(&f.journal).unwrap(), journal);
                assert!(!backup.exists() && !output.exists());
                let mut after: Vec<_> = fs::read_dir(&f.dir)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .collect();
                let mut before = before.clone();
                before.sort();
                after.sort();
                assert_eq!(after, before);
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn offline_outputs_and_lock_aliases_refuse_reserved_names_as_given_or_resolved() {
    for marker in ["create", "retain"] {
        for alias in ["direct", "given", "resolved"] {
            let f = Fixture::new();
            f.two_generations();
            let reserved = f.dir.join(format!("disk.upfj.{marker}-9.tmp"));
            let plain = f.dir.join("destination");
            let output = match alias {
                "direct" => reserved.clone(),
                "given" => {
                    fs::write(&plain, b"preserve").unwrap();
                    std::os::unix::fs::symlink(&plain, &reserved).unwrap();
                    reserved.clone()
                }
                _ => {
                    fs::write(&reserved, b"preserve").unwrap();
                    std::os::unix::fs::symlink(&reserved, &plain).unwrap();
                    plain.clone()
                }
            };
            let original = fs::read(&f.journal).unwrap();
            for (operation, extra) in [
                (
                    "commit",
                    vec!["--to", "1", "--output", output.to_str().unwrap()],
                ),
                (
                    "rollback",
                    vec!["--to", "1", "--backup", output.to_str().unwrap()],
                ),
                (
                    "verify",
                    vec!["--repair-tail", "--backup", output.to_str().unwrap()],
                ),
            ] {
                let (code, _, error) = f.invoke(operation, &extra);
                assert_eq!(code, 2, "{error}");
                assert!(error.contains("reserved journal temporary name"), "{error}");
                assert_eq!(fs::read(&f.journal).unwrap(), original);
                assert_eq!(fs::read(&f.base).unwrap(), f.bytes);
                if alias == "direct" {
                    assert!(!output.exists());
                } else {
                    assert_eq!(fs::read(&output).unwrap(), b"preserve");
                }
            }
        }
        let f = Fixture::new();
        f.two_generations();
        let lock = f.dir.join("disk.upfj.lock");
        let reserved = f.dir.join(format!("disk.upfj.{marker}-9.tmp"));
        fs::rename(&lock, &reserved).unwrap();
        std::os::unix::fs::symlink(&reserved, &lock).unwrap();
        let (code, _, error) = f.invoke("verify", &[]);
        assert_eq!(code, 2, "{error}");
        assert!(
            error.contains("lock") && error.contains("reserved journal temporary name"),
            "{error}"
        );
        assert!(reserved.exists());
    }
}
