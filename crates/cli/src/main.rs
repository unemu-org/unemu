// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC/AT frontend and offline media tools; persistent-media.md § Offline commands.
//! The program is the neutral command line with the platforms compiled in:
//! today the PC's.

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let host = cli::cli_core::HostSources {
        store_root: image_store::location::default_store_root().ok(),
        index: assembly::firmware_discovery::firmware_cache_path(),
    };
    let code = cli::cli_pc::with_platform(|pc| {
        cli::command::run_program(
            &args,
            &mut cli::cli_core::Io {
                output: &mut std::io::stdout(),
                errors: &mut std::io::stderr(),
            },
            &host,
            &[pc],
        )
    });
    std::process::exit(i32::from(code));
}
