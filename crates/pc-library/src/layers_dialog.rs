// SPDX-License-Identifier: AGPL-3.0-or-later
//! The model of the whole-machine disk-layer dialog: the image a workspace's
//! session pins, as the library worker reads it for the Save as new image…
//! form (machine-images §§4 and 7). The dialog itself draws with the
//! desktop's toolkit and stays with the desktop's views.

use image_store::Digest;

/// The image the workspace's session pins: its reference, digest and title,
/// read by the library worker.
#[derive(Clone, Debug, Default)]
pub struct SourceImage {
    pub reference: String,
    pub digest: Option<Digest>,
    pub title: Option<String>,
    /// Every local tag, for the tag rules.
    pub tags: Vec<(String, Digest)>,
    /// An unfinished save of this workspace: its tag and expected digest.
    pub pending: Option<(String, Option<Digest>)>,
}
