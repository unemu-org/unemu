// SPDX-License-Identifier: AGPL-3.0-or-later
//! The standalone PC command line; core-contract.md §12.8.

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let code = unemupc::run(&args, &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(i32::from(code));
}
