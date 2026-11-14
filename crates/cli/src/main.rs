// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC/AT frontend and offline media tools; persistent-media.md § Offline commands.

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let sources = cli::command::FirmwareSources {
        store_root: image_store::location::default_store_root().ok(),
        index: assembly::firmware_discovery::firmware_cache_path(),
    };
    let code = cli::command::run_with_firmware_sources(
        &args,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        &sources,
    );
    std::process::exit(i32::from(code));
}
