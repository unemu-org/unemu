// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's PC views, drawn with egui: the machine library's frame and
//! the PC dialogs; see `backlog/docs/design/core-contract.md` §8.1.
//!
//! [`library_view`] is the library's frame: the Explorer tree, the lists and
//! details panes of images, machines, firmware packages and installation
//! media, and Remotes, with the dialogs the frame owns; its push and pull
//! dialogs drive [`registry_transfer`] and Pull… from Remotes
//! [`remote_pull`]. [`settings_sheet`] draws Settings… of a machine file, Settings on an image
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
//! disk layers (layer `pc`, `scripts/crate-boundaries.sh`). Behind the
//! `test-support` feature, the frame's modules hold the fixtures the
//! application's tests drive the frame with.

/// This crate's source folder, for the desktop's test that every string it
/// draws has a glyph.
#[cfg(any(test, feature = "test-support"))]
pub fn source_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub mod commands;
pub mod layers_dialog;
pub mod library_view;
pub mod new_machine_view;
pub mod pane;
pub mod registry_transfer;
pub mod remote_pull;
pub mod settings_sheet;

// The PC library models, under the paths the dialogs name them by; the
// three with tests of their own here re-export the model beside them.
mod image_settings;
mod library_worker;
mod new_machine;
use pc_library::{firmware_choices, pc_desktop, settings_model};
use pc_media::{catalogue_menu, created_media, kept_catalogue};
// The neutral registry models, under the paths the frame names them by.
#[cfg(test)]
use library_model::registries_model;
use library_model::remote_browse;

/// The media picker's model items the frame names, under the picker's path,
/// with the picker's fixture that a test of the Installation media
/// collection builds a set with.
mod media_picker {
    pub(crate) use pc_media::media_picker::*;

    #[cfg(test)]
    pub(crate) mod tests {
        pub(crate) use media_ui::media_picker::test_support::set;
    }
}

/// The push and pull dialogs, under the path the frame names them by, with
/// copies of the drawing helpers the Remotes tests use; the dialogs' own
/// test plans a push with the program's provider list and stays with the
/// application.
mod registry_transfer_view;

/// The one "Save as new image…" form: its model and its view.
mod save_image {
    pub(crate) use library_model::save_image::*;
    pub(crate) use library_ui::save_image::{form, prompts};
}

/// Waiting on background threads in tests: the library's hang guard.
#[cfg(any(test, feature = "test-support"))]
use pc_library::test_wait;

/// The library's fixtures and the kit's helpers that drive egui frame by
/// frame, under the path the tests name them by.
#[cfg(test)]
mod library_test_support {
    pub(crate) use pc_library::library_test_support::*;
    pub(crate) use ui_kit::test_support::{painted_at, settled_modal, Pointer};
}
