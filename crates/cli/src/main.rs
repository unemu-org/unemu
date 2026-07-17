// SPDX-License-Identifier: AGPL-3.0-or-later
//! Frontend binary (§3.1).
//!
//! Builds a [`Machine`] from the [`SerialChipset`] (a single 16550 UART at
//! COM1), loads a flat Long64 code blob, runs it to `hlt`, and writes the bytes
//! the guest sent to COM1 to stdout. The end-to-end path is
//! CLI → Machine → Bus → Device → serial out.
//!
//! Usage: `unemupc98 [BLOB]`. With no argument it runs the built-in "hello"
//! demo program (the same bytes the golden test asserts). With a path, it maps
//! that flat binary at the entry point and runs it.
//!
//! [`Machine`]: machine::Machine
//! [`SerialChipset`]: chipset::SerialChipset

use std::io::Write;

use anyhow::{Context, Result};
use cli::{run_serial, HELLO_PROGRAM};

fn main() -> Result<()> {
    let blob = match std::env::args().nth(1) {
        Some(path) => std::fs::read(&path).with_context(|| format!("reading blob {path}"))?,
        None => HELLO_PROGRAM.to_vec(),
    };

    let serial = run_serial(&blob)?;
    std::io::stdout()
        .write_all(&serial)
        .context("writing serial output")?;
    Ok(())
}
