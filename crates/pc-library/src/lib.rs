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
//! other builds and marks superseded firmware. The library's worker and the
//! PC's provider seam join them as the desktop split goes on; the views that
//! draw them stay with the desktop.
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
pub mod machines;
pub mod snapshot;
