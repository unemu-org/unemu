// SPDX-License-Identifier: AGPL-3.0-or-later
//! Frontend binary (§3.1).
//!
//! Builds a [`Machine`] and writes the bytes the guest sent to COM1 to stdout.
//! The end-to-end path is CLI → Machine → Bus → Device → serial out.
//!
//! Usage:
//! - `unemupc98` — the built-in Long64 "hello" serial demo (a lone 16550 UART).
//! - `unemupc98 --boot` — the built-in real-mode timer-interrupt demo on the PC/AT
//!   machine shape (prints five `.` markers via IRQ0).
//! - `unemupc98 <BLOB>` — map a flat Long64 binary at the entry point and run it.
//!
//! [`Machine`]: machine::Machine

use std::io::Write;

use anyhow::{Context, Result};
use cli::{run_boot, run_serial, BOOT_PROGRAM, HELLO_PROGRAM};

fn main() -> Result<()> {
    let serial = match std::env::args().nth(1) {
        Some(arg) if arg == "--boot" => run_boot(BOOT_PROGRAM)?,
        Some(path) => {
            let blob = std::fs::read(&path).with_context(|| format!("reading blob {path}"))?;
            run_serial(&blob)?
        }
        None => run_serial(HELLO_PROGRAM)?,
    };

    std::io::stdout()
        .write_all(&serial)
        .context("writing serial output")?;
    Ok(())
}
