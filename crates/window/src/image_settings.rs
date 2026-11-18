// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless Settings on an image: the PC library's model
//! (`pc_library::image_settings`), re-exported under its old path, with the
//! helpers that build images for the desktop's tests. Its tests live with the
//! model and with the settings sheet (`pc_ui`).

pub(crate) use pc_library::image_settings::*;

#[cfg(test)]
pub(crate) mod tests {
    // The helpers live with the model; the desktop's tests name them here.
    pub(crate) use pc_library::image_settings::test_support::{
        open_image, open_new_image, opened, reviewed, write_c,
    };
}
