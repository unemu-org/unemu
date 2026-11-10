// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real16 PC/AT frontend (§3.1); `--boot` runs the timer-interrupt demo.

use std::io::Write;

use anyhow::{Context, Result};
use cli::{run_boot, BOOT_PROGRAM};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let usage = "usage: unemupc98 --boot (Real16 timer demo)\n       unemupc98 --bios PATH [--allow-unlisted-firmware]\nDOS boot and interactive sessions: see crates/cli/tests/*dos*golden.rs";
    if args.is_empty() {
        println!("{usage}");
        return Ok(());
    }
    let mut options = assembly::AssemblyOptions::default();
    let args: Vec<_> = args
        .iter()
        .filter(|arg| {
            if *arg == "--allow-unlisted-firmware" {
                options.allow_unlisted_firmware = true;
                false
            } else {
                true
            }
        })
        .collect();
    let (serial, stop) = if args.len() == 1 && args[0] == "--boot" {
        (run_boot(BOOT_PROGRAM)?, None)
    } else if args.len() == 2 && args[0] == "--bios" {
        let run = cli::run_bios_file(
            std::path::Path::new(args[1]),
            options,
            &mut std::io::stderr(),
        )?;
        (run.com1, Some(run.stop))
    } else {
        eprintln!("{usage}");
        std::process::exit(2);
    };
    std::io::stdout()
        .write_all(&serial)
        .context("writing serial output")?;
    if let Some(stop) = stop {
        let (message, failed) = bios_stop(&stop);
        eprintln!("{}", message.replace(['\r', '\n'], " "));
        if failed {
            std::process::exit(1);
        }
    }
    Ok(())
}

fn bios_stop(stop: &cli::StopReason) -> (String, bool) {
    use cli::StopReason::*;
    let (reason, failed) = match stop {
        Halt => ("halt".into(), false),
        ResetRequested => ("reset requested".into(), false),
        BudgetExhausted => ("budget exhausted".into(), false),
        Unsupported { detail } => (format!("unsupported instruction: {detail}"), true),
        Unmapped { detail } => (format!("execution error: {detail}"), true),
        Exception { vector } => (format!("exception {vector}"), true),
        ConfigFailed { detail } => (format!("configuration error: {detail}"), true),
        PolicyFailed { detail } => (format!("run policy error: {detail}"), true),
        ObservationFailed { detail } => (format!("observation error: {detail}"), true),
        PersistenceFailed { detail } => (format!("persistence error: {detail}"), true),
    };
    (format!("stopped: {reason}"), failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bios_stop_classifies_every_outcome() {
        use cli::StopReason::*;
        for stop in [Halt, ResetRequested, BudgetExhausted] {
            let (message, failed) = bios_stop(&stop);
            assert!(!failed);
            if stop == BudgetExhausted {
                assert_eq!(message, "stopped: budget exhausted");
            }
        }
        for stop in [
            PersistenceFailed {
                detail: "journal".into(),
            },
            Unsupported {
                detail: "opcode".into(),
            },
            Unmapped {
                detail: "address".into(),
            },
            Exception { vector: 13 },
            ConfigFailed {
                detail: "config".into(),
            },
            PolicyFailed {
                detail: "budget".into(),
            },
            ObservationFailed {
                detail: "route".into(),
            },
        ] {
            assert!(bios_stop(&stop).1);
        }
    }
}
