// SPDX-License-Identifier: AGPL-3.0-or-later
//! FIFO refusal from persistent-media.md § Offline commands in unemupc. Keep this
//! binary separate: a spawned child can inherit locks held by sibling test threads.

#![cfg(unix)]

use journal_files::Header;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

/// The PC journal commands through the frontend's platform seam.
fn run(args: &[OsString], output: &mut Vec<u8>, errors: &mut Vec<u8>) -> u8 {
    cli_pc::command::run(args, output, errors)
}

struct Fixture {
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("unemupc-media-fifo-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let base = vec![0; 1024];
        fs::write(dir.join("base.img"), &base).unwrap();
        fs::write(
            dir.join("disk.upfj"),
            Header::new(&base, 0).unwrap().encode().unwrap(),
        )
        .unwrap();
        Self { dir }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn fifo_inputs_are_refused_without_blocking() {
    use std::process::Command as Process;
    if let Some(dir) = std::env::var_os("MEDIA_TOOLS_FIFO_DIRECTORY") {
        let dir = PathBuf::from(dir);
        for role in ["base", "journal"] {
            let base = dir.join(if role == "base" { "pipe" } else { "base.img" });
            let journal = dir.join(if role == "journal" {
                "pipe"
            } else {
                "disk.upfj"
            });
            for operation in ["verify", "rollback"] {
                let mut args = vec![
                    "media".into(),
                    operation.into(),
                    "--base".into(),
                    base.clone().into(),
                    "--journal".into(),
                    journal.clone().into(),
                ];
                if operation == "rollback" {
                    args.extend([
                        "--to".into(),
                        "0".into(),
                        "--backup".into(),
                        dir.join("before").into(),
                    ]);
                }
                let (mut out, mut err) = (Vec::new(), Vec::new());
                assert_eq!(run(&args, &mut out, &mut err), 1);
                let error = String::from_utf8(err).unwrap();
                assert!(error.contains(role), "{error}");
                assert!(
                    error.contains(dir.join("pipe").to_str().unwrap()),
                    "{error}"
                );
                assert!(error.contains("regular file"), "{error}");
            }
        }
        return;
    }
    let f = Fixture::new();
    assert!(Process::new("mkfifo")
        .arg(f.dir.join("pipe"))
        .status()
        .unwrap()
        .success());
    let mut child = Process::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fifo_inputs_are_refused_without_blocking",
            "--nocapture",
        ])
        .env("MEDIA_TOOLS_FIFO_DIRECTORY", &f.dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            assert!(
                status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("opening an input blocked on a FIFO");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
