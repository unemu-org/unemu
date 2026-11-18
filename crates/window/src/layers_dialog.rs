// SPDX-License-Identifier: AGPL-3.0-or-later
//! The disk-layer dialog: `pc_ui::layers_dialog`, re-exported here under the
//! path the library frame names it by, with the helpers the frame's tests
//! drive it with.

pub(crate) use pc_ui::layers_dialog::*;

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) use pc_ui::layers_dialog::test_support::{finish, reviewed_publication};
}
