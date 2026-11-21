// SPDX-License-Identifier: AGPL-3.0-or-later
//! Process death leaves an incomplete frame for normal recovery; see
//! persistent-media.md § Open and tail recovery. Kept separate from lock tests
//! so a child cannot inherit another test's live journal descriptors.
use journal_files::{Header, JournalFile, JournalMedia, SinkStatus, HEADER_LEN};
use std::io::{BufRead, Write};
use unemu_core_api::Record;

#[test]
fn recovery_discards_bytes_written_before_the_writer_is_killed() {
    let base = [0; 1024];
    if let Some(path) = std::env::var_os("UNEMUPC_JOURNAL_CRASH_PATH") {
        let mut record = JournalFile::open(
            std::path::Path::new(&path),
            &Header::new(&base, 0).unwrap(),
            None,
        )
        .unwrap();
        record.append(b"broken!").unwrap();
        println!("writer ready");
        std::io::stdout().flush().unwrap();
        let mut byte = [0];
        std::io::Read::read_exact(&mut std::io::stdin(), &mut byte).unwrap();
        panic!("the writer must be killed while its record is open");
    }
    let dir = std::env::temp_dir().join(format!("journal-crash-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("disk.upfj");
    drop(JournalMedia::open(&path, &base, 1024, SinkStatus::default()).unwrap());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "recovery_discards_bytes_written_before_the_writer_is_killed",
            "--nocapture",
        ])
        .env("UNEMUPC_JOURNAL_CRASH_PATH", &path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "writer exited before append"
        );
        if line.trim() == "writer ready" {
            break;
        }
    }
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        HEADER_LEN as u64 + 7
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let media = JournalMedia::open(&path, &base, 1024, SinkStatus::default()).unwrap();
    assert_eq!(media.mount_report().discarded_bytes, 7);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), HEADER_LEN as u64);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        Header::new(&base, 0).unwrap().encode().unwrap()
    );
    drop(media);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn first_mount_with_write_masked_by_umask_keeps_the_creation_descriptor() {
    use std::os::unix::fs::PermissionsExt;
    if let Some(path) = std::env::var_os("UNEMUPC_JOURNAL_UMASK_PATH") {
        let path = std::path::Path::new(&path);
        let result = JournalMedia::open(path, &[0; 1024], 1024, SinkStatus::default());
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o444
        );
        let mut media = result.unwrap_or_else(|error| panic!("first mount: {error}"));
        media.write_batch(&[(0, [7; 512])]);
        media.finish().unwrap();
        return;
    }
    let dir = std::env::temp_dir().join(format!("journal-umask-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let output = std::process::Command::new("sh")
        .args(["-c", "umask 0222; exec \"$@\"", "journal-umask"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "first_mount_with_write_masked_by_umask_keeps_the_creation_descriptor",
            "--nocapture",
        ])
        .env("UNEMUPC_JOURNAL_UMASK_PATH", dir.join("disk.upfj"))
        .output()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
