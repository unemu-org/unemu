// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's PC library models, without a toolkit; see
//! `backlog/docs/design/core-contract.md` §8.1.
//!
//! [`firmware_choices`] names the firmware a machine's or an image's
//! hardware tables bind, offers the catalogue's other builds for Settings…
//! and marks superseded firmware. The library's other PC models, its worker
//! and the PC's provider seam join it as the desktop split goes on; the
//! views that draw them stay with the desktop.
//!
//! The crate is PC: it reads the PC firmware catalogue and machine tables
//! (layer `pc`, `scripts/crate-boundaries.sh`).

/// This crate's source folder, for the desktop's test that every string it
/// draws has a glyph.
#[cfg(any(test, feature = "test-support"))]
pub fn source_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub mod firmware_choices;
