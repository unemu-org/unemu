// SPDX-License-Identifier: AGPL-3.0-or-later
//! Command composition for `unemu` (§3.1): the platform-neutral command line of
//! `cli-core` with the platforms compiled in, which today is the PC's
//! (`cli-pc`), and the program's own `--version` and `--about`
//! (`backlog/docs/design/core-contract.md` §7.1). [`command`] composes them;
//! the frontend and the tests that pin the composed command line come
//! through it.

pub mod about;
pub mod command;

/// Run a command word using the host's default store and firmware index.
/// The desktop owns informational options and dispatches here before opening
/// a window (core-contract.md §12.13 Q1).
pub fn run_command(args: &[std::ffi::OsString]) -> u8 {
    let host = command::FirmwareSources {
        store_root: image_store::location::default_store_root().ok(),
        index: machine_file::firmware_discovery::firmware_cache_path(),
    };
    command::run_with_firmware_sources(args, &mut std::io::stdout(), &mut std::io::stderr(), &host)
}

/// The composed command-line usage, for the frontend's combined help.
pub fn usage() -> String {
    cli_pc::with_platform(|pc| cli_core::usage(&[pc]))
}
