// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pull… from Remotes' model is `library_model::remote_pull`, re-exported
//! here under the path the library frame names it by, with the PC's
//! description of a stopped machine's floppy drive. Its tests read PC
//! artifacts and live here, beside the frame.

pub use library_model::remote_pull::*;

/// A floppy drive of a stopped PC as Pull and insert… offers it: named by
/// its letter and format, and taking the formats its drive type reads (IBM
/// Personal Computer AT Technical Reference, Diskette Drive Adapter: which
/// media a drive type takes, as `FloppyGeometry::accepted_media` has it).
pub(crate) fn pc_drive(
    machine: String,
    name: String,
    unit: u8,
    geometry: assembly::FloppyGeometry,
    holds: Option<String>,
) -> Drive {
    Drive {
        machine,
        name,
        unit,
        slot: crate::media_picker::drive_name(unit),
        format: crate::media_picker::format_name(geometry),
        takes: geometry
            .accepted_media()
            .into_iter()
            .filter_map(assembly::descriptors::floppy_type_name)
            .map(str::to_owned)
            .collect(),
        holds,
    }
}

#[cfg(test)]
mod tests;
