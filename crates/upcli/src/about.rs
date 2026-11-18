// SPDX-License-Identifier: AGPL-3.0-or-later
//! Licence, source and third-party notices that both binaries print for
//! `--version` and `--about`.

pub use platforms::licence_notice;

/// `upcli --version` and `upcli --about`.
pub fn upcli_notice() -> String {
    format!(
        "upcli {}\nemulator of period IBM-compatible PCs, command line\n{}\n\nthird-party components:\n\n{}\n",
        env!("CARGO_PKG_VERSION"),
        licence_notice(),
        devices::OPL3_NOTICE,
    )
}
