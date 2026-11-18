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
//! The rest of the launch path (the file-dialog filters, opening a file,
//! choosing a core, creating the machine) comes to this crate later; it
//! names no platform and no host location.

use unemu_core_api::{ContractVersion, Core, CoreDescription, CONTRACT};

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
