// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop library's views that know no platform, drawn with egui.
//!
//! The drawn halves of the library's registry and listing code: the list
//! tables every collection and Remotes draw with ([`table`]), the Save as
//! new image… form and its questions ([`save_image`]), the Push…, Pull by
//! reference… and Pull… dialogs ([`registry_transfer_view`]) and the
//! Remotes group with its details pane ([`remotes`]), which the library
//! frame lends what it reads of the library and which answers with what it
//! asks of the frame. The models they draw are `library_model`'s. See
//! `backlog/docs/design/core-contract.md` §8.1 for the split and the layer
//! rules (§7.3): this crate is `neutral`, so it may depend only on the
//! contract and other neutral crates, its tests included.

/// This crate's source folder, for the desktop's test that every string it
/// draws has a glyph.
#[cfg(any(test, feature = "test-support"))]
pub fn source_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub mod registry_transfer_view;
pub mod remotes;
pub mod save_image;
pub mod table;
