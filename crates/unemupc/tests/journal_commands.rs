// SPDX-License-Identifier: AGPL-3.0-or-later
//! Standalone journal command contracts; core-contract.md §12.8 and
//! persistent-media.md § Offline commands. All bytes are synthetic.

use journal_files::{Header, HEADER_LEN};
use std::ffi::OsString;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "standalone-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("base.img"), [0; 1024]).unwrap();
        std::fs::write(
            path.join("disk.upfj"),
            Header::new(&[0; 1024], 0).unwrap().encode().unwrap(),
        )
        .unwrap();
        Self(path)
    }
    fn run(&self, action: &str, extra: &[&str]) -> (u8, String, String) {
        let mut args: Vec<OsString> = ["media", action, "--base"]
            .into_iter()
            .map(Into::into)
            .collect();
        args.push(self.0.join("base.img").into());
        args.push("--journal".into());
        args.push(self.0.join("disk.upfj").into());
        args.extend(extra.iter().map(Into::into));
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let code = unemupc::run(&args, &mut output, &mut errors);
        (
            code,
            String::from_utf8(output).unwrap(),
            String::from_utf8(errors).unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn standalone_usage_lists_all_four_journal_commands() {
    let usage = unemupc::usage();
    for action in ["generations", "verify", "rollback", "commit"] {
        assert!(
            usage.contains(&format!("unemupc media {action}")),
            "{usage}"
        );
    }
}

#[test]
fn standalone_inspection_and_mutations_keep_the_base_immutable() {
    let fixture = Fixture::new();
    for action in ["generations", "verify"] {
        let (code, output, errors) = fixture.run(action, &[]);
        assert_eq!(code, 0, "{errors}");
        assert!(!output.is_empty());
        assert!(errors.is_empty());
    }
    let backup = fixture.0.join("backup.upfj");
    let (code, output, errors) = fixture.run(
        "rollback",
        &["--to", "0", "--backup", backup.to_str().unwrap()],
    );
    assert_eq!(code, 0, "{errors}");
    assert!(output.starts_with("rollback to generation 0:"));
    assert_eq!(std::fs::read(backup).unwrap().len(), HEADER_LEN);
    for action in ["commit", "compact"] {
        let output = fixture.0.join(action);
        let (code, _, errors) =
            fixture.run(action, &["--to", "0", "--output", output.to_str().unwrap()]);
        assert_eq!(code, 0, "{errors}");
        assert_eq!(std::fs::read(output).unwrap(), [0; 1024]);
    }
    assert_eq!(
        std::fs::read(fixture.0.join("base.img")).unwrap(),
        [0; 1024]
    );
}

#[test]
fn standalone_journal_exit_codes_distinguish_usage_storage_and_recovery() {
    let fixture = Fixture::new();
    assert_eq!(fixture.run("rollback", &[]).0, 2);
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(fixture.0.join("disk.upfj"))
        .unwrap()
        .write_all(b"torn")
        .unwrap();
    assert_eq!(fixture.run("verify", &[]).0, 3);
    std::fs::remove_file(fixture.0.join("disk.upfj")).unwrap();
    assert_eq!(fixture.run("verify", &[]).0, 1);
}
