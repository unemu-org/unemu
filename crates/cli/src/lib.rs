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
//! - [`run_rtc`]: a **real-mode** blob on the [`AtChipset`] that programs the
//!   cascaded PIC pair and the MC146818 RTC, takes IRQ8 across the slave→master
//!   cascade, and prints to serial.
//!
//! They return the bytes the guest sent to COM1, reconstructed through the
//! first-class tracing seam (§3.2), never a concrete-device downcast.

use anyhow::{Context, Result};
use bus::Bus;
use chipset::{
    AtChipset, Chipset, SerialChipset, AT_CMOS_RTC_INDEX, AT_I8042_INDEX, AT_UART_INDEX,
    AT_VGA_INDEX, COM1_BASE,
};
use cpu::x86jit_backend::RealSegments;
use cpu::X86JitCpu;
use device::{AccessDir, TraceEvent};
use devices::vga_text::{
    COLS, CRTC_CURSOR_HIGH_INDEX, CRTC_CURSOR_LOW_INDEX, CRTC_CURSOR_START_INDEX, CRTC_DATA_PORT,
    CRTC_INDEX_PORT, CRTC_START_HIGH_INDEX, CRTC_START_LOW_INDEX, ROWS, TEXT_BUFFER_BASE,
    TEXT_BUFFER_LEN,
};
use machine::{IrqLine, Machine};
use video::{render_text_mode, CaptureFrameSink, Frame, FrameSink, TextGrid, TextModeState};

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

/// Linear address of the IRQ1 (keyboard) handler in [`KBD_PROGRAM`]. It is poked
/// into IVT entry 9 by the main program (CS = 0, so the linear address equals the
/// offset); it must match the handler's position after the 44-byte main program.
pub const KBD_HANDLER_OFFSET: u16 = 0x7C00 + 44;

/// The built-in real-mode keyboard-interrupt demo (§5, §7).
///
/// A hand-encoded 16-bit program (no assembler). The embedder seeds the segment
/// registers and stack (the `x86jit` core does not lift `mov Sreg, r16`), so the
/// program starts straight at device setup: it writes an IRQ1 handler address
/// into IVT entry 9, programs the 8259 PIC (ICW1–ICW4, vector base `0x08`, IRQ1
/// unmasked), enables keyboard interrupts in the 8042 (command byte bit 0),
/// `sti`, then loops on `hlt`. A scancode queued into the keyboard before the run
/// raises IRQ1 once interrupts are enabled; the handler reads it from port `0x60`
/// and emits it to COM1, then masks all IRQs so the machine falls quiescent and
/// terminates. Every instruction's bytes are documented in the `keyboard_golden`
/// integration test.
#[rustfmt::skip]
pub const KBD_PROGRAM: &[u8] = &[
    // ---- main program @ 0x7C00 (segments + SP seeded by the embedder) ----
    0xC7, 0x06, 0x24, 0x00, 0x2C, 0x7C, // mov word [0x0024],0x7C2C  (IVT9 IP)
    0xC7, 0x06, 0x26, 0x00, 0x00, 0x00, // mov word [0x0026],0x0000  (IVT9 CS)
    0xB0, 0x11, 0xE6, 0x20,             // mov al,0x11 ; out 0x20,al  (ICW1)
    0xB0, 0x08, 0xE6, 0x21,             // mov al,0x08 ; out 0x21,al  (ICW2: base 0x08)
    0xB0, 0x04, 0xE6, 0x21,             // mov al,0x04 ; out 0x21,al  (ICW3)
    0xB0, 0x01, 0xE6, 0x21,             // mov al,0x01 ; out 0x21,al  (ICW4)
    0xB0, 0xFD, 0xE6, 0x21,             // mov al,0xFD ; out 0x21,al  (IMR: only IRQ1)
    0xB0, 0x60, 0xE6, 0x64,             // mov al,0x60 ; out 0x64,al  (8042: write cmd byte)
    0xB0, 0x01, 0xE6, 0x60,             // mov al,0x01 ; out 0x60,al  (cmd byte: KBD int on)
    0xFB,                               // sti
    0xF4,                               // hlt          <- loop target @ 0x7C29
    0xEB, 0xFD,                         // jmp $-3 (back to hlt)
    // ---- IRQ1 handler @ 0x7C2C ----
    0x50,                               // push ax
    0x52,                               // push dx
    0xBA, 0x60, 0x00,                   // mov dx,0x0060            (keyboard data port)
    0xEC,                               // in al,dx                 (read scancode)
    0xBA, 0xF8, 0x03,                   // mov dx,0x03F8            (COM1 THR)
    0xEE,                               // out dx,al                (transmit scancode)
    0xB0, 0xFF, 0xE6, 0x21,             // mov al,0xFF ; out 0x21,al (mask all IRQs)
    0xB0, 0x20, 0xE6, 0x20,             // mov al,0x20 ; out 0x20,al (EOI)
    0x5A,                               // pop dx
    0x58,                               // pop ax
    0xCF,                               // iret
];

/// The scancode the built-in keyboard demo queues (make code for `A`). The demo's
/// serial output is exactly this byte, emitted by the IRQ1 handler.
pub const KBD_SCANCODE: u8 = 0x1E;

/// Consecutive halted boundaries with no interrupt before the keyboard machine
/// declares the source quiescent and stops (§5). After the single keystroke the
/// handler masks all IRQs, so no further interrupt appears and the machine gives
/// up after this many idle halts.
pub const KBD_IDLE_LIMIT: u64 = 200;

/// Linear address of the IRQ8 (RTC) handler in [`RTC_PROGRAM`]. It is poked into
/// IVT entry `0x70` by the main program (CS = 0, so the linear address equals the
/// offset); it must match the handler's position after the 64-byte main program.
pub const RTC_HANDLER_OFFSET: u16 = 0x7C00 + 64;

/// The marker byte the RTC demo's IRQ8 handler transmits to COM1 (`'R'`). The
/// demo's serial output is exactly this one byte.
pub const RTC_MARKER: u8 = b'R';

/// Virtual clocks per RTC second for the demo. The real ratio is over a million
/// clocks; a bounded headless run never advances that far, so the RTC is built
/// with this small ratio (via [`AtChipset::with_rtc_timing`]) and the
/// update-ended interrupt fires a few dozen boundaries into the `hlt` loop. It is
/// still virtual time (retired-instruction-derived), so the run is deterministic.
pub const RTC_SECOND_TICKS: u64 = 64;
/// Update-in-progress window for the demo RTC, in virtual clocks.
pub const RTC_UIP_TICKS: u64 = 8;

/// Consecutive halted boundaries with no interrupt before the RTC machine
/// declares the source quiescent and stops (§5). After the single update-ended
/// interrupt the handler masks the master PIC (the cascade line included), so no
/// further vector is serviceable and the machine gives up after this many idle
/// halts. Comfortably longer than one RTC second so a live clock is never
/// mistaken for a dead one before it first fires.
pub const RTC_IDLE_LIMIT: u64 = 200;

/// The built-in real-mode RTC update-ended interrupt demo (§5, §7).
///
/// A hand-encoded 16-bit program (no assembler). The embedder seeds the segment
/// registers and stack (the `x86jit` core does not lift `mov Sreg, r16`), so the
/// program starts straight at device setup: it writes an IRQ8 handler address
/// into IVT entry `0x70` (`0x0000:0x01C0`), programs the master 8259 (ICW1–ICW4,
/// vector base `0x08`, IRQ2 unmasked for the cascade) and the slave 8259 (ICW1–
/// ICW4, vector base `0x70`, IRQ8 unmasked), enables the RTC's update-ended
/// interrupt (register B bit 4, UIE), `sti`, then loops on `hlt`. Once a full RTC
/// second ticks the RTC raises IRQ8; the machine edge-detects it onto the slave
/// PIC, which cascades into the master at IRQ2, and injects vector `0x70`. The
/// handler emits a marker byte `R` to COM1, ends the cascaded interrupt with an
/// EOI to **both** the slave (`0xA0`) and the master (`0x20`), reads register C of
/// the RTC to deassert IRQ8, masks all master IRQs so the source falls quiescent,
/// and `iret`s. Every instruction's bytes are documented in the `rtc_golden`
/// integration test.
#[rustfmt::skip]
pub const RTC_PROGRAM: &[u8] = &[
    // ---- main program @ 0x7C00 (segments + SP seeded by the embedder) ----
    0xC7, 0x06, 0xC0, 0x01, 0x40, 0x7C, // mov word [0x01C0],0x7C40  (IVT0x70 IP)
    0xC7, 0x06, 0xC2, 0x01, 0x00, 0x00, // mov word [0x01C2],0x0000  (IVT0x70 CS)
    0xB0, 0x11, 0xE6, 0x20,             // mov al,0x11 ; out 0x20,al  (master ICW1)
    0xB0, 0x08, 0xE6, 0x21,             // mov al,0x08 ; out 0x21,al  (master ICW2: base 0x08)
    0xB0, 0x04, 0xE6, 0x21,             // mov al,0x04 ; out 0x21,al  (master ICW3: slave on IRQ2)
    0xB0, 0x01, 0xE6, 0x21,             // mov al,0x01 ; out 0x21,al  (master ICW4: 8086)
    0xB0, 0xFB, 0xE6, 0x21,             // mov al,0xFB ; out 0x21,al  (master IMR: unmask IRQ2)
    0xB0, 0x11, 0xE6, 0xA0,             // mov al,0x11 ; out 0xA0,al  (slave ICW1)
    0xB0, 0x70, 0xE6, 0xA1,             // mov al,0x70 ; out 0xA1,al  (slave ICW2: base 0x70)
    0xB0, 0x02, 0xE6, 0xA1,             // mov al,0x02 ; out 0xA1,al  (slave ICW3: identity 2)
    0xB0, 0x01, 0xE6, 0xA1,             // mov al,0x01 ; out 0xA1,al  (slave ICW4: 8086)
    0xB0, 0xFE, 0xE6, 0xA1,             // mov al,0xFE ; out 0xA1,al  (slave IMR: unmask IRQ8)
    0xB0, 0x0B, 0xE6, 0x70,             // mov al,0x0B ; out 0x70,al  (select RTC reg B)
    0xB0, 0x12, 0xE6, 0x71,             // mov al,0x12 ; out 0x71,al  (reg B = 24h | UIE)
    0xFB,                               // sti
    0xF4,                               // hlt          <- loop target @ 0x7C3D
    0xEB, 0xFD,                         // jmp $-3 (back to hlt)
    // ---- IRQ8 handler @ 0x7C40 ----
    0x50,                               // push ax
    0x52,                               // push dx
    0xB0, 0x52,                         // mov al,0x52 ('R')
    0xBA, 0xF8, 0x03,                   // mov dx,0x03F8            (COM1 THR)
    0xEE,                               // out dx,al                (transmit marker)
    0xB0, 0x20, 0xE6, 0xA0,             // mov al,0x20 ; out 0xA0,al (EOI slave)
    0xB0, 0x20, 0xE6, 0x20,             // mov al,0x20 ; out 0x20,al (EOI master)
    0xB0, 0x0C, 0xE6, 0x70,             // mov al,0x0C ; out 0x70,al (select RTC reg C)
    0xBA, 0x71, 0x00,                   // mov dx,0x0071            (RTC data port)
    0xEC,                               // in al,dx                 (read reg C -> deassert IRQ8)
    0xB0, 0xFF, 0xE6, 0x21,             // mov al,0xFF ; out 0x21,al (mask all master IRQs)
    0x5A,                               // pop dx
    0x58,                               // pop ax
    0xCF,                               // iret
];

/// Segment the VGA text demo points DS/ES at so a near store reaches the text
/// buffer: `0xB800 << 4 == 0xB8000`, the base of the colour text page.
pub const VGA_TEXT_SEGMENT: u16 = 0xB800;

/// The built-in real-mode VGA text demo (§9.2).
///
/// A hand-encoded 16-bit program (no assembler). The embedder seeds DS/ES to
/// `0xB800` so a near word store lands in the text buffer at `0xB8000`; the demo
/// writes two glyph+attribute cells ("Hi", bright white on black) into cells 0
/// and 1, then `hlt`s. It takes no interrupts. Every instruction's bytes are
/// documented in the `vga_text_golden` integration test.
#[rustfmt::skip]
pub const VGA_TEXT_PROGRAM: &[u8] = &[
    0xB8, b'H', 0x1F,       // mov ax,0x1F48   ('H', attr 0x1F)
    0x89, 0x06, 0x00, 0x00, // mov [0x0000],ax  -> cell 0 @ 0xB8000
    0xB8, b'i', 0x1F,       // mov ax,0x1F69   ('i', attr 0x1F)
    0x89, 0x06, 0x02, 0x00, // mov [0x0002],ax  -> cell 1 @ 0xB8002
    0xF4,                   // hlt
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

/// Assemble the PC/AT machine with `scancodes` queued in the keyboard, run `blob`
/// in real mode, and return the bytes the guest sent to COM1 (§5, §7).
///
/// The 8042's keyboard-byte-ready line (bus device index [`AT_I8042_INDEX`]) is
/// wired to PIC input IRQ1; the machine edge-detects it and acknowledges/injects
/// the keyboard vector once the guest enables keyboard interrupts and sets `IF`.
pub fn run_keyboard(blob: &[u8], scancodes: &[u8]) -> Result<Vec<u8>> {
    let (devices, map, pic) = AtChipset::with_keyboard_input(scancodes).build();
    let bus = Bus::new(devices, map);

    let cpu = X86JitCpu::new_real16(
        RAM_BYTES,
        BOOT_CS,
        BOOT_IP,
        blob,
        RealSegments::flat(BOOT_CS),
    )
    .context("building real-mode cpu")?;

    // Wire the keyboard controller (device index 3) byte-ready line to PIC IRQ1.
    let irq_lines = vec![IrqLine {
        device: AT_I8042_INDEX,
        irq: 1,
    }];
    let mut machine = Machine::new(cpu, bus, pic, QUANTUM, irq_lines, Some(KBD_IDLE_LIMIT));

    machine.run().context("running machine")?;

    Ok(drain_serial(&mut machine, AT_UART_INDEX))
}

/// Assemble the PC/AT machine with a fast-ticking RTC, run `blob` in real mode,
/// and return the bytes the guest sent to COM1 (§5, §7).
///
/// The RTC's update-ended output ([`AT_CMOS_RTC_INDEX`]) is wired to PIC input
/// IRQ8, which lives on the *slave* 8259; the machine edge-detects it onto the
/// cascaded pair, where it surfaces to the CPU through the master's IRQ2 cascade.
/// This is the first path where an interrupt reaches the CPU across the cascade.
/// The RTC is built with a small per-second tick ratio ([`RTC_SECOND_TICKS`]) so
/// the update-ended interrupt fires within a bounded run; the timing is virtual,
/// so the output stays deterministic.
pub fn run_rtc(blob: &[u8]) -> Result<Vec<u8>> {
    let (devices, map, pic) = AtChipset::with_rtc_timing(RTC_SECOND_TICKS, RTC_UIP_TICKS).build();
    let bus = Bus::new(devices, map);

    let cpu = X86JitCpu::new_real16(
        RAM_BYTES,
        BOOT_CS,
        BOOT_IP,
        blob,
        RealSegments::flat(BOOT_CS),
    )
    .context("building real-mode cpu")?;

    // Wire the RTC (device index 6) update-ended line to PIC IRQ8 (slave line 0).
    let irq_lines = vec![IrqLine {
        device: AT_CMOS_RTC_INDEX,
        irq: 8,
    }];
    let mut machine = Machine::new(cpu, bus, pic, QUANTUM, irq_lines, Some(RTC_IDLE_LIMIT));

    machine.run().context("running machine")?;

    Ok(drain_serial(&mut machine, AT_UART_INDEX))
}

/// Assemble the PC/AT machine, run `blob` in real mode, then render the VGA text
/// card into a [`TextGrid`] and present it to a [`CaptureFrameSink`] (§9.2).
///
/// DS/ES are seeded to [`VGA_TEXT_SEGMENT`] so the guest's near stores reach the
/// text buffer, and the `0xB8000` window is mapped as trapped MMIO so those
/// stores surface on the bus and reach the card. Nothing runs a wall clock, so
/// the produced grid is deterministic.
pub fn run_vga_text(blob: &[u8]) -> Result<TextGrid> {
    let (devices, map, pic) = AtChipset::new().build();
    let bus = Bus::new(devices, map);

    // The text buffer at 0xB8000 sits above low RAM, so the flat guest space must
    // extend past it while RAM stops at its base — a Trap window may not overlap
    // RAM. RAM below the window is ample for the guest's code and stack.
    let mut cpu = X86JitCpu::new_real16_mmio(
        TEXT_BUFFER_BASE + TEXT_BUFFER_LEN as u64,
        TEXT_BUFFER_BASE,
        BOOT_CS,
        BOOT_IP,
        blob,
        RealSegments {
            ds: VGA_TEXT_SEGMENT,
            es: VGA_TEXT_SEGMENT,
            ss: BOOT_CS,
            sp: RealSegments::DEFAULT_SP,
        },
    )
    .context("building real-mode cpu")?;
    // Trap the colour text page so stores into it surface as MMIO on the bus.
    cpu.map_mmio(TEXT_BUFFER_BASE, TEXT_BUFFER_LEN as u64)
        .context("mapping VGA text MMIO window")?;

    // No interrupt wiring: `None` idle limit terminates on the guest's `hlt`.
    let mut machine = Machine::new(cpu, bus, pic, QUANTUM, Vec::new(), None);

    machine.run().context("running machine")?;

    let grid = render_vga(&mut machine, AT_VGA_INDEX);
    // Exercise the presentation seam: hand the frame to a capture sink and read
    // it back, exactly as a terminal or windowed presenter would receive it.
    let mut sink = CaptureFrameSink::new();
    sink.present(&Frame::Text(grid));
    Ok(sink
        .last_text()
        .cloned()
        .expect("capture sink holds the presented text frame"))
}

/// Reconstruct a [`TextGrid`] from the VGA card's trace (§3.2 tracing seam, §9.2).
///
/// The machine core sees only `&dyn Device`, so the card's memory and CRTC state
/// are read back through the same tracing seam the serial drain uses — never a
/// concrete-device downcast. Every MMIO write into the text window is replayed
/// into a fresh buffer (the card powers on cleared, so unwritten cells stay
/// blank), and every CRTC port write is replayed to recover the display start
/// address and cursor, which [`render_text_mode`] then resolves into the grid.
fn render_vga(machine: &mut Machine<X86JitCpu>, vga_index: usize) -> TextGrid {
    let mut vram = vec![0u8; TEXT_BUFFER_LEN];
    // Mirror the card's CRTC register file. Indexed by the guest-written index
    // byte (0..=0xFF); only the display-start and cursor registers are read back.
    let mut crtc = [0u8; 0x100];
    let mut crtc_index: u8 = 0;

    if let Some(dev) = machine.bus_mut().device_mut(vga_index) {
        for event in dev.take_trace() {
            match event {
                TraceEvent::Mmio {
                    addr,
                    size,
                    dir: AccessDir::Write,
                    value,
                    ..
                } => {
                    let offset = (addr - TEXT_BUFFER_BASE) as usize;
                    for i in 0..usize::from(size) {
                        if let Some(slot) = vram.get_mut(offset + i) {
                            *slot = (value >> (8 * i)) as u8;
                        }
                    }
                }
                TraceEvent::Port {
                    port,
                    dir: AccessDir::Write,
                    value,
                    ..
                } => {
                    if port == CRTC_INDEX_PORT {
                        crtc_index = value as u8;
                    } else if port == CRTC_DATA_PORT {
                        crtc[usize::from(crtc_index)] = value as u8;
                    }
                }
                _ => {}
            }
        }
    }

    let reg = |index: u8| crtc[usize::from(index)];
    let start_cell =
        u16::from(reg(CRTC_START_HIGH_INDEX)) << 8 | u16::from(reg(CRTC_START_LOW_INDEX));
    let cursor_cell =
        u16::from(reg(CRTC_CURSOR_HIGH_INDEX)) << 8 | u16::from(reg(CRTC_CURSOR_LOW_INDEX));
    // Cursor-start bit 5 disables the cursor; clear means visible.
    let cursor_visible = reg(CRTC_CURSOR_START_INDEX) & 0x20 == 0;

    render_text_mode(&TextModeState {
        vram: &vram,
        cols: COLS,
        rows: ROWS,
        start_cell,
        cursor_cell,
        cursor_visible,
    })
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
