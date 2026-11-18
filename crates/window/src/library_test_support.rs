// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless desktop sessions use synthetic firmware, disks, catalogue and CMOS.
//!
//! The library's fixtures live with the library (`pc_library`'s test
//! support) and the modal helper that drives the toolkit with the kit
//! (`ui_kit::test_support`); both are re-exported here.
pub(crate) use pc_library::library_test_support::*;
pub(crate) use ui_kit::test_support::settled_modal;
