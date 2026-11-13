// SPDX-License-Identifier: AGPL-3.0-or-later
//! Legacy test entry points for production host ROM discovery; commands.md §User-supplied ROM tests.

pub use crate::firmware_discovery::{
    required_user_firmware, FoundUserFirmware, UserFirmwareError, UserFirmwareLayout,
};

/// Discover with the historical vendor and environment roots, preserving the
/// test API while sharing all hashing and layout matching with frontends.
pub fn find_user_firmware(
    entry_id: &str,
    catalogue: &[crate::FirmwareEntry],
) -> Result<Option<FoundUserFirmware>, UserFirmwareError> {
    crate::firmware_discovery::find_test_firmware(
        entry_id,
        catalogue,
        &crate::firmware_discovery::default_search_roots(),
    )
}
