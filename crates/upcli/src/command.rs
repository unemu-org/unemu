// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `unemu` program: the platform-neutral command line (`cli_core`) with
//! the platforms compiled in, which today is the PC (`cli_pc`), and the
//! program's own `--version` and `--about`;
//! `backlog/docs/design/core-contract.md` §7.1. The executable and the tests
//! that pin the composed command line come through here.

use cli_core::{CliPlatform, Io};
use core_launch::Cores;
use std::ffi::OsString;
use std::io::Write;

/// Host paths are supplied by the executable, never discovered by library tests.
pub use cli_core::HostSources as FirmwareSources;
/// The journal report and its exit code, which tests of the journal tools read.
pub use cli_pc::journal::{render_report, report_exit_code};

/// A parsed `unemu` command line.
pub enum Command {
    /// `--version` or `--about`: the licence and third-party notices.
    About,
    /// A command of the neutral command line or of a platform.
    Line(cli_core::Command),
}

// The program's parse pin predates the neutral stored-run commands. Keep
// its diagnostic representation stable while their implementation moves.
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use cli_core::image::ImageCommand;
        match self {
            Self::About => f.write_str("About"),
            Self::Line(cli_core::Command::Image(command @ ImageCommand::Run { .. })) => {
                write!(f, "Line(Platform(Image({command:?})))")
            }
            Self::Line(cli_core::Command::Image(ImageCommand::MachineRun {
                store,
                id,
                max_steps,
            })) => {
                write!(f, "Line(Platform(Image(Machine(MachineCommand {{ store: {store:?}, action: Run {{ id: {id:?}, max_steps: {max_steps:?} }} }}))))")
            }
            Self::Line(command) => f.debug_tuple("Line").field(command).finish(),
        }
    }
}

/// `--version` and `--about` stand alone; nothing else on the command line
/// starts with them.
fn about(args: &[OsString]) -> bool {
    matches!(args, [arg] if arg == "--version" || arg == "--about")
}

/// Parse an `unemu` command line with the platforms compiled in.
pub fn parse(args: &[OsString]) -> Result<Command, &'static str> {
    if about(args) {
        return Ok(Command::About);
    }
    cli_pc::with_platform(|pc| cli_core::parse(args, &[pc])).map(Command::Line)
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
    let cores = Cores::new(platforms::cores());
    cli_pc::with_platform(|pc| {
        run_program(args, &mut Io { output, errors }, sources, &[pc], &cores)
    })
}

/// Run an `unemu` command line with `platforms` and `cores`: the program's
/// notices, or the command line of [`cli_core::run`].
pub fn run_program(
    args: &[OsString],
    io: &mut Io<'_>,
    host: &FirmwareSources,
    platforms: &[&dyn CliPlatform],
    cores: &Cores,
) -> u8 {
    if !about(args) {
        return cli_core::run(args, io, host, platforms, cores);
    }
    match io
        .output
        .write_all(crate::about::unemu_notice(cores).as_bytes())
    {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(io.errors, "{error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn usage_names_the_program_unemu() {
        let mut output = Vec::new();
        assert_eq!(run(&[], &mut output, &mut io::sink()), 0);
        let usage = String::from_utf8(output).unwrap();
        assert!(usage.starts_with("usage: unemu --version"), "{usage}");
        let commands = usage
            .lines()
            .skip(1)
            .filter(|line| line.starts_with("       ") && !line.starts_with("        "));
        for line in commands {
            assert!(line.trim_start().starts_with("unemu "), "{line}");
        }
        let former = image_store::location::preferences::FORMER_DIRECTORY;
        assert!(!usage.to_ascii_lowercase().contains(former), "{usage}");
    }

    #[test]
    fn version_and_about_name_the_licence_source_and_third_party_notices() {
        for option in ["--version", "--about"] {
            let mut output = Vec::new();
            let mut errors = Vec::new();
            assert_eq!(
                run(&[option.into()], &mut output, &mut errors),
                0,
                "{option}"
            );
            assert!(errors.is_empty(), "{option}");
            let text = String::from_utf8(output).unwrap();
            assert!(
                text.starts_with(&format!("unemu {}\n", env!("CARGO_PKG_VERSION"))),
                "{text}"
            );
            for expected in [
                "AGPL-3.0-or-later",
                "https://github.com/unemu-org/unemupc",
                "Nuked-OPL3",
                "LGPL-2.1-or-later",
                "THIRD-PARTY-LICENSES.md",
            ] {
                assert!(text.contains(expected), "{option}: {expected}");
            }
        }
    }
}
