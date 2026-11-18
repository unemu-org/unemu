// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's PC media views, drawn with egui.
//!
//! The media picker, one Insert… per floppy drive, with its state and its
//! view ([`media_picker`]); Create from files…, the picker's second view
//! ([`media_create`]); and a floppy drive's Catalogue submenu, which answers
//! with the disk chosen and leaves the command to the shell
//! ([`catalogue_menu`]). The media models they draw are `pc_media`'s; the
//! pane colours come from `ui_kit::pane`. See
//! `backlog/docs/design/core-contract.md` §8.1 for the split and
//! `backlog/docs/design/machine-images.md` §5.2 and §7 for the views.
//!
//! The crate is PC: it draws floppy geometries, the PC's media sets and its
//! catalogue entries (layer `pc`, `scripts/crate-boundaries.sh`).

/// This crate's source folder, for the desktop's test that every string it
/// draws has a glyph.
#[cfg(any(test, feature = "test-support"))]
pub fn source_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub mod catalogue_menu;
pub mod media_create;
pub mod media_picker;
