// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's PC dialogs, drawn with egui; see
//! `backlog/docs/design/core-contract.md` §8.1.
//!
//! [`settings_sheet`] draws Settings… of a machine file, Settings on an image
//! and the review of the image's new revision; [`new_machine_view`] draws
//! File › New machine…'s wizard; [`layers_dialog`] is the disk-layer dialog
//! with its save form; [`pane`] holds the PC firmware pieces of the library's
//! details panes beside the kit's shared layout. [`commands`] is the
//! desktop's command vocabulary, which the views push and the shell carries
//! out. The models they draw are
//! `pc_library`'s, the save form's model and view `library_model`'s and
//! `library_ui`'s.
//!
//! The crate is PC: it draws the PC firmware catalogue, machine tables and
//! disk layers (layer `pc`, `scripts/crate-boundaries.sh`).

/// This crate's source folder, for the desktop's test that every string it
/// draws has a glyph.
#[cfg(any(test, feature = "test-support"))]
pub fn source_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub mod commands;
pub mod layers_dialog;
pub mod new_machine_view;
pub mod pane;
pub mod settings_sheet;

// The PC library models, under the paths the dialogs name them by; the
// three with tests of their own here re-export the model beside them.
mod image_settings;
mod library_worker;
mod new_machine;
use pc_library::{firmware_choices, settings_model};

/// The one "Save as new image…" form: its model and its view.
mod save_image {
    pub(crate) use library_model::save_image::*;
    pub(crate) use library_ui::save_image::{form, prompts};
}

/// Waiting on background threads in tests: the library's hang guard.
#[cfg(test)]
use pc_library::test_wait;

/// The library's fixtures and the kit's helpers that drive egui frame by
/// frame, under the path the tests name them by.
#[cfg(test)]
mod library_test_support {
    pub(crate) use pc_library::library_test_support::*;
    pub(crate) use ui_kit::test_support::{painted_at, settled_modal, Pointer};
}
