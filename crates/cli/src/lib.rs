// SPDX-License-Identifier: AGPL-3.0-or-later
//! Serial-machine assembly for the frontend (§3.1).
//!
//! The `cli` crate is split lib + bin so the golden integration test can call
//! the same [`run_serial`] path the binary uses. [`run_serial`] builds a
//! [`Machine`] from the [`SerialChipset`], runs a flat Long64 blob to `hlt`, and
//! returns the bytes the guest sent to COM1 — reconstructed through the
//! first-class tracing seam (§3.2), never a concrete-device downcast.

use anyhow::{Context, Result};
use bus::Bus;
use chipset::{Chipset, SerialChipset, COM1_BASE};
use cpu::X86JitCpu;
use device::{AccessDir, TraceEvent};
use machine::Machine;

/// Guest RAM size for the flat address space (64 KiB).
pub const RAM_BYTES: u64 = 0x1_0000;
/// Entry point the blob is loaded at and RIP is set to.
pub const ENTRY: u64 = 0x1000;
/// Cooperative run quantum in `x86jit` block units (§6.2). The built-in demo is
/// a single straight-line block, so any budget completes it; `None` runs to a
/// non-budget stop.
pub const QUANTUM: Option<u64> = None;

/// The built-in demo: write "hello" to COM1 THR via `out dx, al`, then `hlt`.
///
/// Hand-encoded Long64 machine code (verified against the iced-x86 disassembler
/// in `x86jit-core`). Byte-for-byte:
///
/// ```text
/// 66 BA F8 03   mov dx, 0x3F8   ; COM1 THR port into DX
/// B0 68         mov al, 'h'     ; 0x68
/// EE            out dx, al
/// B0 65         mov al, 'e'     ; 0x65
/// EE            out dx, al
/// B0 6C         mov al, 'l'     ; 0x6C
/// EE            out dx, al
/// B0 6C         mov al, 'l'     ; 0x6C
/// EE            out dx, al
/// B0 6F         mov al, 'o'     ; 0x6F
/// EE            out dx, al
/// F4            hlt
/// ```
#[rustfmt::skip]
pub const HELLO_PROGRAM: &[u8] = &[
    0x66, 0xBA, 0xF8, 0x03,
    0xB0, b'h', 0xEE,
    0xB0, b'e', 0xEE,
    0xB0, b'l', 0xEE,
    0xB0, b'l', 0xEE,
    0xB0, b'o', 0xEE,
    0xF4,
];

/// Assemble the serial machine, run `blob`, and return the bytes the guest
/// sent to COM1 (§3.1).
pub fn run_serial(blob: &[u8]) -> Result<Vec<u8>> {
    let (devices, map) = SerialChipset::new().build();
    let bus = Bus::new(devices, map);

    let cpu = X86JitCpu::new(RAM_BYTES, ENTRY, blob).context("building cpu")?;
    let mut machine = Machine::new(cpu, bus, QUANTUM);

    machine.run().context("running machine")?;

    Ok(drain_serial(&mut machine))
}

/// Reconstruct the COM1 byte stream from the UART's trace (§3.2 tracing seam).
///
/// The machine core sees only `&dyn Device`, so the transmitted bytes are read
/// back through the tracing seam: each THR write to `COM1_BASE` surfaces as a
/// `TraceEvent::Port { dir: Write, port: COM1_BASE, .. }`.
fn drain_serial(machine: &mut Machine<X86JitCpu>) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(dev) = machine.bus_mut().device_mut(0) {
        for event in dev.take_trace() {
            if let TraceEvent::Port {
                port,
                dir: AccessDir::Write,
                value,
                ..
            } = event
            {
                if port == COM1_BASE {
                    out.push(value as u8);
                }
            }
        }
    }
    out
}
