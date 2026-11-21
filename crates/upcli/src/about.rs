// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command composition's licence, source and third-party notice;
//! the desktop owns the combined executable's informational options.

pub use platforms::licence_notice;

/// `unemu --version` and `unemu --about`, with a line for each of `cores`.
pub fn unemu_notice(cores: &core_launch::Cores) -> String {
    let mut lines = String::new();
    for line in cores.about_lines() {
        lines.push_str(&line);
        lines.push('\n');
    }
    format!(
        "unemu {}\nemulator of period IBM-compatible PCs, command line\n{lines}{}\n\nthird-party components:\n\n{}\n",
        env!("CARGO_PKG_VERSION"),
        licence_notice(),
        devices::OPL3_NOTICE,
    )
}
