// SPDX-License-Identifier: AGPL-3.0-or-later
//! Why a file did not become a machine to start, each case with the
//! sentence the application shows (core-contract.md §13.3, the refusals of
//! the player's approved mock-up).

use std::fmt;

/// Why a file did not become a machine to start: it could not be read, it
/// changed while it was read, no core opens it, a core recognised it and
/// cannot run it, the named core is not compiled in or drives another
/// contract, a core failed, it
/// needs content beyond itself, or more than one core runs it. Its
/// `Display` is the sentence the application shows; `FILE` in each is the
/// file's name without its directory.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LaunchError {
    /// The host could not read the file.
    Read {
        /// The file's name.
        file: String,
        /// What the host said.
        reason: String,
    },
    /// The file is no longer the bytes that were hashed.
    Changed {
        /// The file's name.
        file: String,
    },
    /// No core asked answered for the file, and none said it recognised it.
    NoCore {
        /// The file's name.
        file: String,
        /// What the program's cores open, one `NAME (*.ext;…)` per kind.
        kinds: Vec<String>,
    },
    /// The core the person named does not open the file.
    NotOpenedBy {
        /// The file's name.
        file: String,
        /// The core's name.
        core: String,
        /// What that core opens, one `NAME (*.ext;…)` per kind.
        kinds: Vec<String>,
    },
    /// A core recognised the file and cannot run it.
    Unsupported {
        /// The file's name.
        file: String,
        /// The core's name.
        core: String,
        /// The core's own message: what is missing.
        reason: String,
    },
    /// The person named a core this program does not have, or cannot drive.
    UnknownCore {
        /// The id the person named.
        id: String,
        /// The ids of the cores the program can drive, in the list's order.
        known: Vec<String>,
    },
    /// The person named a core that is compiled in and implements another
    /// contract.
    Unusable {
        /// The id the person named.
        id: String,
        /// "core NAME implements contract A.B; this program needs C.D".
        reason: String,
    },
    /// A core failed in a way the contract does not give a person a cause
    /// for: an error other than "not mine" or "cannot run it" from
    /// `identify`, a refusal of the config it wrote itself, or a config
    /// that does not name the file.
    Core {
        /// The file's name.
        file: String,
        /// The core's name.
        core: String,
        /// What went wrong, in the core's words where it gave some.
        reason: String,
    },
    /// The file needs more content than itself, which opening a file does
    /// not hand a machine.
    NeedsMore {
        /// The file's name.
        file: String,
        /// The labels of what is missing, as the core names them.
        missing: Vec<String>,
    },
    /// More than one core runs the file and nothing chose between them.
    Several {
        /// The file's name.
        file: String,
        /// The cores, each as its name and its id, in the order they were
        /// asked.
        cores: Vec<(String, String)>,
    },
}

/// `Name (*.a;*.b)`, joined for one sentence.
fn opens(kinds: &[String]) -> String {
    if kinds.is_empty() {
        "nothing".to_owned()
    } else {
        kinds.join("; ")
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { file, reason } => write!(formatter, "{file} could not be read: {reason}."),
            Self::Changed { file } => write!(
                formatter,
                "{file} changed while it was being opened. Open it again."
            ),
            Self::NoCore { file, kinds } => write!(
                formatter,
                "No core in this program opens {file}. This program opens: {}.",
                opens(kinds)
            ),
            Self::NotOpenedBy { file, core, kinds } => write!(
                formatter,
                "The core {core} does not open {file}. It opens: {}.",
                opens(kinds)
            ),
            Self::Unsupported { file, core, reason } => write!(
                formatter,
                "The core {core} recognised {file} and cannot run it: {reason}"
            ),
            Self::UnknownCore { id, known } => {
                write!(formatter, "This program has no core {id}.")?;
                if known.is_empty() {
                    write!(formatter, " It has no core that runs files.")
                } else {
                    write!(formatter, " Its cores: {}.", known.join(", "))
                }
            }
            Self::Unusable { reason, .. } => formatter.write_str(reason),
            Self::Core { file, core, reason } => {
                write!(formatter, "The core {core} failed on {file}: {reason}")
            }
            Self::NeedsMore { file, missing } => write!(
                formatter,
                "{file} needs more than itself to run. Missing: {}. Open… hands a machine the one file and nothing else.",
                missing.join(", ")
            ),
            Self::Several { file, cores } => {
                let cores: Vec<String> = cores
                    .iter()
                    .map(|(name, id)| format!("{name} ({id})"))
                    .collect();
                write!(
                    formatter,
                    "More than one core in this program runs {file}: {}. Name one on the command line: --open FILE --core ID.",
                    cores.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for LaunchError {}
