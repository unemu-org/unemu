// SPDX-License-Identifier: AGPL-3.0-or-later
// Input for the affected-tests self-test: which ignored tests a changed file
// holds. Not compiled.

#[test]
fn ordinary_test_is_not_listed() {}

#[test]
#[ignore = "slow: boots a guest"]
fn slow_golden_is_listed() {}

#[test]
#[ignore = "requires user-supplied firmware"]
fn user_rom_test_is_left_to_the_user_rom_tier() {}

/// Mentions `#[ignore]` in prose before the attribute.
#[test]
#[ignore]
#[cfg(unix)]
fn attribute_after_ignore_is_skipped_over() {}

mod nested {
    #[test]
    #[ignore = "slow"]
    pub fn nested_golden_is_listed() {}
}
