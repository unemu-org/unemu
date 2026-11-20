// SPDX-License-Identifier: AGPL-3.0-or-later
//! The quick slot: one saved state per content and core
//! (core-contract.md §13.3, "Saved state"), behind Machine › Save state
//! (Ctrl+Alt+S) and Load state (Ctrl+Alt+L) for a machine whose description
//! lists saved state.
//!
//! The slot is the file `quick.state` in the content's states directory,
//! `states/CORE/aa/DIGEST/` ([`core_launch::Prepared::states_dir`]):
//!
//! ```text
//! unemu-state 1
//! IDENTITY
//! the core's bytes, as its save_state wrote them
//! ```
//!
//! The first line names this layout; the second is the machine's
//! `state_identity()`, the token a state loads only into a machine with an
//! equal one. Save state writes a file beside it under a temporary name and
//! renames it over the slot, so a save that fails part way leaves the state
//! saved before it whole. Load state compares the identity line with the
//! running machine's before the core sees a byte, and refuses another as the
//! player mock-up words it. Named slots and thumbnails are not part of this.

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
const FIRST_LINE: &[u8] = b"unemu-state 1";
/// The most the two lines may take together. Longer is not a slot's file,
/// and the player reads no further on the window's thread to find out.
const HEADER_LIMIT: u64 = 4096;

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

/// Read a slot's two lines from `file` and give its identity line, leaving
/// `file` at the core's bytes; `None` when the file does not have the slot's
/// layout: another first line, no end to the identity line within the
/// header's limit (a file cut short), or an identity that is not text.
fn read_header(file: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut header = Vec::new();
    let mut lines = file.take(HEADER_LIMIT);
    lines.read_until(b'\n', &mut header)?;
    if header.strip_suffix(b"\n") != Some(FIRST_LINE) {
        return Ok(None);
    }
    let mut identity = Vec::new();
    lines.read_until(b'\n', &mut identity)?;
    if identity.pop() != Some(b'\n') {
        return Ok(None);
    }
    Ok(String::from_utf8(identity).ok())
}

/// Write the slot through a temporary file in the same directory and a
/// rename, so the slot holds either the state saved before or the whole new
/// one. `body` writes the core's bytes after the two lines.
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
        out.write_all(FIRST_LINE)
            .and_then(|()| out.write_all(b"\n"))
            .and_then(|()| out.write_all(identity.as_bytes()))
            .and_then(|()| out.write_all(b"\n"))
            .map_err(|error| error.to_string())?;
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
            Some(identity) if !identity.contains(['\n', '\r']) => {
                write_atomically(states, identity, |out| {
                    session.save_state(out).map_err(|error| error.to_string())
                })
            }
            Some(_) => Err("the machine's state identity is not one line".into()),
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
    /// once its identity line is the running machine's. A refusal shows the
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
    let mut out = FIRST_LINE.to_vec();
    out.push(b'\n');
    out.extend_from_slice(identity.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(state);
    out
}
