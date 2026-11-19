// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tools › Diagnostics: the machine report, the exception recording and
//! their folder.
//!
//! Save machine report (Ctrl+Alt+Shift+D; Command+Shift+D on macOS, where
//! the desktop's Ctrl+Alt chords become Command chords) writes the running
//! PC's diagnostic report (`assembly::diagnostic`) to
//! `<data>/unemupc/diagnostics/diag-N.txt`, beside the `saves` and `states`
//! of the platform data directory, and the status line says where. N is one
//! more than the highest number already in the folder, so reports of earlier
//! sessions stay. The chord is the host's whether or not the pointer is
//! captured: its D never reaches the guest. Whenever a machine runs, it keeps
//! its code trail for the report; its last exception deliveries are kept
//! from the first report, or from Record exceptions, since the CPU runs
//! measurably slower while it logs them.
//!
//! Record exceptions, off at every launch, records each session's
//! exceptions and interrupts (`machine::DeliveryRecording`): the last
//! 10,000 entries, consecutive identical deliveries folded into one, and
//! the totals per vector since the recording began. The recording goes to
//! one `trace-N.txt` per recording, created new and never written over a
//! file it did not write, written, and rewritten, at Save machine report, Stop, unchecking and closing the window; an app that is killed
//! writes nothing. Open diagnostics folder shows the folder in the
//! platform's file manager. See `backlog/docs/design/observation.md`
//! §Diagnostic report of a running machine and §Exception and interrupt
//! deliveries, and `backlog/docs/commands.md`.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};
use winit::keyboard::KeyCode;

/// The diagnostics folder in the platform data directory, or why there is
/// none.
pub(crate) fn folder<E: std::fmt::Display>(data: Result<PathBuf, E>) -> Result<PathBuf, String> {
    data.map(|data| data.join("unemupc").join("diagnostics"))
        .map_err(|error| format!("the host has no data directory ({error})"))
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

/// Write `text` as the next `diag-N.txt` in `folder`, creating the folder;
/// the path written.
pub(crate) fn save(folder: &Path, text: &str) -> io::Result<PathBuf> {
    let (path, mut file) = create_next(folder, "diag-")?;
    file.write_all(text.as_bytes())?;
    Ok(path)
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
/// already there, creating the folder; the path and the open file.
fn create_next(folder: &Path, prefix: &str) -> io::Result<(PathBuf, std::fs::File)> {
    std::fs::create_dir_all(folder)?;
    let mut next = std::fs::read_dir(folder)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_prefix(prefix)?
                .strip_suffix(".txt")?
                .parse::<u64>()
                .ok()
        })
        .max()
        .map_or(1, |highest| highest.saturating_add(1));
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

    #[test]
    fn the_folder_is_in_the_data_directory() {
        assert_eq!(
            folder(io::Result::Ok(PathBuf::from("/data"))),
            Ok(PathBuf::from("/data/unemupc/diagnostics"))
        );
        assert!(folder(Err::<PathBuf, _>(io::Error::other("none"))).is_err());
    }

    /// A recording takes the next free number the first time, created new,
    /// and keeps that file afterwards, rewritten with the latest text; a
    /// file it did not write last is left alone and the text takes the next
    /// number.
    #[test]
    fn a_recording_keeps_its_file_and_never_overwrites_another() {
        let temp = assembly::test_support::TempDir::new("trace-numbering");
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
    fn reports_take_the_next_free_number() {
        let temp = assembly::test_support::TempDir::new("diag-numbering");
        let folder = temp.path().join("diagnostics");
        assert_eq!(save(&folder, "a").unwrap(), folder.join("diag-1.txt"));
        assert_eq!(save(&folder, "b").unwrap(), folder.join("diag-2.txt"));
        std::fs::write(folder.join("diag-7.txt"), "kept").unwrap();
        std::fs::write(folder.join("diag-x.txt"), "ignored").unwrap();
        assert_eq!(save(&folder, "c").unwrap(), folder.join("diag-8.txt"));
        assert_eq!(
            std::fs::read_to_string(folder.join("diag-1.txt")).unwrap(),
            "a"
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("diag-7.txt")).unwrap(),
            "kept"
        );
    }
}
