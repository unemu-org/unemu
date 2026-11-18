// SPDX-License-Identifier: AGPL-3.0-or-later
//! Non-writing machine checks; persistent-media § Session lifecycle and inspection.
//! No test in this binary spawns a process while another test can hold a lock.

use media::{Header, JournalMedia, SinkStatus, HEADER_LEN};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Files {
    root: PathBuf,
    base: Vec<u8>,
}
impl Files {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "machine-check-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let files = Self {
            root,
            base: vec![0; 368_640],
        };
        std::fs::write(files.root.join("base.img"), &files.base).unwrap();
        std::fs::write(files.root.join("bios.bin"), vec![0xF4; 32768]).unwrap();
        std::fs::write(files.machine(), "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='base.img'\nwrite='journal'\njournal='disk.upfj'\n").unwrap();
        files
    }
    fn machine(&self) -> PathBuf {
        self.root.join("machine.toml")
    }
    fn journal(&self) -> PathBuf {
        self.root.join("disk.upfj")
    }
    fn mount(&self) -> JournalMedia {
        JournalMedia::open(
            &self.journal(),
            &self.base,
            self.base.len() as u64,
            SinkStatus::default(),
        )
        .unwrap()
    }
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        std::fs::read_dir(&self.root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().into(),
                    std::fs::read(&path).unwrap(),
                )
            })
            .collect()
    }
    fn check_unchanged(&self, expected: u8) -> String {
        let before = self.snapshot();
        let args: Vec<OsString> = vec![
            "machine".into(),
            "check".into(),
            self.machine().into(),
            "--allow-unlisted-firmware".into(),
        ];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = cli_pc::command::run(&args, &mut out, &mut err);
        let err = String::from_utf8(err).unwrap();
        assert_eq!(
            self.snapshot(),
            before,
            "machine check changed directory contents: {err}"
        );
        assert_eq!(code, expected, "{err}");
        assert!(out.is_empty());
        err
    }
    fn assert_drive_origin(&self, text: &str) {
        assert!(
            text.contains(&format!(
                "{}:9: card[fdc-765].drives[unit=0]:",
                self.machine().display()
            )),
            "{text}"
        );
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn missing_journal_is_valid_without_creating_a_journal_or_lock() {
    let files = Files::new();
    let text = files.check_unchanged(0);
    assert!(text.contains("note: journal will be created"), "{text}");
    files.assert_drive_origin(&text);
}

#[test]
fn valid_journal_is_inspected_without_cleanup_or_new_sidecars() {
    let files = Files::new();
    let mut disk = files.mount();
    disk.write_batch(&[(0, [0x11; 512])]);
    disk.finish().unwrap();
    std::fs::write(files.root.join("disk.upfj.retain-123"), b"leftover").unwrap();
    files.check_unchanged(0);
    std::fs::remove_file(files.root.join("disk.upfj.lock")).unwrap();
    let text = files.check_unchanged(0);
    assert!(
        text.contains("journal lock is absent; inspected without a lock"),
        "{text}"
    );
}

#[test]
fn unsealed_commands_and_recoverable_tail_are_valid_without_recovery() {
    for tail in [false, true] {
        let files = Files::new();
        let mut disk = files.mount();
        disk.write_batch(&[(0, [0x22; 512])]);
        drop(disk);
        if tail {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(files.journal())
                .unwrap()
                .write_all(b"torn")
                .unwrap();
        }
        let text = files.check_unchanged(0);
        assert!(
            text.contains("warning: journal recovery will run on the next mount"),
            "{text}"
        );
        files.assert_drive_origin(&text);
    }
}

#[test]
fn interior_damage_and_wrong_base_are_invalid_at_the_drive_origin() {
    for wrong_base in [false, true] {
        let files = Files::new();
        let mut disk = files.mount();
        disk.write_batch(&[(0, [0x33; 512])]);
        disk.checkpoint().unwrap();
        disk.write_batch(&[(0, [0x44; 512])]);
        disk.finish().unwrap();
        if wrong_base {
            let mut base = files.base.clone();
            base[0] ^= 1;
            std::fs::write(files.root.join("base.img"), base).unwrap();
        } else {
            let mut journal = std::fs::read(files.journal()).unwrap();
            journal[HEADER_LEN + 64] ^= 1;
            std::fs::write(files.journal(), journal).unwrap();
        }
        let text = files.check_unchanged(1);
        assert!(text.contains("verification refused"), "{text}");
        files.assert_drive_origin(&text);
    }
}

#[test]
fn active_session_is_valid_but_not_inspected() {
    let files = Files::new();
    let mut disk = files.mount();
    disk.write_batch(&[(0, [0x55; 512])]);
    let text = files.check_unchanged(0);
    assert!(
        text.contains("warning: journal is in use and was not inspected"),
        "{text}"
    );
    files.assert_drive_origin(&text);
    drop(disk);
}

#[test]
fn catalogue_checks_still_run_after_read_only_journal_inspection() {
    let files = Files::new();
    std::fs::write(
        files.journal(),
        Header::new(&files.base, 0).unwrap().encode().unwrap(),
    )
    .unwrap();
    let before = files.snapshot();
    let args: Vec<OsString> = vec!["machine".into(), "check".into(), files.machine().into()];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(cli_pc::command::run(&args, &mut out, &mut err), 1);
    assert_eq!(files.snapshot(), before);
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.contains(&format!(
            "{}:5: firmware.bios.image:",
            files.machine().display()
        )),
        "{err}"
    );
}
