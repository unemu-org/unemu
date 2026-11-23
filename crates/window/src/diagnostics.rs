// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tools › Diagnostics: the machine report, guest RAM image, exception
//! recording and their folder.
//!
//! Save machine report (Ctrl+Alt+Shift+D; Command+Shift+D on macOS, where
//! the desktop's Ctrl+Alt chords become Command chords) writes the running
//! PC's diagnostic report (`assembly::diagnostic`) to
//! `<data>/unemu/diagnostics/diag-N.txt` and `diag-N.mem`, beside the `saves`
//! and `states` of the platform data directory. The memory file is flat:
//! each byte's offset is its guest physical address, gaps are zero-filled,
//! and its length is the end of the highest region. Regions and gaps are
//! streamed without another image-sized buffer. The complete report, with
//! the planned image file, size and regions, is written and flushed before
//! the image. An image write failure removes the partial image if possible
//! and appends the reason to the report; a read failure is included in the
//! report without writing an image. The status line reports both outcomes.
//! N is reserved for both files, one more than the highest number of either
//! extension already in the folder; existing files are never overwritten.
//! Images hold guest bytes for local diagnosis only: keep them in this
//! folder, never in a repository. The chord is the host's whether or not
//! the pointer is captured: its D never reaches the guest. Whenever a
//! machine runs, it keeps its code trail for the report; its last exception
//! deliveries are kept from the first report, or from Record exceptions,
//! since the CPU runs measurably slower while it logs them.
//!
//! Record exceptions, off at every launch, records each session's
//! exceptions and interrupts (`machine::DeliveryRecording`): the last
//! 10,000 entries, consecutive identical deliveries folded into one, and
//! the totals per vector since the recording began. The recording goes to
//! one `trace-N.txt` per recording, created new and never written over a
//! file it did not write, written, and rewritten, at Save machine report, Stop,
//! unchecking and closing the window; an app that is killed
//! writes nothing. Open diagnostics folder shows the folder in the
//! platform's file manager. See `backlog/docs/architecture.md` § Core boundary and
//! host timing, and `backlog/docs/commands.md` § Tools diagnostics.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};
use winit::keyboard::KeyCode;

/// The diagnostics folder in the platform data directory, or why there is
/// none.
pub(crate) fn folder<E: std::fmt::Display>(data: Result<PathBuf, E>) -> Result<PathBuf, String> {
    data.map_err(|error| format!("the host has no data directory ({error})"))
        .and_then(|data| {
            image_store::location::frontend::directory(&data, "diagnostics")
                .map_err(|error| error.to_string())
        })
}

/// Whether a press of `key` with `modifiers` (left and right Ctrl, Alt,
/// Shift and Command, as the shell holds them) is the diagnostic chord.
pub(crate) fn is_chord(key: KeyCode, modifiers: [bool; 8], macos: bool) -> bool {
    let ctrl = modifiers[..2].contains(&true);
    let alt = modifiers[2..4].contains(&true);
    let shift = modifiers[4..6].contains(&true);
    let command = modifiers[6..].contains(&true);
    key == KeyCode::KeyD
        && shift
        && if macos {
            command && !ctrl && !alt
        } else {
            ctrl && alt && !command
        }
}

/// The text report's path and whether its companion memory image was written.
pub(crate) struct SavedReport {
    pub path: PathBuf,
    pub memory: Result<PathBuf, String>,
}

/// Reserve one number for both files, then flush the report before the image.
/// An image failure is reported in the text, without losing the machine report.
pub(crate) fn save(
    folder: &Path,
    text: &str,
    image: Result<assembly::MemoryImage, String>,
) -> io::Result<SavedReport> {
    save_with_writer(folder, text, image, write_report, write_memory)
}

fn save_with_writer(
    folder: &Path,
    text: &str,
    image: Result<assembly::MemoryImage, String>,
    report: impl FnOnce(&mut std::fs::File, &[u8]) -> io::Result<()>,
    write: impl FnOnce(&mut std::fs::File, &assembly::MemoryImage) -> io::Result<u64>,
) -> io::Result<SavedReport> {
    let (path, mut file, memory_path, memory_file) = loop {
        let (path, file) = create_next(folder, "diag-")?;
        let memory_path = path.with_extension("mem");
        let memory_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&memory_path);
        if matches!(&memory_file, Err(error) if error.kind() == io::ErrorKind::AlreadyExists) {
            // A companion created after the directory scan owns this number.
            drop(file);
            std::fs::remove_file(path)?;
            continue;
        }
        break (path, file, memory_path, memory_file);
    };
    let memory_created = memory_file.is_ok();
    let mut text = text.to_owned();
    text.push_str(&format!(
        "desktop  memory image file {}\n",
        memory_path.display()
    ));
    if let Ok(image) = &image {
        for line in image.to_string().lines().skip(1) {
            text.push_str(&format!("desktop  memory {line}\n"));
        }
        let size = image
            .regions
            .last()
            .map_or(0, |region| region.base + region.bytes.len() as u64);
        text.push_str(&format!(
            "desktop  memory image size {size} bytes ({size:#x})\n"
        ));
    }
    text.push_str("desktop  memory image offsets are physical addresses; gaps are zero-filled\n");
    let image = image.map_err(|error| format!("cannot read guest RAM: {error}"));
    let image_read_failed = image.is_err();
    if let Err(error) = &image {
        text.push_str(&format!("desktop  memory image not written: {error}\n"));
    }
    // Preserve the primary artefact before the much larger image can fill the disk.
    if let Err(error) = report(&mut file, text.as_bytes()) {
        drop(memory_file);
        if memory_created {
            std::fs::remove_file(&memory_path)?;
        }
        return Err(error);
    }
    let memory = match image {
        Ok(image) => memory_file
            .map_err(|error| error.to_string())
            .and_then(|mut file| write(&mut file, &image).map_err(|error| error.to_string())),
        Err(error) => {
            drop(memory_file);
            Err(error)
        }
    };
    let memory = match memory {
        Ok(_) => Ok(memory_path),
        Err(mut error) => {
            // Reclaim partial image space so the failure line can still fit.
            if memory_created {
                if let Err(cleanup) = std::fs::remove_file(&memory_path) {
                    error.push_str(&format!(
                        "; incomplete memory image could not be removed: {cleanup}"
                    ));
                }
            }
            if !image_read_failed {
                let note = format!("desktop  memory image not written: {error}\n");
                if let Err(append) = file.write_all(note.as_bytes()).and_then(|()| file.flush()) {
                    error.push_str(&format!(
                        "; report could not record image failure: {append}"
                    ));
                }
            }
            Err(error)
        }
    };
    Ok(SavedReport { path, memory })
}

fn write_report(writer: &mut std::fs::File, text: &[u8]) -> io::Result<()> {
    writer.write_all(text)?;
    writer.flush()
}

fn write_memory(writer: &mut impl Write, image: &assembly::MemoryImage) -> io::Result<u64> {
    let zeros = [0; 8192];
    let mut offset = 0;
    for region in &image.regions {
        let mut gap = region.base - offset;
        while gap > 0 {
            let count = gap.min(zeros.len() as u64) as usize;
            writer.write_all(&zeros[..count])?;
            gap -= count as u64;
        }
        writer.write_all(&region.bytes)?;
        offset = region.base + region.bytes.len() as u64;
    }
    writer.flush()?;
    Ok(offset)
}

/// The file a recording went to and a digest of what it wrote there last.
#[derive(Debug, Default)]
pub(crate) struct RecordingFile {
    written: Option<(PathBuf, u64)>,
}

fn digest(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Write a recording's `text` to its file: the next `trace-N.txt` in
/// `folder` the first time, created new, never over another file; later the
/// same file again, while it still holds what this recording wrote last.
/// When it went missing or holds something else (another instance took the
/// number), the text goes to the next free number instead. The path
/// written.
pub(crate) fn save_recording(
    folder: &Path,
    file: &mut RecordingFile,
    text: &str,
) -> io::Result<PathBuf> {
    if let Some((path, last)) = &file.written {
        if std::fs::read(path).is_ok_and(|held| digest(&held) == *last) {
            let path = path.clone();
            std::fs::write(&path, text)?;
            file.written = Some((path.clone(), digest(text.as_bytes())));
            return Ok(path);
        }
    }
    let (created, mut handle) = create_next(folder, "trace-")?;
    handle.write_all(text.as_bytes())?;
    file.written = Some((created.clone(), digest(text.as_bytes())));
    Ok(created)
}

/// Show `folder` in the platform's file manager, creating it first.
pub(crate) fn reveal(folder: &Path) -> io::Result<()> {
    std::fs::create_dir_all(folder)?;
    #[cfg(test)]
    {
        REVEALED.with(|revealed| revealed.borrow_mut().push(folder.to_path_buf()));
        Ok(())
    }
    #[cfg(not(test))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        };
        let mut child = std::process::Command::new(opener)
            .arg(folder)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        // The opener may outlive the call; a thread reaps it, so it never
        // stays behind as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    /// The folders [`reveal`] would have shown, in a test.
    pub(crate) static REVEALED: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Create `<prefix>N.txt` in `folder`, N one more than the highest number
/// already there (including `.mem` companions for reports), creating the
/// folder; the path and the open file.
fn create_next(folder: &Path, prefix: &str) -> io::Result<(PathBuf, std::fs::File)> {
    std::fs::create_dir_all(folder)?;
    let highest = std::fs::read_dir(folder)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?.strip_prefix(prefix)?;
            let number = name.strip_suffix(".txt").or_else(|| {
                if prefix == "diag-" {
                    name.strip_suffix(".mem")
                } else {
                    None
                }
            })?;
            number.parse::<u64>().ok()
        })
        .max()
        .unwrap_or(0);
    let mut next = highest
        .checked_add(1)
        .ok_or_else(|| io::Error::other("no report number is left"))?;
    loop {
        let path = folder.join(format!("{prefix}{next}.txt"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            // Another program took the number meanwhile.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                next = next
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("no report number is left"))?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_image() -> Result<assembly::MemoryImage, String> {
        Ok(assembly::MemoryImage {
            regions: Vec::new(),
        })
    }

    fn synthetic_image() -> assembly::MemoryImage {
        assembly::MemoryImage {
            regions: vec![
                assembly::MemoryRegion {
                    base: 2,
                    kind: assembly::MemoryKind::Ram,
                    bytes: vec![1, 2],
                },
                assembly::MemoryRegion {
                    base: 0x4007,
                    kind: assembly::MemoryKind::Rom,
                    bytes: vec![3, 4, 5],
                },
            ],
        }
    }

    #[test]
    fn the_folder_is_in_the_data_directory() {
        assert_eq!(
            folder(io::Result::Ok(PathBuf::from("/data"))),
            Ok(PathBuf::from("/data/unemu/diagnostics"))
        );
        assert!(folder(Err::<PathBuf, _>(io::Error::other("none"))).is_err());
    }

    #[test]
    fn old_diagnostics_and_logs_stay_in_use_for_the_next_report() {
        let temp = machine_file::test_support::TempDir::new("diagnostics-upgrade");
        let old = temp.path().join("unemupc/diagnostics");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("diag-7.txt"), "report").unwrap();
        std::fs::write(old.join("trace-3.txt"), "trace").unwrap();
        let new = folder(io::Result::Ok(temp.path().to_path_buf())).unwrap();
        assert_eq!(new, old);
        assert_eq!(
            save(&new, "new", empty_image()).unwrap().path,
            new.join("diag-8.txt")
        );
        assert_eq!(std::fs::read(new.join("trace-3.txt")).unwrap(), b"trace");
        assert_eq!(std::fs::read(old.join("diag-7.txt")).unwrap(), b"report");
        assert_eq!(std::fs::read(old.join("trace-3.txt")).unwrap(), b"trace");
        assert!(old.join("diag-8.txt").exists());
        assert!(!temp.path().join("unemu/diagnostics").exists());
    }

    /// A recording takes the next free number the first time, created new,
    /// and keeps that file afterwards, rewritten with the latest text; a
    /// file it did not write last is left alone and the text takes the next
    /// number.
    #[test]
    fn a_recording_keeps_its_file_and_never_overwrites_another() {
        let temp = machine_file::test_support::TempDir::new("trace-numbering");
        let folder = temp.path().join("diagnostics");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("trace-3.txt"), "earlier").unwrap();
        let mut file = RecordingFile::default();
        let first = save_recording(&folder, &mut file, "one").unwrap();
        assert_eq!(first, folder.join("trace-4.txt"));
        assert_eq!(save_recording(&folder, &mut file, "two").unwrap(), first);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "two");
        // Another program replaced it: it stays, the text moves on.
        std::fs::write(&first, "theirs").unwrap();
        assert_eq!(
            save_recording(&folder, &mut file, "three").unwrap(),
            folder.join("trace-5.txt")
        );
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "theirs");
        assert_eq!(
            std::fs::read_to_string(folder.join("trace-3.txt")).unwrap(),
            "earlier"
        );
        let mut other = RecordingFile::default();
        assert_eq!(
            save_recording(&folder, &mut other, "four").unwrap(),
            folder.join("trace-6.txt")
        );
    }

    #[test]
    fn the_chord_is_ctrl_alt_shift_d_or_command_shift_d_on_macos() {
        let held =
            |ctrl, alt, shift, command| [ctrl, false, alt, false, false, shift, command, false];
        assert!(is_chord(
            KeyCode::KeyD,
            held(true, true, true, false),
            false
        ));
        assert!(!is_chord(
            KeyCode::KeyD,
            held(true, true, false, false),
            false
        ));
        assert!(!is_chord(
            KeyCode::KeyE,
            held(true, true, true, false),
            false
        ));
        assert!(!is_chord(
            KeyCode::KeyD,
            held(false, false, true, true),
            false
        ));
        assert!(is_chord(
            KeyCode::KeyD,
            held(false, false, true, true),
            true
        ));
        // Command+D alone is Duplicate in the library.
        assert!(!is_chord(
            KeyCode::KeyD,
            held(false, false, false, true),
            true
        ));
        assert!(!is_chord(
            KeyCode::KeyD,
            held(true, true, true, false),
            true
        ));
    }

    #[test]
    fn memory_images_preserve_physical_offsets_and_report_regions() {
        let temp = machine_file::test_support::TempDir::new("diag-memory-layout");
        let saved = save(temp.path(), "report\n", Ok(synthetic_image())).unwrap();
        assert_eq!(saved.path, temp.path().join("diag-1.txt"));
        let bytes = std::fs::read(saved.memory.unwrap()).unwrap();
        assert_eq!(bytes.len(), 0x400a);
        assert_eq!(&bytes[..2], &[0, 0]);
        assert_eq!(&bytes[2..4], &[1, 2]);
        assert!(bytes[4..0x4007].iter().all(|&byte| byte == 0));
        assert_eq!(&bytes[0x4007..], &[3, 4, 5]);
        let report = std::fs::read_to_string(saved.path).unwrap();
        for expected in [
            "diag-1.mem",
            "desktop  memory ram      base 0x00000002, length 0x2\n",
            "desktop  memory rom      base 0x00004007, length 0x3\n",
            "gaps are zero-filled",
            "desktop  memory image size 16394 bytes (0x400a)\n",
        ] {
            assert!(report.contains(expected), "{report}");
        }
    }

    #[test]
    fn orphan_reports_and_memory_images_keep_their_number_and_bytes() {
        let temp = machine_file::test_support::TempDir::new("diag-orphans");
        for (number, extension) in [(7, "mem"), (12, "txt")] {
            let orphan = temp.path().join(format!("diag-{number}.{extension}"));
            std::fs::write(&orphan, "kept").unwrap();
            let saved = save(temp.path(), "report\n", empty_image()).unwrap();
            assert_eq!(
                saved.path,
                temp.path().join(format!("diag-{}.txt", number + 1))
            );
            assert_eq!(
                saved.memory.unwrap(),
                temp.path().join(format!("diag-{}.mem", number + 1))
            );
            assert_eq!(std::fs::read(orphan).unwrap(), b"kept");
        }
    }

    #[test]
    fn an_image_read_failure_still_writes_the_report_and_reason() {
        let temp = machine_file::test_support::TempDir::new("diag-memory-read-error");
        let saved = save(
            temp.path(),
            "report\n",
            Err("synthetic read failure".into()),
        )
        .unwrap();
        assert_eq!(
            saved.memory.unwrap_err(),
            "cannot read guest RAM: synthetic read failure"
        );
        let report = std::fs::read_to_string(saved.path).unwrap();
        assert!(report.starts_with("report\n"));
        assert!(report.contains("desktop  memory image not written: cannot read guest RAM: synthetic read failure\n"), "{report}");
        assert!(!temp.path().join("diag-1.mem").exists());
    }

    #[test]
    fn an_image_write_failure_still_writes_the_report_and_reason() {
        struct FailingWriter<'a> {
            file: &'a mut std::fs::File,
            remaining: usize,
        }
        impl Write for FailingWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.remaining == 0 {
                    return Err(io::Error::other("synthetic write failure"));
                }
                let count = self.file.write(&bytes[..bytes.len().min(self.remaining)])?;
                self.remaining -= count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.file.flush()
            }
        }
        let temp = machine_file::test_support::TempDir::new("diag-memory-write-error");
        let saved = save_with_writer(
            temp.path(),
            "report\n",
            Ok(synthetic_image()),
            write_report,
            |file, image| write_memory(&mut FailingWriter { file, remaining: 5 }, image),
        )
        .unwrap();
        assert_eq!(saved.memory.unwrap_err(), "synthetic write failure");
        let report = std::fs::read_to_string(saved.path).unwrap();
        assert!(report.starts_with("report\n"));
        assert!(
            report.contains("desktop  memory image not written: synthetic write failure\n"),
            "{report}"
        );
        assert!(
            report.contains("desktop  memory rom      base 0x00004007, length 0x3\n"),
            "{report}"
        );
        assert!(
            report.contains("desktop  memory image size 16394 bytes (0x400a)\n"),
            "{report}"
        );
        assert!(!temp.path().join("diag-1.mem").exists());
    }

    #[test]
    fn report_write_and_flush_failures_remove_only_the_reserved_memory_image() {
        for fail_on_flush in [false, true] {
            let temp = machine_file::test_support::TempDir::new("diag-report-write-error");
            let existing = temp.path().join("diag-1.mem");
            let reserved = temp.path().join("diag-2.mem");
            std::fs::write(&existing, b"kept").unwrap();
            let result = save_with_writer(
                temp.path(),
                "report\n",
                Ok(synthetic_image()),
                |file, text| {
                    assert!(reserved.exists());
                    if fail_on_flush {
                        file.write_all(text)?;
                    }
                    Err(io::Error::other(if fail_on_flush {
                        "synthetic report flush failure"
                    } else {
                        "synthetic report write failure"
                    }))
                },
                |_, _| panic!("an image must not be written after a report failure"),
            );
            let error = result.err().expect("report failure must be returned");
            assert_eq!(
                error.to_string(),
                if fail_on_flush {
                    "synthetic report flush failure"
                } else {
                    "synthetic report write failure"
                }
            );
            assert!(
                !reserved.exists(),
                "empty memory reservation was left behind"
            );
            assert_eq!(std::fs::read(&existing).unwrap(), b"kept");
        }
    }

    #[test]
    fn the_complete_report_precedes_an_image_write_failure() {
        let temp = machine_file::test_support::TempDir::new("diag-report-first");
        let report_path = temp.path().join("diag-1.txt");
        let memory_path = temp.path().join("diag-1.mem");
        let complete_report = format!(
            "machine report\ndesktop  paused\n\
             desktop  memory image file {}\n\
             desktop  memory ram      base 0x00000002, length 0x2\n\
             desktop  memory rom      base 0x00004007, length 0x3\n\
             desktop  memory image size 16394 bytes (0x400a)\n\
             desktop  memory image offsets are physical addresses; gaps are zero-filled\n",
            memory_path.display()
        );
        let saved = save_with_writer(
            temp.path(),
            "machine report\ndesktop  paused\n",
            Ok(synthetic_image()),
            write_report,
            |file, _image| {
                assert_eq!(
                    std::fs::read_to_string(&report_path).unwrap(),
                    complete_report
                );
                assert_eq!(file.metadata()?.len(), 0);
                file.write_all(b"partial image")?;
                Err(io::Error::other("synthetic disk full"))
            },
        )
        .unwrap();
        assert_eq!(saved.path, report_path);
        assert_eq!(saved.memory.unwrap_err(), "synthetic disk full");
        assert_eq!(
            std::fs::read_to_string(report_path).unwrap(),
            format!("{complete_report}desktop  memory image not written: synthetic disk full\n")
        );
        assert!(!memory_path.exists());
    }

    #[test]
    fn reports_take_the_next_free_number() {
        let temp = machine_file::test_support::TempDir::new("diag-numbering");
        let folder = temp.path().join("diagnostics");
        assert_eq!(
            save(&folder, "a", empty_image()).unwrap().path,
            folder.join("diag-1.txt")
        );
        assert_eq!(
            save(&folder, "b", empty_image()).unwrap().path,
            folder.join("diag-2.txt")
        );
        std::fs::write(folder.join("diag-7.txt"), "kept").unwrap();
        std::fs::write(folder.join("diag-x.txt"), "ignored").unwrap();
        assert_eq!(
            save(&folder, "c", empty_image()).unwrap().path,
            folder.join("diag-8.txt")
        );
        assert!(std::fs::read_to_string(folder.join("diag-1.txt"))
            .unwrap()
            .starts_with("a"));
        assert_eq!(
            std::fs::read_to_string(folder.join("diag-7.txt")).unwrap(),
            "kept"
        );
    }
}
