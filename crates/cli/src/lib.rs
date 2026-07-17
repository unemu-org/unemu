// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine assembly for the frontend (§3.1).
//!
//! The `cli` crate is split lib + bin so the golden integration tests can call
//! the same paths the binary uses. Two machines are assembled here:
//!
//! - [`run_serial`]: a Long64 blob on the [`SerialChipset`] (one 16550 UART) —
//!   the minimal serial-output path.
//! - [`run_boot`]: a **real-mode** blob on the [`AtChipset`] (UART + 8259 PIC +
//!   8253 PIT) that programs the timer, takes IRQ0, and prints to serial.
//!
//! Both return the bytes the guest sent to COM1, reconstructed through the
//! first-class tracing seam (§3.2), never a concrete-device downcast.

use anyhow::{Context, Result};
use bus::Bus;
use chipset::{AtChipset, Chipset, SerialChipset, AT_UART_INDEX, COM1_BASE};
use cpu::x86jit_backend::RealSegments;
use cpu::X86JitCpu;
use device::{AccessDir, TraceEvent};
use machine::{IrqLine, Machine};

/// Guest RAM size for the flat address space (64 KiB).
pub const RAM_BYTES: u64 = 0x1_0000;
/// Entry point the Long64 serial blob is loaded at and RIP is set to.
pub const ENTRY: u64 = 0x1000;
/// Cooperative run quantum in `x86jit` block units (§6.2). The built-in demos are
/// short blocks, so any budget completes them; `None` runs to a non-budget stop.
pub const QUANTUM: Option<u64> = None;

/// The built-in serial demo: write "hello" to COM1 THR via `out dx, al`, then
/// `hlt`.
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

/// Load segment/IP for the real-mode boot blob (classic boot address, CS = 0).
///
/// The blob runs from segment `0x0000`, IP `0x7C00`, so its linear load address
/// is `0x7C00`. All four segment registers start at `0x0000` (flat low memory);
/// the handler and the IVT it programs therefore live in the same segment.
pub const BOOT_CS: u16 = 0x0000;
/// Real-mode boot IP (linear load address, since CS = 0).
pub const BOOT_IP: u16 = 0x7C00;

/// Consecutive halted boundaries with no interrupt before the boot machine
/// declares the timer quiescent and stops (§5). The boot guest masks IRQ0 after
/// the fifth marker; from then on no serviceable interrupt appears, so the
/// machine gives up after this many idle halts. Comfortably longer than one PIT
/// period (divisor 20 → ~10 halted `jmp;hlt` iterations per period) so a live
/// timer is never mistaken for a dead one.
pub const BOOT_IDLE_LIMIT: u64 = 200;

/// Linear address of the IRQ0 handler (= its offset, since CS = 0). It is poked
/// into IVT entry 8 by the boot program; it must match the handler's position in
/// [`BOOT_PROGRAM`].
pub const BOOT_HANDLER_OFFSET: u16 = 0x7C30;

/// The built-in real-mode boot demo (§5, §7).
///
/// A hand-encoded 16-bit program (no assembler). The embedder seeds the segment
/// registers and the stack (the `x86jit` core does not lift `mov Sreg, r16`), so
/// the program starts straight at device setup: it writes an IRQ0 handler address
/// into IVT entry 8, programs the 8259 PIC (ICW1–ICW4, vector base `0x08`, IRQ0
/// unmasked), programs PIT counter 0 (mode 2, divisor 20), `sti`, then loops on
/// `hlt`. The IRQ0 handler emits a marker byte `.` to COM1, and after five
/// markers masks all IRQs so the machine falls quiescent and terminates. Every
/// instruction's bytes are documented in the `boot_golden` integration test.
#[rustfmt::skip]
pub const BOOT_PROGRAM: &[u8] = &[
    // ---- main program @ 0x7C00 (segments + SP seeded by the embedder) ----
    0xC7, 0x06, 0x20, 0x00, 0x30, 0x7C, // mov word [0x0020],0x7C30  (IVT8 IP)
    0xC7, 0x06, 0x22, 0x00, 0x00, 0x00, // mov word [0x0022],0x0000  (IVT8 CS)
    0xB0, 0x11, 0xE6, 0x20,             // mov al,0x11 ; out 0x20,al  (ICW1)
    0xB0, 0x08, 0xE6, 0x21,             // mov al,0x08 ; out 0x21,al  (ICW2)
    0xB0, 0x04, 0xE6, 0x21,             // mov al,0x04 ; out 0x21,al  (ICW3)
    0xB0, 0x01, 0xE6, 0x21,             // mov al,0x01 ; out 0x21,al  (ICW4)
    0xB0, 0xFE, 0xE6, 0x21,             // mov al,0xFE ; out 0x21,al  (IMR: only IRQ0)
    0xB0, 0x34, 0xE6, 0x43,             // mov al,0x34 ; out 0x43,al  (PIT cmd)
    0xB0, 0x14, 0xE6, 0x40,             // mov al,0x14 ; out 0x40,al  (div lo = 20)
    0xB0, 0x00, 0xE6, 0x40,             // mov al,0x00 ; out 0x40,al  (div hi = 0)
    0xFB,                               // sti
    0xF4,                               // hlt          <- loop target @ 0x7C2D
    0xEB, 0xFD,                         // jmp $-3 (back to hlt)
    // ---- IRQ0 handler @ 0x7C30 ----
    0x50,                               // push ax
    0x52,                               // push dx
    0xFE, 0x06, 0x00, 0x05,             // inc byte [0x0500]        (++counter)
    0xB0, 0x2E,                         // mov al,0x2E ('.')
    0xBA, 0xF8, 0x03,                   // mov dx,0x03F8            (COM1 THR)
    0xEE,                               // out dx,al                (transmit)
    0x80, 0x3E, 0x00, 0x05, 0x05,       // cmp byte [0x0500],0x05
    0x75, 0x04,                         // jne +4                   (skip mask)
    0xB0, 0xFF, 0xE6, 0x21,             // mov al,0xFF ; out 0x21,al (mask all IRQs)
    0xB0, 0x20, 0xE6, 0x20,             // mov al,0x20 ; out 0x20,al (EOI)
    0x5A,                               // pop dx
    0x58,                               // pop ax
    0xCF,                               // iret
];

/// Assemble the serial machine, run `blob` in Long64, and return the bytes the
/// guest sent to COM1 (§3.1).
pub fn run_serial(blob: &[u8]) -> Result<Vec<u8>> {
    let (devices, map, pic) = SerialChipset::new().build();
    let bus = Bus::new(devices, map);

    let cpu = X86JitCpu::new(RAM_BYTES, ENTRY, blob).context("building cpu")?;
    // No interrupt wiring: `None` idle limit terminates on the guest's `hlt`.
    let mut machine = Machine::new(cpu, bus, pic, QUANTUM, Vec::new(), None);

    machine.run().context("running machine")?;

    Ok(drain_serial(&mut machine, AT_UART_INDEX))
}

/// Assemble the PC/AT machine, run `blob` in real mode, and return the bytes the
/// guest sent to COM1 (§5, §7).
///
/// The PIT counter-0 OUT line (bus device index `2`) is wired to PIC input IRQ0;
/// the machine edge-detects it and acknowledges/injects the timer vector.
pub fn run_boot(blob: &[u8]) -> Result<Vec<u8>> {
    let (devices, map, pic) = AtChipset::new().build();
    let bus = Bus::new(devices, map);

    let cpu = X86JitCpu::new_real16(
        RAM_BYTES,
        BOOT_CS,
        BOOT_IP,
        blob,
        RealSegments::flat(BOOT_CS),
    )
    .context("building real-mode cpu")?;

    // Wire PIT (device index 2) counter-0 OUT to PIC IRQ0.
    let irq_lines = vec![IrqLine { device: 2, irq: 0 }];
    let mut machine = Machine::new(cpu, bus, pic, QUANTUM, irq_lines, Some(BOOT_IDLE_LIMIT));

    machine.run().context("running machine")?;

    Ok(drain_serial(&mut machine, AT_UART_INDEX))
}

/// Reconstruct the COM1 byte stream from the UART's trace (§3.2 tracing seam).
///
/// The machine core sees only `&dyn Device`, so the transmitted bytes are read
/// back through the tracing seam: each THR write to `COM1_BASE` surfaces as a
/// `TraceEvent::Port { dir: Write, port: COM1_BASE, .. }`.
fn drain_serial(machine: &mut Machine<X86JitCpu>, uart_index: usize) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(dev) = machine.bus_mut().device_mut(uart_index) {
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
