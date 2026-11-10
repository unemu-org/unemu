// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC/AT frontend and offline media tools; persistent-media.md § Offline commands.

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let code = cli::command::run(&args, &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(i32::from(code));
}
