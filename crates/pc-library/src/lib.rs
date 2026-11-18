// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's PC library models, without a toolkit; see
//! `backlog/docs/design/core-contract.md` §8.1.
//!
//! The library's collections, as the worker lists them and the frame shows
//! them: [`snapshot`] answers the frame's query with every list's rows and
//! facets, from the [`machines`], [`firmware`] and [`install_media`]
//! models; [`firmware_metadata`] holds a firmware package's statement and
//! the worker's sides of Edit metadata…; [`hardware`] names an image's
//! hardware, drives and firmware for the details panes; [`image_settings`]
//! is Settings… on an image; [`firmware_choices`] names the firmware a
//! machine's or an image's hardware tables bind, offers the catalogue's
//! other builds and marks superseded firmware. [`library_worker`] is the
//! library's worker thread, which runs every store, registry and machine job
//! off the desktop's frame loop; [`pc_desktop`] is the PC's part of the
//! platform-neutral registry and Remotes models; [`layers_dialog`] holds what
//! the worker reads for the disk-layer dialog's save form. [`new_machine`]
//! is File › New machine…'s wizard and [`settings_model`] the property
//! sheet of a machine file, both without their views; [`machine_files`]
//! lists the machine-file directory and checks each machine's readiness off
//! the frame loop. The views that draw them are in `pc_ui`.
//!
//! The crate is PC: it reads the PC firmware catalogue and machine tables
//! (layer `pc`, `scripts/crate-boundaries.sh`).

/// This crate's source folder, for the desktop's test that every string it
/// draws has a glyph.
#[cfg(any(test, feature = "test-support"))]
pub fn source_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub mod firmware;
pub mod firmware_choices;
pub mod firmware_metadata;
pub mod hardware;
pub mod image_settings;
pub mod install_media;
pub mod layers_dialog;
#[cfg(any(test, feature = "test-support"))]
pub mod library_test_support;
pub mod library_worker;
pub mod machine_files;
pub mod machines;
pub mod new_machine;
pub mod pc_desktop;
// The tests of Remotes' model that read PC artifacts or browse through the
// worker.
#[cfg(test)]
mod remote_browse;
pub mod settings_model;
pub mod snapshot;

/// Waiting on background threads in tests.
///
/// A loaded CI runner can take many seconds for work that takes milliseconds
/// on an idle machine, so no test outcome may depend on how soon a worker
/// answers. The only wall-clock bound is this hang guard, which ends a test
/// whose worker never answers instead of letting it run forever.
#[cfg(any(test, feature = "test-support"))]
pub mod test_wait {
    use std::time::{Duration, Instant};

    /// How long a test waits for a background thread before it reports a hang.
    pub const HANG_GUARD: Duration = Duration::from_secs(300);

    /// Rounds of a polling loop until the hang guard has passed. The loop
    /// pauses between rounds itself, and its own check after the loop
    /// reports a hang.
    pub fn rounds() -> impl Iterator<Item = ()> {
        let deadline = Instant::now() + HANG_GUARD;
        std::iter::from_fn(move || (Instant::now() < deadline).then_some(()))
    }
}
