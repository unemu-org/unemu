// SPDX-License-Identifier: AGPL-3.0-or-later
//! Licence, source and third-party notices that both binaries print for
//! `--version` and `--about`.

pub use platforms::licence_notice;

/// `upcli --version` and `upcli --about`, with a line for each of `cores`.
pub fn upcli_notice(cores: &core_launch::Cores) -> String {
    let mut lines = String::new();
    for line in cores.about_lines() {
        lines.push_str(&line);
        lines.push('\n');
    }
    format!(
        "upcli {}\nemulator of period IBM-compatible PCs, command line\n{lines}{}\n\nthird-party components:\n\n{}\n",
        env!("CARGO_PKG_VERSION"),
        licence_notice(),
        devices::OPL3_NOTICE,
    )
}
