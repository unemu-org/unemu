// SPDX-License-Identifier: AGPL-3.0-or-later
//! Saving the diagnostic report of the running PC on demand.
//!
//! With `UNEMUPC_DIAG=1` in the environment at startup, the chord
//! Ctrl+Alt+Shift+D (Command+Shift+D on macOS, where the desktop's
//! Ctrl+Alt chords become Command chords) writes the running PC's
//! diagnostic report (`assembly::diagnostic`) to
//! `<data>/unemupc/diagnostics/diag-N.txt`, beside the `saves` and `states`
//! of the platform data directory, and the status line says where. N is one
//! more than the highest number already in the folder, so reports of earlier
//! sessions stay. With the switch off the chord does nothing. Either way the
//! chord is the host's: its D never reaches the guest. The machine also
//! keeps its code trail only with the switch on. See
//! `backlog/docs/design/observation.md` §Diagnostic report of a running
//! machine and `backlog/docs/commands.md`.

use std::{
    ffi::OsStr,
    io::{self, Write},
    path::{Path, PathBuf},
};
use winit::keyboard::KeyCode;

/// Setting this to `1` arms the diagnostic chord; read once at startup.
pub(crate) const DIAG_VAR: &str = "UNEMUPC_DIAG";

/// The folder reports go to when the switch is exactly `1`: `None` with the
/// switch off; an error when the host has no data directory.
pub(crate) fn requested<E: std::fmt::Display>(
    value: Option<&OsStr>,
    data: Result<PathBuf, E>,
) -> Option<Result<PathBuf, String>> {
    (value == Some(OsStr::new("1"))).then(|| {
        data.map(|data| data.join("unemupc").join("diagnostics"))
            .map_err(|error| format!("the host has no data directory ({error})"))
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

/// Write `text` as the next `diag-N.txt` in `folder`, creating the folder;
/// the path written.
pub(crate) fn save(folder: &Path, text: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let mut next = std::fs::read_dir(folder)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_prefix("diag-")?
                .strip_suffix(".txt")?
                .parse::<u64>()
                .ok()
        })
        .max()
        .map_or(1, |highest| highest.saturating_add(1));
    loop {
        let path = folder.join(format!("diag-{next}.txt"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(text.as_bytes())?;
                return Ok(path);
            }
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
    fn the_switch_is_exactly_one_and_names_the_data_folder() {
        let data = || io::Result::Ok(PathBuf::from("/data"));
        assert_eq!(requested(None, data()), None);
        for value in ["", "0", "true", "yes", "01"] {
            assert_eq!(requested(Some(OsStr::new(value)), data()), None, "{value}");
        }
        assert_eq!(
            requested(Some(OsStr::new("1")), data()),
            Some(Ok(PathBuf::from("/data/unemupc/diagnostics")))
        );
        assert!(matches!(
            requested(
                Some(OsStr::new("1")),
                Err::<PathBuf, _>(io::Error::other("none"))
            ),
            Some(Err(_))
        ));
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
