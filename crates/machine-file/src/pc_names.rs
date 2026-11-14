// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC guest attachment names shared by media and firmware; machine-images §§5.2–5.3.

/// Canonical guest attachment names shared by PC media and firmware providers.
pub fn valid_dos_name(name: &str) -> bool {
    let mut parts = name.split('.');
    let stem = parts.next().unwrap_or_default();
    let extension = parts.next();
    let valid = |s: &str, max| {
        !s.is_empty()
            && s.len() <= max
            && s.bytes().all(|b| {
                b.is_ascii_uppercase() || b.is_ascii_digit() || b"!#$%&'()-@^_`{}~".contains(&b)
            })
    };
    valid(stem, 8)
        && extension.is_none_or(|s| valid(s, 3))
        && parts.next().is_none()
        && !matches!(stem, "CON" | "NUL" | "AUX" | "PRN" | "CLOCK$")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
