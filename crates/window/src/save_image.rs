// SPDX-License-Identifier: AGPL-3.0-or-later
//! The one "Save as new image…" form: its model and tag rules are
//! `library_model::save_image`, and the form and its questions are drawn by
//! `library_ui::save_image`; both are re-exported here under the old path.

pub(crate) use library_model::save_image::*;
pub(crate) use library_ui::save_image::{form, prompts};
