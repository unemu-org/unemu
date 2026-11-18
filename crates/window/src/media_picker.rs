// SPDX-License-Identifier: AGPL-3.0-or-later
//! The media picker: one Insert… per floppy drive, as the approved
//! media-picker mock-up draws it; machine-images §5.2 and §7.
//!
//! The picker's state and view live in `media_ui::media_picker` and are
//! re-exported here, under the path the shell names them by.

pub(crate) use media_ui::media_picker::*;

// The picker's fixtures, under the path the desktop's tests name them by.
#[cfg(test)]
pub(crate) mod tests {
    pub(crate) use media_ui::media_picker::test_support::{set, set_outcome};
}
