// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop library's models that know no platform and draw nothing.
//!
//! The headless halves of the library's registry and listing code: the
//! Preferences › Registries page ([`registries_model`]), the rows the image
//! lists show ([`listing`]) and the Save as new image… form
//! ([`save_image`]). They build without a toolkit and without any PC crate,
//! so a view change in the desktop never rebuilds or reruns them; the views
//! that draw them stay in the desktop. See
//! `backlog/docs/design/core-contract.md` §8.1 for the split and the layer
//! rules (§7.3): this crate is `neutral`, so it may depend only on the
//! contract and other neutral crates, its tests included.

pub mod listing;
pub mod registries_model;
pub mod save_image;
