// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's PC media models, without a toolkit; see
//! `backlog/docs/design/core-contract.md` §8.1 and
//! `backlog/docs/design/machine-images.md` §7.
//!
//! [`media_dialog`] holds the floppy catalogue drafts of the Insert floppy
//! dialog, the blank-disk form and the floppies that wait for a Start;
//! [`created_media`] the disks Create from files… builds and their pending
//! record; [`kept_catalogue`] the catalogue a library machine keeps at Stop;
//! [`drive_chips`] the status bar's drive views. [`media_picker`] and
//! [`catalogue_menu`] hold the parts of the picker and the Catalogue submenu
//! that these name: the installation sets, the disks in a catalogue and a
//! disk's short name. The views that draw them stay with the desktop.
//!
//! The crate is PC: it reads machine configs, floppy geometries and the PC's
//! media sets (layer `pc`, `scripts/crate-boundaries.sh`).

/// This crate's source folder, for the desktop's test that every string it
/// draws has a glyph.
#[cfg(any(test, feature = "test-support"))]
pub fn source_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub mod catalogue_menu;
pub mod created_media;
pub mod drive_chips;
pub mod kept_catalogue;
pub mod media_dialog;
pub mod media_picker;
