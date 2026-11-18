// SPDX-License-Identifier: AGPL-3.0-or-later
//! From a list of cores to what a front end may drive
//! (`backlog/docs/design/core-contract.md` §13.3, §13.4).
//!
//! A program is built with a fixed list of cores, which the application
//! names in one place and hands down. [`Cores`] checks each against the
//! contract this program drives, by the value the core states
//! (`CoreDescription::implements`, §3.2): a core that states another
//! contract is kept with the reason, for the About text, and is never asked
//! anything. The About lines of both programs come from here, so the
//! desktop and the command line list the same cores the same way.
//!
//! The launch path of a file is here too, and names no platform and no
//! host location: [`Cores::filters`] for the host's file dialog, then
//! [`prepare`], which hashes the file, asks the cores and checks the config
//! the answering core wrote (on a worker thread), and [`start`], which
//! builds the machine over the verified file and a storage in the
//! directories the application chose ([`HostDirs`]).

mod error;
mod launch;

pub use crate::error::LaunchError;
pub use crate::launch::{prepare, start, HostDirs, Outcome, Prepared};

use unemu_core_api::{ContentKind, ContractVersion, Core, CoreDescription, CONTRACT};

/// The cores a program was built with, checked against the contract it
/// drives. A core that states another contract is kept with the reason, for
/// the About text, and is never asked anything.
pub struct Cores {
    /// Every core of the list, in its order, with the reason it cannot be
    /// driven, if any.
    entries: Vec<(&'static dyn Core, Option<String>)>,
    /// The refused cores, in the order of the list.
    refused: Vec<(&'static CoreDescription, String)>,
}

impl Cores {
    /// Check `cores` against the contract this program was built with.
    pub fn new(cores: &'static [&'static dyn Core]) -> Self {
        let entries: Vec<_> = cores
            .iter()
            .map(|&core| (core, refusal(CONTRACT, core.description())))
            .collect();
        let refused = entries
            .iter()
            .filter_map(|(core, reason)| Some((core.description(), reason.clone()?)))
            .collect();
        Self { entries, refused }
    }

    /// The cores this program can drive, in the order of the list.
    pub fn usable(&self) -> impl Iterator<Item = &'static dyn Core> + '_ {
        self.entries
            .iter()
            .filter(|(_, reason)| reason.is_none())
            .map(|(core, _)| *core)
    }

    /// The cores it cannot, each with "core NAME implements contract A.B;
    /// this program needs C.D".
    pub fn refused(&self) -> &[(&'static CoreDescription, String)] {
        &self.refused
    }

    /// The usable core with the id `id`. A refused core is never asked
    /// anything, so it is not found here either.
    pub fn by_id(&self, id: &str) -> Option<&'static dyn Core> {
        self.usable().find(|core| core.description().id == id)
    }

    /// One line per core of the list, in its order, for the About text of
    /// both programs: `core ID RELEASE (NAME), contract A.B`, followed for a
    /// refused core by `, unusable: ` and the reason.
    pub fn about_lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|(core, reason)| {
                let description = core.description();
                let line = format!(
                    "core {} {} ({}), contract {}",
                    description.id, description.version, description.name, description.implements
                );
                match reason {
                    Some(reason) => format!("{line}, unusable: {reason}"),
                    None => line,
                }
            })
            .collect()
    }

    /// One file-dialog filter per platform that opens content, in the order
    /// of the list, then "All supported files" with every extension any
    /// core opens, then "All files". A platform's filter is named after
    /// its one content kind (`NES cartridge images (*.nes)`), or after the
    /// platform and its kinds when it has several. A kind that breaks the
    /// contract's rules for its fields is left out, and so is a platform
    /// left with no extension; two platforms that give the same line (two
    /// cores of one platform) give it once; "All supported files" comes
    /// only when some core opens something.
    pub fn filters(&self) -> Vec<Filter> {
        let mut filters = Vec::new();
        let mut all = Vec::new();
        for core in self.usable() {
            for platform in core.description().platforms {
                let kinds: Vec<&ContentKind> = valid_kinds(platform.content).collect();
                let mut extensions = Vec::new();
                for kind in &kinds {
                    add_unique(&mut extensions, kind.extensions);
                }
                if extensions.is_empty() {
                    continue;
                }
                let name = match kinds.as_slice() {
                    [kind] => kind.name.to_owned(),
                    _ => format!(
                        "{}: {}",
                        platform.name,
                        kinds
                            .iter()
                            .map(|kind| kind.name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                add_unique(&mut all, &extensions);
                let filter = Filter::new(name, extensions);
                if !filters.contains(&filter) {
                    filters.push(filter);
                }
            }
        }
        if !all.is_empty() {
            filters.push(Filter::new("All supported files".to_owned(), all));
        }
        filters.push(Filter {
            name: "All files (*.*)".to_owned(),
            extensions: vec!["*"],
        });
        filters
    }
}

/// A file-dialog filter: a name for people, with its patterns, and the
/// extensions it shows. "All files" shows the one extension `*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    /// `NAME (*.a;*.b)`.
    pub name: String,
    /// Without the dot, in lower case.
    pub extensions: Vec<&'static str>,
}

impl Filter {
    fn new(name: String, extensions: Vec<&'static str>) -> Self {
        Self {
            name: format!("{name} ({})", patterns(&extensions)),
            extensions,
        }
    }
}

/// `*.a;*.b`.
fn patterns(extensions: &[&str]) -> String {
    extensions
        .iter()
        .map(|extension| format!("*.{extension}"))
        .collect::<Vec<_>>()
        .join(";")
}

fn add_unique(to: &mut Vec<&'static str>, extensions: &[&'static str]) {
    for extension in extensions {
        if !to.contains(extension) {
            to.push(extension);
        }
    }
}

/// The kinds a front end may trust: the fields are public, so a kind is
/// checked before it is shown or matched.
pub(crate) fn valid_kinds(
    kinds: &'static [ContentKind],
) -> impl Iterator<Item = &'static ContentKind> {
    kinds.iter().filter(|kind| kind.is_valid())
}

/// What `cores` open, one `NAME (*.ext;…)` per content kind, in the order
/// of the list, for the refusals that say so.
pub(crate) fn kind_lines(cores: impl Iterator<Item = &'static dyn Core>) -> Vec<String> {
    let mut lines = Vec::new();
    for core in cores {
        for platform in core.description().platforms {
            for kind in valid_kinds(platform.content) {
                let line = if kind.extensions.is_empty() {
                    kind.name.to_owned()
                } else {
                    format!("{} ({})", kind.name, patterns(kind.extensions))
                };
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
        }
    }
    lines
}

/// Why a front end written for `contract` cannot drive the core `core`
/// describes, or `None` when it can (§3.2).
fn refusal(contract: ContractVersion, core: &CoreDescription) -> Option<String> {
    (!contract.drives(core.implements)).then(|| {
        format!(
            "core {} implements contract {}; this program needs {contract}",
            core.name, core.implements
        )
    })
}

#[cfg(test)]
mod tests;
