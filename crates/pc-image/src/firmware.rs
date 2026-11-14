// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC firmware package provider; machine-images §5.3.
//! IBM Personal Computer AT Technical Reference, Section 1, System Board ROM:
//! logical BIOS packages preserve even/odd socket lanes and assembled offsets.

pub use machine_file::firmware_package::{role, Fit, PcFirmwareRules, BOARDS, MACHINE_CLASSES};

/// Storage roles include controllers which are not yet executable by the machine.
pub const ROLES: &[&str] = &[
    "system-bios",
    "option-rom",
    "video-bios",
    "keyboard-controller",
    "other",
];

/// Production catalogue; callers may inject synthetic identities for isolation tests.
pub fn catalogue() -> &'static [assembly::FirmwareEntry] {
    assembly::BoardModel::Ibm5170.firmware_catalogue()
}
