// SPDX-License-Identifier: AGPL-3.0-or-later
//! The quick slot: one saved state per content and core
//! (core-contract.md §13.3, "Saved state"), behind Machine › Save state
//! (Ctrl+Alt+S) and Load state (Ctrl+Alt+L) for a machine whose description
//! lists saved state.
//!
//! The slot is the file `quick.state` in the content's states directory,
//! `states/CORE/aa/DIGEST/` ([`core_launch::Prepared::states_dir`]):
//!
//! New saves use `unemu-state 2\n`, an eight-byte little-endian length,
//! the identity's UTF-8 bytes, then the core's state bytes. The original
//! `unemu-state 1` format with a newline-delimited identity remains readable.
//!
//! `state_identity()` is an opaque token: a state loads only into a machine
//! with an equal one, including every byte of whitespace. Save state writes
//! a file beside it under a temporary name and renames it over the slot, so
//! a save that fails part way leaves the state saved before it whole. Load
//! compares identities before the core sees a byte, and refuses another as
//! the player mock-up words it. Named slots and thumbnails are not part of this.

use crate::{wake, Dialog, MessageBox, Player};
use std::{
    fs,
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use unemu_core_api::ErrorKind;

/// The slot's file name in the content's states directory.
pub(crate) const FILE_NAME: &str = "quick.state";
/// What a save writes before renaming it over the slot. A file of this name
/// left by a save cut short is overwritten by the next save.
const PARTIAL_NAME: &str = "quick.state.partial";
/// The layout's first line, without its newline.
const FIRST_LINE: &[u8] = b"unemu-state 2";
const LEGACY_FIRST_LINE: &[u8] = b"unemu-state 1";
/// The original format bounded its two newline-delimited header lines.
const HEADER_LIMIT: u64 = 4096;
/// The contract sets no size limit. 64 KiB leaves ample room for an opaque
/// release/config/media compatibility token, which names state dependencies
/// rather than containing the state, while bounding untrusted header reads.
const IDENTITY_LIMIT: u64 = 64 * 1024;

/// The refusal's lead and the line the player mock-up closes it with: a
/// refused load leaves the machine as it was.
const NOT_LOADED: &str = "The state was not loaded";
const RUNS_ON: &str = "The machine runs on as it was.";
/// The mock-up's reason for a state of another identity.
const OTHER_IDENTITY: &str =
    "this state was saved by another release of the core, or for another setup";
/// The reason for a file that does not have the slot's layout.
const DAMAGED: &str = "the file is damaged, or it is not a saved state this program reads";

/// The slot's file for the content in `states`.
pub(crate) fn path(states: &Path) -> PathBuf {
    states.join(FILE_NAME)
}

/// Read either header format, leaving `file` at the core's state bytes.
/// A truncated header or an unknown format is not a saved-state slot.
fn read_header(file: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut header = Vec::new();
    file.take((FIRST_LINE.len() + 1) as u64)
        .read_until(b'\n', &mut header)?;
    let mut identity = Vec::new();
    match header.strip_suffix(b"\n") {
        Some(LEGACY_FIRST_LINE) => {
            file.take(HEADER_LIMIT - header.len() as u64)
                .read_until(b'\n', &mut identity)?;
            if identity.pop() != Some(b'\n') {
                return Ok(None);
            }
        }
        Some(FIRST_LINE) => {
            let mut length = [0; 8];
            if let Err(error) = file.read_exact(&mut length) {
                return if error.kind() == io::ErrorKind::UnexpectedEof {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            let length = u64::from_le_bytes(length);
            if length > IDENTITY_LIMIT {
                return Ok(None);
            }
            file.take(length).read_to_end(&mut identity)?;
            if identity.len() as u64 != length {
                return Ok(None);
            }
        }
        _ => return Ok(None),
    }
    Ok(String::from_utf8(identity).ok())
}

fn write_header(out: &mut impl Write, identity: &str) -> io::Result<()> {
    out.write_all(FIRST_LINE)?;
    out.write_all(b"\n")?;
    out.write_all(&(identity.len() as u64).to_le_bytes())?;
    out.write_all(identity.as_bytes())
}

/// Write the slot through a temporary file in the same directory and a
/// rename, so the slot holds either the state saved before or the whole new
/// one. `body` writes the core's bytes after the identity.
fn write_atomically(
    dir: &Path,
    identity: &str,
    body: impl FnOnce(&mut dyn Write) -> Result<(), String>,
) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|error| format!("{} cannot be made: {error}", dir.display()))?;
    let partial = dir.join(PARTIAL_NAME);
    let written = (|| {
        let file = fs::File::create(&partial).map_err(|error| error.to_string())?;
        let mut out = BufWriter::new(file);
        write_header(&mut out, identity).map_err(|error| error.to_string())?;
        body(&mut out)?;
        let file = out.into_inner().map_err(|error| error.to_string())?;
        // Durable before the rename, so the slot never names a file whose
        // bytes a power cut could still lose.
        file.sync_all().map_err(|error| error.to_string())?;
        // Closed before the rename, which some hosts refuse for an open file.
        drop(file);
        fs::rename(&partial, path(dir)).map_err(|error| error.to_string())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&partial);
    }
    written
}

impl Player {
    /// Machine › Save state: the machine's whole state into the quick slot,
    /// replacing the one saved before only once the new one is written.
    pub(crate) fn save_state(&mut self) {
        if !self.offers.saved_state {
            return;
        }
        let Some(states) = self.prepared.states_dir() else {
            return;
        };
        let Some(session) = self.session.as_mut().filter(|s| s.takes_media()) else {
            return;
        };
        let identity = session.state_identity();
        let saved = match identity.as_deref() {
            Some(identity) => write_atomically(states, identity, |out| {
                session.save_state(out).map_err(|error| error.to_string())
            }),
            None => Err("the machine names no state identity".into()),
        };
        match saved {
            Ok(()) => {
                self.state_saved = true;
                self.message = "State saved".into();
            }
            Err(error) => {
                self.message = "State not saved".into();
                let kept = if self.state_saved {
                    "\n\nThe state saved before is kept."
                } else {
                    ""
                };
                self.dialog = Some(Dialog::Message(MessageBox::refusal(
                    "Save state",
                    format!("The state was not saved: {}.{kept}", trim(&error)),
                )));
            }
        }
    }

    /// Machine › Load state: the quick slot's state, at host time `elapsed`,
    /// once its identity equals the running machine's. A refusal shows the
    /// Load state box and leaves the machine as it was.
    pub(crate) fn load_state(&mut self, elapsed: Duration) {
        if !self.offers.saved_state || !self.state_saved {
            return;
        }
        let Some(states) = self.prepared.states_dir() else {
            return;
        };
        let Some(session) = self.session.as_mut().filter(|s| s.takes_media()) else {
            return;
        };
        let unreadable = |error: io::Error| format!("the file could not be read: {error}");
        let refusal = match fs::File::open(path(states)) {
            Err(error) => Some(unreadable(error)),
            Ok(file) => {
                let mut file = BufReader::new(file);
                match read_header(&mut file) {
                    Err(error) => Some(unreadable(error)),
                    Ok(None) => Some(DAMAGED.to_owned()),
                    Ok(Some(identity))
                        if session.state_identity().as_deref() != Some(identity.as_str()) =>
                    {
                        Some(OTHER_IDENTITY.to_owned())
                    }
                    Ok(Some(_)) => match session.load_state(&mut file as &mut dyn Read, elapsed) {
                        Ok(()) => None,
                        Err(error) if error.kind == ErrorKind::State => {
                            Some(trim(&error.to_string()).to_owned())
                        }
                        // Any other failure may come after the core took the
                        // state, while the session took its picture: the
                        // machine is not known to be as it was, so its run
                        // ends as for any host failure and Stop is left.
                        Err(error) => {
                            self.host_failure(format!(
                                "{NOT_LOADED}: {}.",
                                trim(&error.to_string())
                            ));
                            return;
                        }
                    },
                }
            }
        };
        match refusal {
            None => {
                if let Some(session) = &mut self.session {
                    wake(session, elapsed);
                }
                // A state that leaves the machine running is painted and
                // advanced again at once, also after a halt, which had no
                // next frame to wait for. Only a running machine is paused:
                // a state saved while the machine waited halted leaves it
                // halted again, not paused.
                if !self.running() {
                    self.paused = false;
                }
                self.message = "State loaded".into();
            }
            Some(reason) => {
                self.message = "State not loaded".into();
                self.dialog = Some(Dialog::Message(MessageBox::refusal(
                    "Load state",
                    format!("{NOT_LOADED}: {reason}.\n\n{RUNS_ON}"),
                )));
            }
        }
    }
}

/// A reason without the full stop the sentence around it adds.
fn trim(reason: &str) -> &str {
    reason.trim_end().trim_end_matches('.')
}

/// Whether the slot's file exists for the content in `states`.
pub(crate) fn exists(states: &Path) -> bool {
    path(states).is_file()
}

#[cfg(test)]
pub(crate) fn partial_path(states: &Path) -> PathBuf {
    states.join(PARTIAL_NAME)
}

#[cfg(test)]
pub(crate) fn encode(identity: &str, state: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    write_header(&mut out, identity).unwrap();
    out.extend_from_slice(state);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_identity_is_refused_before_reading_its_bytes() {
        struct HeaderOnly(io::Cursor<Vec<u8>>);

        impl Read for HeaderOnly {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                assert!(
                    self.0.position() < (FIRST_LINE.len() + 1 + 8) as u64,
                    "oversized identity payload must not be read"
                );
                self.0.read(out)
            }
        }

        impl BufRead for HeaderOnly {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                self.0.fill_buf()
            }

            fn consume(&mut self, amount: usize) {
                self.0.consume(amount);
            }
        }

        for length in [1u64 << 32, u64::MAX, 64 * 1024 + 1] {
            let bytes = [FIRST_LINE, b"\n", &length.to_le_bytes(), b"tiny"].concat();
            let mut file = HeaderOnly(io::Cursor::new(bytes));
            assert_eq!(read_header(&mut file).unwrap(), None);
        }
    }

    #[test]
    fn bounded_identity_leaves_the_core_state_unread() {
        for length in [0, 64 * 1024] {
            let identity = "x".repeat(length);
            let mut file = io::Cursor::new(encode(&identity, b"core state"));
            assert_eq!(read_header(&mut file).unwrap(), Some(identity));
            let mut state = Vec::new();
            file.read_to_end(&mut state).unwrap();
            assert_eq!(state, b"core state");
        }
    }

    #[test]
    fn short_identity_is_damaged() {
        let bytes = [FIRST_LINE, b"\n", &8u64.to_le_bytes(), b"tiny"].concat();
        assert_eq!(read_header(&mut io::Cursor::new(bytes)).unwrap(), None);
    }
}
