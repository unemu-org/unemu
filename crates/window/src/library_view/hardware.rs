// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the details panes say about an image's hardware, drives and
//! firmware: the PC library's model (`pc_library::hardware`), re-exported
//! under its old path, with its tests.

pub(crate) use pc_library::hardware::{
    controller_name, letter, medium_name, DriveFact, FirmwareFact, ImageFacts, CMOS,
};
