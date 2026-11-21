// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command line with the PC platform alone: the neutral command line of
//! `cli-core` with [`crate::PcPlatform`] offered every command it does not
//! take itself (`backlog/docs/design/core-contract.md` §7.2). The program's
//! own `--version` and `--about` are not here: the `unemu` program owns them
//! with the notices of everything compiled into it. The tests of the PC
//! commands drive the command line through here, as the program does.

use cli_core::{Command, Io};
use std::ffi::OsString;
use std::io::Write;

/// Host paths are supplied by the executable, never discovered by library tests.
pub use cli_core::HostSources as FirmwareSources;

/// Parse a command line with the PC platform.
pub fn parse(args: &[OsString]) -> Result<Command, &'static str> {
    crate::with_platform(|pc| cli_core::parse(args, &[pc]))
}

/// Library runner with host firmware defaults disabled. The executable supplies
/// its configured sources through `run_with_firmware_sources`.
pub fn run(args: &[OsString], output: &mut impl Write, errors: &mut impl Write) -> u8 {
    run_with_firmware_sources(args, output, errors, &FirmwareSources::default())
}

/// Run with explicit host defaults. None disables the corresponding host source.
pub fn run_with_firmware_sources(
    args: &[OsString],
    output: &mut impl Write,
    errors: &mut impl Write,
    sources: &FirmwareSources,
) -> u8 {
    let mut sources = sources.clone();
    if sources.compatibility.frontend.is_none() {
        use unemu_core_api::Core;
        let version = unemu_pc_core::PcCore.description().version.parse();
        let Ok(version) = version else {
            let _ = writeln!(errors, "invalid PC core release");
            return 1;
        };
        sources.compatibility = image_store::compat::Compatibility::new(
            image_store::compat::EXTRACTION_BASELINE,
            pc_image::compat::newest_frontend_row(),
        )
        .with_core(
            pc_image::PLATFORM,
            version,
            pc_image::compat::core_newest_row(),
        );
    }
    crate::with_platform(|pc| {
        cli_core::run(
            args,
            &mut Io { output, errors },
            &sources,
            &[pc],
            &core_launch::Cores::new(&[&unemu_pc_core::PcCore]),
        )
    })
}
