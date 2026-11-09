// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real16 PC/AT frontend (§3.1); `--boot` runs the timer-interrupt demo.

use std::io::Write;

use anyhow::{Context, Result};
use cli::{run_boot, BOOT_PROGRAM};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let usage = "usage: unemupc98 --boot (Real16 timer demo)\nDOS boot and interactive sessions: see crates/cli/tests/*dos*golden.rs";
    if args.is_empty() {
        println!("{usage}");
        return Ok(());
    }
    if args.len() != 1 || args[0] != "--boot" {
        eprintln!("{usage}");
        std::process::exit(2);
    }
    let serial = run_boot(BOOT_PROGRAM)?;
    std::io::stdout()
        .write_all(&serial)
        .context("writing serial output")?;
    Ok(())
}
