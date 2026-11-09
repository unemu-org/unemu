// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine runners for the frontend (§3.1).
//!
//! The `cli` crate is split lib + bin so the golden integration tests can call
//! the same paths the binary uses. Every PC-family runner here now builds a
//! declarative [`assembly::MachineConfig`] from a preset, realises it with
//! [`assembly::assemble`], and drives the resulting machine — addressing devices
//! by role through the assembler's device-index map, never a hardcoded bus index.
//!
//! The lone exception is [`run_serial`]: the Long64 serial skeleton is one 16550
//! UART assembled inline (trait-free), not through a config preset — a declarative
//! mode axis for a single Long64 user would not earn its keep.
//!
//! Runners return their observables (COM1 bytes, POST stream, text screens)
//! through the UART's transmitted-output sink for COM1 and the first-class
//! tracing seam (§3.2) for POST and text screens.

use anyhow::{Context, Result};
use assembly::{
    assemble, AssembledMachine, ConsoleProbe, DeviceId, InjectionSchedule, KeyboardStimulus,
    MachineConfig, RunPolicy, StopCondition,
};
use bus::{Bus, DecodeMap, PortRange};
use chipset::{NoInterruptController, COM1_BASE, COM1_TOP};
use cpu::{Cpu, CpuError, CpuReg, X86JitCpu};
use device::{AccessDir, Device, TraceEvent};
use devices::post_port::POST_PORT;
use devices::upd765::FLOPPY_IMAGE_BYTES;
use devices::vga_text::{
    COLS, CRTC_CURSOR_HIGH_INDEX, CRTC_CURSOR_LOW_INDEX, CRTC_CURSOR_START_INDEX, CRTC_DATA_PORT,
    CRTC_INDEX_PORT, CRTC_START_HIGH_INDEX, CRTC_START_LOW_INDEX, ROWS, TEXT_BUFFER_BASE,
    TEXT_BUFFER_LEN,
};
use devices::{Uart16550, I8042};
use machine::{Machine, MachineError, RunOutcome};
use std::collections::HashMap;
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
/// with this small ratio (the `at_rtc` preset's fast-tick timing) and the
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

/// Guest-physical base of the write demo's source buffer: the distinctive sector
/// the guest hands to the DMA controller for the floppy WRITE lives here, preloaded
/// by the preset. Below the read-back buffer and the program, all within 64 KiB.
pub const FDC_WRITE_SRC: u64 = 0x0600;

/// Guest-physical base of the write demo's read-back buffer: the READ DATA that
/// follows the write streams the just-written sector here, where the guest then
/// compares it against the pattern. Starts as cleared RAM (zeroes).
pub const FDC_WRITE_DST: u16 = 0x0A00;

/// The marker byte the write demo emits to COM1 when the sector it read back
/// matches the sector it wrote (`'W'`). The demo's serial output is exactly this
/// one byte; an empty capture means the read-back did not match the write.
pub const FDC_WRITE_MARKER: u8 = b'W';

/// Total-retired stop for the write demo. The guest writes, reads back, emits its
/// marker, and then spins in place (`jmp $`) with no interrupt source wired; with
/// nothing to wake it the run reaches this cap and stops (a clean
/// [`RunOutcome::RetiredCapReached`], as the busy-polling floppy-boot guest also
/// does). It is far above the ~150 instructions the guest retires before the
/// marker, so the marker always emits.
pub const FDC_WRITE_RETIRED_CAP: u64 = 5_000;

/// The distinctive 512-byte sector the write demo commits to the blank floppy.
///
/// Preloaded into guest RAM at [`FDC_WRITE_SRC`] and, after the write→read cycle,
/// expected byte-for-byte in the read-back buffer. Byte `i` is `0xC0 ^ i`, so the
/// first four bytes are `C0 C1 C2 C3` — the immediates [`FDC_WRITE_PROGRAM`]
/// compares the read-back buffer against. Every byte of the compared prefix is
/// non-zero, so it cannot be spoofed by the blank (all-zero) floppy: only an actual
/// write of this pattern can make the read-back match.
#[must_use]
pub fn fdc_write_pattern() -> Vec<u8> {
    (0..512u32).map(|i| 0xC0u8 ^ i as u8).collect()
}

/// The built-in real-mode floppy WRITE demo (§7, §7.7, §7.8).
///
/// A hand-encoded 16-bit program (no assembler). The embedder seeds the segment
/// registers and stack (the `x86jit` core does not lift `mov Sreg, r16`), so the
/// program starts straight at device setup. With a blank writable floppy in drive
/// 0 it: brings the µPD765 out of reset (DOR: not-reset + DMA-enable + motor), then
/// programs the 8237 channel 2 for a memory→device transfer of one 512-byte sector
/// from [`FDC_WRITE_SRC`] and issues WRITE DATA (drive 0, C=0/H=0/R=1). The machine
/// services the DMA on the boundary of the last parameter byte, so the sector is
/// committed and the result phase is ready by the guest's next instruction; the
/// guest drains the seven result bytes. It then programs channel 2 the other way
/// (device→memory) into [`FDC_WRITE_DST`] and issues READ DATA for the same CHS,
/// pulling the just-written sector back. Finally it compares the first four
/// read-back bytes against the pattern and, on a full match, emits [`FDC_WRITE_MARKER`]
/// to COM1, then busy-spins on a port read (`in al,dx ; jmp $-3`). The port read
/// yields to the machine each iteration, so the run stops cleanly at
/// [`FDC_WRITE_RETIRED_CAP`] (the busy-guest idiom) rather than looping forever
/// inside one CPU quantum. Because the floppy starts blank, a match proves the
/// write reached media. Every instruction's bytes are documented in the
/// `fdc_write_golden` integration test.
#[rustfmt::skip]
pub const FDC_WRITE_PROGRAM: &[u8] = &[
    // ---- bring the FDC out of reset (DOR @ 0x3F2) ----
    0xB0, 0x1C,                         // mov al,0x1C            (not-reset|DMA|motor0, drive 0)
    0xBA, 0xF2, 0x03,                   // mov dx,0x03F2          (DOR)
    0xEE,                               // out dx,al
    // ---- program 8237 ch2: memory -> device (read), src 0x0600, 512 bytes ----
    0xB0, 0x06, 0xE6, 0x0A,             // mov al,0x06 ; out 0x0A,al  (mask ch2)
    0xB0, 0x00, 0xE6, 0x0C,             // mov al,0x00 ; out 0x0C,al  (clear flip-flop)
    0xB0, 0x4A, 0xE6, 0x0B,             // mov al,0x4A ; out 0x0B,al  (mode: ch2 read single)
    0xB0, 0x00, 0xE6, 0x04,             // mov al,0x00 ; out 0x04,al  (addr low)
    0xB0, 0x06, 0xE6, 0x04,             // mov al,0x06 ; out 0x04,al  (addr high -> 0x0600)
    0xB0, 0x00, 0xE6, 0x81,             // mov al,0x00 ; out 0x81,al  (page ch2)
    0xB0, 0xFF, 0xE6, 0x05,             // mov al,0xFF ; out 0x05,al  (count low)
    0xB0, 0x01, 0xE6, 0x05,             // mov al,0x01 ; out 0x05,al  (count high -> 512)
    0xB0, 0x02, 0xE6, 0x0A,             // mov al,0x02 ; out 0x0A,al  (unmask ch2)
    // ---- WRITE DATA (0x05): drive 0, C=0 H=0 R=1 N=2 EOT=1 (DATA @ 0x3F5) ----
    0xBA, 0xF5, 0x03,                   // mov dx,0x03F5          (DATA)
    0xB0, 0x05, 0xEE,                   // mov al,0x05 ; out dx,al  (WRITE DATA)
    0xB0, 0x00, 0xEE,                   // mov al,0x00 ; out dx,al  (drive/head)
    0xB0, 0x00, 0xEE,                   // mov al,0x00 ; out dx,al  (cylinder)
    0xB0, 0x00, 0xEE,                   // mov al,0x00 ; out dx,al  (head)
    0xB0, 0x01, 0xEE,                   // mov al,0x01 ; out dx,al  (sector R=1)
    0xB0, 0x02, 0xEE,                   // mov al,0x02 ; out dx,al  (N=2, 512-byte sector)
    0xB0, 0x01, 0xEE,                   // mov al,0x01 ; out dx,al  (EOT=1)
    0xB0, 0x1B, 0xEE,                   // mov al,0x1B ; out dx,al  (GPL)
    0xB0, 0xFF, 0xEE,                   // mov al,0xFF ; out dx,al  (DTL)
    // ---- drain the 7 WRITE result bytes so the controller returns to idle ----
    0xEC, 0xEC, 0xEC, 0xEC, 0xEC, 0xEC, 0xEC, // in al,dx x7
    // ---- program 8237 ch2: device -> memory (write), dst 0x0A00, 512 bytes ----
    0xB0, 0x06, 0xE6, 0x0A,             // mov al,0x06 ; out 0x0A,al  (mask ch2)
    0xB0, 0x00, 0xE6, 0x0C,             // mov al,0x00 ; out 0x0C,al  (clear flip-flop)
    0xB0, 0x46, 0xE6, 0x0B,             // mov al,0x46 ; out 0x0B,al  (mode: ch2 write single)
    0xB0, 0x00, 0xE6, 0x04,             // mov al,0x00 ; out 0x04,al  (addr low)
    0xB0, 0x0A, 0xE6, 0x04,             // mov al,0x0A ; out 0x04,al  (addr high -> 0x0A00)
    0xB0, 0x00, 0xE6, 0x81,             // mov al,0x00 ; out 0x81,al  (page ch2)
    0xB0, 0xFF, 0xE6, 0x05,             // mov al,0xFF ; out 0x05,al  (count low)
    0xB0, 0x01, 0xE6, 0x05,             // mov al,0x01 ; out 0x05,al  (count high -> 512)
    0xB0, 0x02, 0xE6, 0x0A,             // mov al,0x02 ; out 0x0A,al  (unmask ch2)
    // ---- READ DATA (0x06): same CHS, streams the sector into 0x0A00 ----
    0xBA, 0xF5, 0x03,                   // mov dx,0x03F5          (DATA)
    0xB0, 0x06, 0xEE,                   // mov al,0x06 ; out dx,al  (READ DATA)
    0xB0, 0x00, 0xEE,                   // mov al,0x00 ; out dx,al  (drive/head)
    0xB0, 0x00, 0xEE,                   // mov al,0x00 ; out dx,al  (cylinder)
    0xB0, 0x00, 0xEE,                   // mov al,0x00 ; out dx,al  (head)
    0xB0, 0x01, 0xEE,                   // mov al,0x01 ; out dx,al  (sector R=1)
    0xB0, 0x02, 0xEE,                   // mov al,0x02 ; out dx,al  (N=2)
    0xB0, 0x01, 0xEE,                   // mov al,0x01 ; out dx,al  (EOT=1)
    0xB0, 0x1B, 0xEE,                   // mov al,0x1B ; out dx,al  (GPL)
    0xB0, 0xFF, 0xEE,                   // mov al,0xFF ; out dx,al  (DTL)
    // ---- compare read-back [0x0A00..0x0A04] to the pattern, emit 'W' on match ----
    0x80, 0x3E, 0x00, 0x0A, 0xC0,       // cmp byte [0x0A00],0xC0
    0x75, 0x1B,                         // jne end (+27)
    0x80, 0x3E, 0x01, 0x0A, 0xC1,       // cmp byte [0x0A01],0xC1
    0x75, 0x14,                         // jne end (+20)
    0x80, 0x3E, 0x02, 0x0A, 0xC2,       // cmp byte [0x0A02],0xC2
    0x75, 0x0D,                         // jne end (+13)
    0x80, 0x3E, 0x03, 0x0A, 0xC3,       // cmp byte [0x0A03],0xC3
    0x75, 0x06,                         // jne end (+6)
    0xB0, 0x57,                         // mov al,0x57 ('W')
    0xBA, 0xF8, 0x03,                   // mov dx,0x03F8          (COM1 THR)
    0xEE,                               // out dx,al               (transmit marker)
    0xEC,                               // in al,dx     (I/O-yielding spin)  <- end
    0xEB, 0xFD,                         // jmp $-3 (back to the in)
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

/// Flat guest space for the test386 smoke run: the whole 1 MiB is RAM (§3.1).
///
/// The spike maps no ROM region and no unmapped hole — the diagnostic image is
/// loaded into RAM at the top 64 KiB and executed from the reset vector.
pub const TEST386_SPACE: u64 = 0x10_0000;
/// Segment the test386 image is loaded at: the top 64 KiB begins at `0xF0000`.
pub const TEST386_LOAD_SEG: u16 = 0xF000;
/// Reset entry point: `CS:IP = F000:FFF0` → linear `0xFFFF0`, the reset vector's
/// far jump into the diagnostic's POST entry.
pub const TEST386_RESET_IP: u16 = 0xFFF0;
/// Cooperative run quantum for the test386 machine, in `x86jit` block units (§6.2).
///
/// A finite quantum (unlike the built-in demos' `None`) so a guest that spins
/// without any port I/O or halt still returns to the machine loop periodically,
/// where the retired-instruction cap can stop it. The value only bounds how often
/// the cap is checked; it does not affect a run that reaches a natural stop.
pub const TEST386_QUANTUM: Option<u64> = Some(65_536);
/// Total retired-instruction safety cap for the test386 smoke run (§6).
///
/// A generous bound: the diagnostic is expected to stop early (an instruction the
/// real-mode core does not lift), long before this. It exists only so a spin
/// cannot hang the test; reaching it is a clean stop, not a failure.
pub const TEST386_MAX_RETIRED: u64 = 5_000_000;

/// Why the test386 smoke run stopped (§3.1). Captured on **every** exit path so
/// the POST progress stream is never lost to an error return.
///
/// This is a diagnostic classification for the report, not a golden: assertions
/// pin the *variant*, never an address (NASM output shifts stop addresses with
/// the toolchain, so any address lives in `detail` for humans only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The guest executed `hlt` and the machine terminated.
    Halt,
    /// The core hit a decodable instruction it does not lift (e.g. a segment-
    /// register load the real-mode backend does not model). `detail` records the
    /// faulting address for the report.
    Unsupported {
        /// Human-readable location of the unlifted instruction.
        detail: String,
    },
    /// The guest touched an address or port the minimal machine does not map.
    Unmapped {
        /// Human-readable description of the unmapped access.
        detail: String,
    },
    /// The guest raised a CPU exception the machine escaped to the host.
    Exception {
        /// x86 exception vector.
        vector: u8,
    },
    /// The retired-instruction safety cap was reached before a natural stop.
    BudgetExhausted,
}

/// The result of the test386 smoke run (§3.1): the captured POST progress bytes
/// and why execution stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Test386Run {
    /// Checkpoint bytes the guest wrote to the POST port `0x190`, in order.
    pub post: Vec<u8>,
    /// Why the run ended (for the report; assertions pin only the variant).
    pub stop: StopReason,
}

/// Boot the third-party test386 diagnostic image from RAM and record how far it
/// gets — an exploratory smoke test of the reset → real-mode-execution →
/// POST-capture plumbing, **not** full CPU validation (§3.1, §5).
///
/// The whole 1 MiB is RAM; the 64 KiB `blob` is loaded at `0xF0000` and execution
/// starts at the reset vector `F000:FFF0`, whose far jump enters the diagnostic's
/// POST sequence. The machine decodes just two devices — the POST checkpoint port
/// at `0x190` and a COM1 UART (in case a build variant enables serial) — behind a
/// [`NoInterruptController`]: the diagnostic runs `cli`, so wiring an IRQ that
/// could never be delivered would keep the host loop from ever terminating.
///
/// The POST byte stream is captured through the tracing seam on **every** exit
/// path. A real-mode-only core is expected to stop early on a 386 diagnostic, and
/// that stop surfaces as an `Err` from the machine (an unlifted instruction) — so
/// the error is classified into [`StopReason`] rather than propagated, and the
/// captured checkpoints are drained regardless of how the run ended.
pub fn run_test386(blob: &[u8]) -> Test386Run {
    let config = assembly::test386(blob.to_vec());
    let AssembledMachine {
        mut machine,
        device_index,
    } = match assemble(&config) {
        Ok(assembled) => assembled,
        Err(e) => {
            return Test386Run {
                post: Vec::new(),
                stop: StopReason::Unmapped {
                    detail: format!("building machine: {e}"),
                },
            }
        }
    };

    let result = machine.run_bounded(Some(TEST386_MAX_RETIRED));
    let rip = machine.cpu().reg(CpuReg::Rip);
    let stop = classify_test386_stop(rip, result);
    let post = drain_post_at(&mut machine, device_index[&DeviceId::Post], POST_PORT);
    Test386Run { post, stop }
}

/// Classify how a POST-only (no video) run stopped, reading the RIP for the
/// human-readable `detail` (never an assertion target — stop addresses shift with
/// the assembler toolchain).
fn classify_test386_stop(rip: u64, result: Result<RunOutcome, MachineError>) -> StopReason {
    match result {
        Ok(RunOutcome::Halted) => StopReason::Halt,
        Ok(RunOutcome::RetiredCapReached) => StopReason::BudgetExhausted,
        Err(MachineError::Cpu(CpuError::UnsupportedInstruction { addr })) => {
            StopReason::Unsupported {
                detail: format!("unsupported instruction at {addr:#x} (rip {rip:#x})"),
            }
        }
        Err(MachineError::Cpu(CpuError::Unmapped { addr })) => StopReason::Unmapped {
            detail: format!("unmapped guest access at {addr:#x} (rip {rip:#x})"),
        },
        Err(MachineError::Cpu(CpuError::Memory(msg))) => StopReason::Unmapped {
            detail: format!("guest memory error: {msg} (rip {rip:#x})"),
        },
        Err(MachineError::Cpu(e @ CpuError::UnexpectedExit { .. })) => StopReason::Unmapped {
            detail: format!("{e} (rip {rip:#x})"),
        },
        Err(MachineError::Bus(e)) => StopReason::Unmapped {
            detail: format!("{e} (rip {rip:#x})"),
        },
        Err(MachineError::Exception { vector }) => StopReason::Exception { vector },
    }
}

// ---------------------------------------------------------------------------
// Full BIOS machine: load a real 32 KiB PC BIOS ROM and boot it (§3.1, §5, §9).
// ---------------------------------------------------------------------------

/// The full 1 MiB guest address space of the BIOS machine.
pub const BIOS_SPACE: u64 = 0x10_0000;
/// Top of conventional (640 KiB) RAM — RAM covers `[0, 0xA0000)`.
pub const BIOS_CONVENTIONAL_TOP: u64 = 0x0A_0000;
/// Base of the option-ROM scan area `[0xC0000, 0xF8000)`, mapped as zeroed RAM so
/// the BIOS's `0x55AA` signature scan finds nothing and skips it cleanly.
pub const BIOS_OPTION_ROM_BASE: u64 = 0x0C_0000;
/// Base of the 32 KiB masked BIOS ROM: `[0xF8000, 0x100000)`.
pub const BIOS_ROM_BASE: u64 = 0x0F_8000;
/// Reset code segment: `CS = 0xF000`.
pub const BIOS_RESET_CS: u16 = 0xF000;
/// Reset instruction pointer: `IP = 0xFFF0` → linear `0xFFFF0`, inside the ROM,
/// where the reset far jump enters POST.
pub const BIOS_RESET_IP: u16 = 0xFFF0;

/// Cooperative run quantum for the BIOS machine, in `x86jit` block units (§6.2).
///
/// Finite (unlike the built-in demos' `None`) so a guest that spins without any
/// port I/O or halt still returns to the machine loop, where the retired cap and
/// the clock/interrupt path run. It only bounds how often the cap is checked.
pub const BIOS_QUANTUM: Option<u64> = Some(65_536);
/// Retired-instruction safety cap for the BIOS run (§6). POST plus a 640 KiB
/// memory test is hundreds of millions of instructions; this is a generous bound
/// so a spin cannot hang the run. Reaching it is a clean stop, not a failure.
pub const BIOS_MAX_RETIRED: u64 = 2_000_000_000;
/// Consecutive halted boundaries with no serviceable interrupt before the machine
/// declares the interrupt source quiescent and stops. Large so a BIOS that halts
/// waiting for a slow timer tick is not cut off mid-POST.
pub const BIOS_IDLE_LIMIT: u64 = 1_000_000;

/// Base of the monochrome (MDA) text window. When the option-ROM scan finds no
/// video BIOS, the Kiselev POST falls back to an MDA/CGA console whose INT 10h
/// teletype writes glyphs here rather than to the colour page at `0xB8000`. This
/// spike maps `0xB0000` as plain conventional RAM, so the console text is read
/// back from guest memory after the run.
pub const MDA_BUFFER_BASE: u64 = 0x0B_0000;

/// The result of a full BIOS boot run (§3.1): the POST checkpoint stream, both
/// text windows, the serial stream, and why execution stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BiosRun {
    /// Checkpoint bytes the BIOS wrote to POST port `0x80`, in order.
    pub post: Vec<u8>,
    /// The colour (CGA/VGA) text page at `0xB8000`, rendered from the card.
    pub screen: TextGrid,
    /// The monochrome (MDA) text window at `0xB0000`, read back from guest RAM.
    /// This is where the Kiselev BIOS's INT 10h console actually writes after its
    /// no-video-BIOS MDA fallback, so a booted guest's console text lands here.
    pub mda: TextGrid,
    /// Bytes the guest transmitted to COM1 (a booted sector's serial output).
    pub com1: Vec<u8>,
    /// Why the run ended (assertions pin only the variant, never an address).
    pub stop: StopReason,
}

/// Assemble the full PC/AT BIOS machine, map the 32 KiB `rom` at `0xF8000`, and
/// boot it from the reset vector — an exploratory spike of how far a real BIOS's
/// POST gets on a real-mode-only core (§3.1, §5, §9).
///
/// The address space is a full 1 MiB mosaic with no unmapped holes, so a POST
/// probe never aborts on unmapped memory:
///
/// - `[0x00000, 0xA0000)` — 640 KiB conventional RAM (RWX).
/// - `[0xA0000, 0xB8000)` — RAM (the BIOS may probe this graphics-adapter window).
/// - `[0xB8000, 0xC0000)` — the VGA colour text buffer, trapped MMIO routed to the
///   text card.
/// - `[0xC0000, 0xF8000)` — zeroed RAM standing in for the option-ROM scan area:
///   the BIOS's `0x55AA` signature scan finds nothing and skips it (no option ROM
///   this spike).
/// - `[0xF8000, 0x100000)` — the masked 32 KiB BIOS ROM, loaded with `rom`.
///
/// The cascaded 8259 pair, 8253 PIT, 8042, seeded MC146818 CMOS/RTC, VGA text
/// card, port-`0x61` system-control latch, and a POST port at `0x80` are all
/// decoded. The PIT counter-0 OUT is wired to IRQ0, the 8042 to IRQ1, and the RTC
/// update-ended line to IRQ8; the BIOS programs the PICs itself.
///
/// **Port `0x80` decode.** On a real AT the DMA page registers overlap `0x80`;
/// this spike wires no 8237 (floppy DMA is later work), so `0x80` is decoded to
/// the POST port and any stray DMA-page write elsewhere is absorbed by the bus's
/// open-bus default.
///
/// The POST stream and the text screen are captured on **every** exit path — the
/// whole point is to see how far POST got even when the run ends on an
/// instruction the real-mode core does not lift.
pub fn run_bios(rom: &[u8]) -> BiosRun {
    run_bios_config(assembly::bios(rom.to_vec(), None, BIOS_QUANTUM), |_| {
        bios_natural_policy()
    })
}

/// The natural-stop policy the plain BIOS/POST boots run under: cap the total
/// retired instructions at [`BIOS_MAX_RETIRED`], declare the interrupt source
/// quiescent after [`BIOS_IDLE_LIMIT`] idle boundaries, and stop when the guest
/// halts (relocated out of machine construction into the policy).
fn bios_natural_policy() -> RunPolicy {
    RunPolicy::natural(Some(BIOS_MAX_RETIRED), Some(BIOS_IDLE_LIMIT))
}

/// The MDA console probe the FreeDOS runs read back to detect FreeCOM's prompt:
/// the monochrome text window at [`MDA_BUFFER_BASE`], rendered exactly as the
/// frontend renders that console so a needle match here is what the caller sees.
fn mda_console_probe() -> ConsoleProbe {
    ConsoleProbe {
        base: MDA_BUFFER_BASE,
        cols: COLS,
        rows: ROWS,
    }
}

/// The prompt-poll policy the FreeDOS runs share: step to FreeCOM's date prompt on
/// the MDA console in [`DOS_INTERACTIVE_POLL_STEP`] chunks, then queue `stimuli` and
/// run `then_budget` more so the shell processes them. The plain FreeDOS boot passes
/// no stimuli and a zero budget (it stops at the prompt); the interactive run passes
/// the keyboard script and [`DOS_INTERACTIVE_COMMAND_BUDGET`].
fn freedos_prompt_policy(stimuli: Vec<KeyboardStimulus>, then_budget: u64) -> RunPolicy {
    RunPolicy {
        retired_cap: Some(BIOS_MAX_RETIRED),
        idle_limit: Some(BIOS_IDLE_LIMIT),
        stop: StopCondition::PromptThen {
            console: mda_console_probe(),
            needle: DOS_DATE_PROMPT.to_string(),
            poll_step: DOS_INTERACTIVE_POLL_STEP,
            then_budget,
        },
        stimuli,
    }
}

/// Assemble a BIOS machine from `config`, run it under the policy `build_policy`
/// derives, and capture every observable output path into a [`BiosRun`] (§3.1).
///
/// "How we run" is a [`RunPolicy`], not part of the machine, and `build_policy`
/// builds it *after* assembly so a stimulus can address the keyboard controller by
/// role through the assembler's device-index map (never a hardcoded bus index) —
/// exactly how the interactive FreeDOS run resolves its 8042. The plain BIOS/POST
/// paths hand back a natural-stop policy; the FreeDOS paths a prompt-poll one.
fn run_bios_config(
    config: MachineConfig,
    build_policy: impl FnOnce(&HashMap<DeviceId, usize>) -> RunPolicy,
) -> BiosRun {
    let AssembledMachine {
        mut machine,
        device_index,
    } = match assemble(&config) {
        Ok(assembled) => assembled,
        Err(e) => {
            return BiosRun {
                post: Vec::new(),
                screen: blank_screen(),
                mda: blank_screen(),
                com1: Vec::new(),
                stop: StopReason::Unmapped {
                    detail: format!("building machine: {e}"),
                },
            }
        }
    };

    let policy = build_policy(&device_index);
    let result = assembly::run(&mut machine, &policy);

    let stop = classify_bios_stop(&machine, result);
    let post = drain_post_at(&mut machine, device_index[&DeviceId::Post], 0x80);
    let screen = render_vga(&mut machine, device_index[&DeviceId::Vga]);
    let mda = render_mda(&machine);
    let com1 = drain_serial(&mut machine, device_index[&DeviceId::Uart]);
    BiosRun {
        post,
        screen,
        mda,
        com1,
        stop,
    }
}

/// Assemble the full PC/AT BIOS machine as [`run_bios`] does, but with a µPD765
/// floppy controller (ports `0x3F0..=0x3F7`, IRQ6) driven by an 8237 DMA
/// controller (ports `0x00..=0x0F` and the page latches `0x81..=0x8F`) backed by
/// `floppy`, and boot it (§3.1, §5, §7.7).
///
/// This is the path the BIOS's INT 13h / INT 19h use to load a boot sector: the
/// controller reads the sector from `floppy`, the machine moves it into the guest
/// buffer through the 8237 (the SMC-safe `note_write` path), and IRQ6 signals
/// completion. `floppy` is a raw disk image (typically [`FLOPPY_IMAGE_BYTES`]
/// bytes for a 1.44 MB disk); [`bootable_floppy_image`] builds a minimal one.
pub fn run_bios_boot(rom: &[u8], floppy: Vec<u8>) -> BiosRun {
    run_bios_config(
        assembly::bios(rom.to_vec(), Some(floppy), BIOS_QUANTUM),
        |_| bios_natural_policy(),
    )
}

/// Boot a real FreeDOS floppy image on the Kiselev BIOS machine and report how
/// far the FreeDOS boot chain gets (§3.1, §5, §7.7).
///
/// This is exactly [`run_bios_boot`] — the same firmware, FDC, and DMA — named
/// for its purpose: `floppy` is a bootable 1.44 MB FreeDOS image (FAT12 BPB, a
/// FreeDOS boot sector, an 8086 `KERNEL.SYS` and `COMMAND.COM` in the root). The
/// BIOS's INT 19h loads sector 0 and jumps to it, the FreeDOS boot sector loads
/// the kernel, and the kernel initialises and hands off to the shell. The
/// returned [`BiosRun`] carries every observable output path so the stop point
/// is captured whatever it is: the POST stream, the colour page ([`BiosRun::screen`]),
/// the MDA console the BIOS actually writes to ([`BiosRun::mda`]), COM1, and the
/// classified [`BiosRun::stop`].
pub fn run_freedos(rom: &[u8], floppy: Vec<u8>) -> BiosRun {
    run_bios_config(
        assembly::bios(rom.to_vec(), Some(floppy), BIOS_QUANTUM),
        |_| freedos_prompt_policy(Vec::new(), 0),
    )
}

/// A substring of the FreeCOM startup date prompt, used to detect that the shell
/// has booted and is blocked on keyboard input before the script is injected.
pub const DOS_DATE_PROMPT: &str = "Enter new date";

/// Cooperative run quantum for the interactive FreeDOS machine (§6.2), far finer
/// than the boot-path [`BIOS_QUANTUM`].
///
/// The machine acknowledges and injects at most one PIC vector per quantum, at the
/// quantum boundary. With the coarse boot quantum, a busy-polling guest retires
/// many timer periods per quantum, so a fresh higher-priority timer IRQ0 is latched
/// every boundary and always wins the PIC's priority race — a pending keyboard IRQ1
/// is starved forever and never delivered. A quantum finer than one timer period
/// (BIOS divisor 65536 → ~65536 retired instructions per period) means most
/// boundaries have no fresh IRQ0, so the pending IRQ1 is acknowledged and INT 09h
/// runs. The boot to the shell prompt is only ~25M instructions, so the finer
/// quantum costs no measurable extra time.
///
/// ## Why this is a driver knob, not a model fix
///
/// This is the one interrupt-fidelity limitation that stays in the frontend: it is
/// a granularity tradeoff, not an edge-detection bug. Interrupt latency in this
/// emulator is quantised to the run quantum — devices tick and the interrupt path
/// is serviced only on `run()` boundaries, and only one vector is acknowledged per
/// boundary. Real hardware clears IRQ0's line between its ~55 ms periods, leaving
/// gaps in which a pending IRQ1 wins; batching many periods into one coarse quantum
/// collapses those gaps.
///
/// A faithful in-model fix would bound how much virtual time may elapse between
/// interrupt-service points regardless of the caller's quantum — i.e. run the CPU
/// in sub-quanta no longer than about one timer period so the interrupt path is
/// revisited before IRQ0 is re-latched. That cannot be done without re-running the
/// CPU between service points (an injected vector is only delivered on the next
/// `run()`), so it amounts to the machine capping the quantum itself. Doing that
/// globally would shift every interrupt-timing-sensitive golden that boots on the
/// coarse [`BIOS_QUANTUM`] (the BIOS-POST and FreeDOS-boot captures are tuned to
/// it), so it is deliberately left as this per-run knob until a fix can be shown
/// golden-neutral. Choosing the injection quantum is exactly the "budget quantum =
/// interrupt latency" contract the interrupt-injection design already records.
pub const DOS_INTERACTIVE_QUANTUM: Option<u64> = Some(64);

/// Retired-instruction step between prompt-detection polls in the interactive
/// FreeDOS run (§6). Small enough to inject promptly once FreeCOM reaches its
/// date prompt, large enough that the poll overhead is negligible against the
/// hundreds of millions of instructions the BIOS POST and boot retire.
pub const DOS_INTERACTIVE_POLL_STEP: u64 = 25_000_000;

/// Retired-instruction budget granted after the script is queued, for INT 09h to
/// deliver every scancode and FreeCOM to finish the command and print its output
/// before the console is read.
pub const DOS_INTERACTIVE_COMMAND_BUDGET: u64 = 80_000_000;

/// Boot a real FreeDOS floppy image on the Kiselev BIOS machine and drive the
/// shell interactively with a **keyboard script**, past its startup prompts and
/// into a command (§3.1, §5, §7.5).
///
/// Same firmware, FDC, and DMA as [`run_freedos`], but the machine runs on the
/// finer [`DOS_INTERACTIVE_QUANTUM`] and is driven in two phases:
///
/// 1. **Boot to the prompt.** Step in [`DOS_INTERACTIVE_POLL_STEP`] chunks until
///    FreeCOM boots and blocks at its date prompt ([`DOS_DATE_PROMPT`] on the MDA
///    console). Keys must wait for DOS to own the keyboard: injected at power-on
///    they are consumed or corrupted by the BIOS keyboard-reset handshake and the
///    POST buffer flush, and the prompt never advances.
/// 2. **Type the script and run the command.** Queue the whole `scancodes` script
///    into the 8042 at once — each a set-1 (XT) make/break byte, exactly what INT
///    09h reads at `0x60` (this machine models no translation) — then run
///    [`DOS_INTERACTIVE_COMMAND_BUDGET`] more. The 8042 delivers queued bytes
///    serially, shifting the next in on a tick after each read, so every scancode
///    drops and re-raises the IRQ1 line on its own and INT 09h gets a clean edge
///    per byte; INT 09h translates each into the BDA keyboard ring where DOS's INT
///    16h reads pop it, then FreeCOM runs the command and prints its output.
///
/// A short script such as `\r\rDIR\r` (accept the date, accept the time, then type
/// `DIR` + Enter) reaches the `A:\>` prompt and a directory listing.
///
/// The still-finer-than-boot quantum is a deliberate concession, not the input
/// path's requirement: the machine acknowledges at most one PIC vector per run
/// quantum, so on the coarse boot quantum a busy-polling FreeCOM lets the periodic
/// timer IRQ0 win every priority race and starves the keyboard IRQ1 (see
/// [`DOS_INTERACTIVE_QUANTUM`]). A per-byte injection pace is no longer needed —
/// that was the 8042's edge-collapsing behaviour, now fixed in the controller
/// model.
///
/// Fully deterministic — every boundary falls on a retired-instruction count,
/// never wall time (§6) — so the session reproduces byte for byte. The returned
/// [`BiosRun`] carries every observable output path, so the resulting `A:\>`
/// prompt and command output land in the MDA console ([`BiosRun::mda`]).
pub fn run_freedos_interactive(rom: &[u8], floppy: Vec<u8>, scancodes: &[u8]) -> BiosRun {
    // The machine is the BIOS+floppy shape on the fine interactive quantum (guest-
    // visible interrupt latency, so it lives in the config's clock — see
    // [`DOS_INTERACTIVE_QUANTUM`]); the run is a prompt-poll policy that boots to the
    // date prompt, queues the keyboard script into the 8042, then runs the command
    // budget. The 8042 is addressed by role: the stimulus takes its bus index from
    // the assembler's device-index map, not a hardcoded constant.
    let script = scancodes.to_vec();
    run_bios_config(
        assembly::bios(rom.to_vec(), Some(floppy), DOS_INTERACTIVE_QUANTUM),
        |device_index| {
            let stimulus = KeyboardStimulus {
                keyboard_index: device_index[&DeviceId::Keyboard],
                scancodes: script,
                schedule: InjectionSchedule::AfterPrompt,
            };
            freedos_prompt_policy(vec![stimulus], DOS_INTERACTIVE_COMMAND_BUDGET)
        },
    )
}

/// Queue a keyboard `scancodes` script into the 8042 at bus index `kbc_index`,
/// reaching the concrete controller through the [`Device::as_any_mut`] downcast
/// seam — the machine core still sees only `&mut dyn Device`.
fn inject_scancodes(machine: &mut Machine<X86JitCpu>, kbc_index: usize, scancodes: &[u8]) {
    if let Some(dev) = machine.bus_mut().device_mut(kbc_index) {
        if let Some(kbc) = dev.as_any_mut().and_then(|a| a.downcast_mut::<I8042>()) {
            for &scancode in scancodes {
                kbc.push_scancode(scancode);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Keyboard script: set-1 (XT) make/break scancodes for driving the DOS shell.
// ---------------------------------------------------------------------------
//
// The BIOS INT 09h keyboard table expects set-1 codes and this machine's 8042
// performs no translation, so the exact make/break bytes below are what INT 09h
// reads at port 0x60. Each key press is a make code followed by its break code
// (make | 0x80).

/// `Enter` — set-1 make/break. INT 09h maps it to carriage return (`\r`), which
/// terminates a DOS input line.
pub const KEY_ENTER: [u8; 2] = [0x1C, 0x9C];
/// `D` — set-1 make/break.
pub const KEY_D: [u8; 2] = [0x20, 0xA0];
/// `I` — set-1 make/break.
pub const KEY_I: [u8; 2] = [0x17, 0x97];
/// `R` — set-1 make/break.
pub const KEY_R: [u8; 2] = [0x13, 0x93];

/// The keyboard script that drives FreeCOM from its date prompt to a `DIR`
/// listing: accept the date (Enter), accept the time (Enter), then type `DIR`
/// and run it (Enter). Each key contributes its make **and** break code.
///
/// FreeCOM, started with no `AUTOEXEC.BAT`, prompts for the date and then the
/// time before giving `A:\>`; an empty line (a bare Enter) keeps the current
/// value. So two Enters clear both prompts, and `DIR`+Enter runs the directory
/// listing at the shell.
#[must_use]
pub fn dos_dir_script() -> Vec<u8> {
    let mut script = Vec::new();
    script.extend_from_slice(&KEY_ENTER); // accept the date
    script.extend_from_slice(&KEY_ENTER); // accept the time
    script.extend_from_slice(&KEY_D);
    script.extend_from_slice(&KEY_I);
    script.extend_from_slice(&KEY_R);
    script.extend_from_slice(&KEY_ENTER); // run DIR
    script
}

/// Set-1 (XT) make code for the lowercase letters, digits, space, and `.` that
/// the shell scripts type. The 8042 performs no translation, so these are the
/// exact bytes the BIOS INT 09h handler reads at port `0x60`; INT 09h maps them
/// (with the shift state) to the ASCII FreeCOM's line input sees.
fn set1_make(c: char) -> u8 {
    match c {
        'a' => 0x1E,
        'b' => 0x30,
        'c' => 0x2E,
        'd' => 0x20,
        'e' => 0x12,
        'f' => 0x21,
        'g' => 0x22,
        'h' => 0x23,
        'i' => 0x17,
        'j' => 0x24,
        'k' => 0x25,
        'l' => 0x26,
        'm' => 0x32,
        'n' => 0x31,
        'o' => 0x18,
        'p' => 0x19,
        'q' => 0x10,
        'r' => 0x13,
        's' => 0x1F,
        't' => 0x14,
        'u' => 0x16,
        'v' => 0x2F,
        'w' => 0x11,
        'x' => 0x2D,
        'y' => 0x15,
        'z' => 0x2C,
        ' ' => 0x39,
        '.' => 0x34,
        '0' => 0x0B,
        '1' => 0x02,
        '2' => 0x03,
        '3' => 0x04,
        '4' => 0x05,
        '5' => 0x06,
        '6' => 0x07,
        '7' => 0x08,
        '8' => 0x09,
        '9' => 0x0A,
        _ => panic!("no set-1 scancode for {c:?}"),
    }
}

/// Append `text` to a keyboard `script` as set-1 make/break pairs. `>` is typed
/// as **Shift+`.`** (its US-layout combination): a left-shift make brackets the
/// `.` key and a left-shift break releases it, so INT 09h reads a shifted `.` and
/// yields `>`. Every other character is a plain make-then-break key.
fn type_set1(script: &mut Vec<u8>, text: &str) {
    const LSHIFT_MAKE: u8 = 0x2A;
    const LSHIFT_BREAK: u8 = 0xAA;
    for c in text.chars() {
        if c == '>' {
            let dot = set1_make('.');
            script.extend_from_slice(&[LSHIFT_MAKE, dot, dot | 0x80, LSHIFT_BREAK]);
        } else {
            let m = set1_make(c);
            script.extend_from_slice(&[m, m | 0x80]);
        }
    }
}

/// The keyboard script that drives FreeCOM to **create a file on the floppy** and
/// list it: accept the date (Enter), accept the time (Enter), type `echo x>zz`
/// (redirect one byte into a new file `ZZ`) + Enter, then type `dir` + Enter.
///
/// `echo x>zz` makes FreeCOM open `ZZ` for output and write the line to it, which
/// DOS commits through INT 21h → INT 13h → the µPD765 WRITE DATA path onto the
/// FAT12 media (a directory entry plus one data sector). `dir` then reads the
/// directory back **from disk**, so a run where `ZZ` appears in the listing proves
/// the whole DOS write path reached media — not just that the keys arrived.
#[must_use]
pub fn dos_write_file_script() -> Vec<u8> {
    let mut script = Vec::new();
    script.extend_from_slice(&KEY_ENTER); // accept the date
    script.extend_from_slice(&KEY_ENTER); // accept the time
    type_set1(&mut script, "echo x>zz");
    script.extend_from_slice(&KEY_ENTER); // create the file
    type_set1(&mut script, "dir");
    script.extend_from_slice(&KEY_ENTER); // list the directory from disk
    script
}

/// Read back the monochrome (MDA) text window at [`MDA_BUFFER_BASE`] from guest
/// RAM and render it into a [`TextGrid`] (§9.2).
///
/// Unlike the colour page at `0xB8000` — trapped MMIO reconstructed from the
/// card's trace — the MDA window is plain conventional RAM in this machine, so
/// its 80×25 two-bytes-per-cell contents are read directly through the CPU's
/// `read_mem` seam after the run. The MDA CRTC ports (`0x3B4`/`0x3B5`) are not
/// decoded here, so the display-start and cursor registers are unknown; the grid
/// is rendered from cell 0 with the cursor hidden, which is faithful for a
/// console that scrolls its buffer rather than panning the CRTC start address.
fn render_mda(machine: &Machine<X86JitCpu>) -> TextGrid {
    let mut vram = vec![0u8; TEXT_BUFFER_LEN];
    let cells = (usize::from(COLS) * usize::from(ROWS) * 2).min(vram.len());
    // A short read leaves the tail blank rather than failing the whole render.
    let _ = machine.cpu().read_mem(MDA_BUFFER_BASE, &mut vram[..cells]);
    render_text_mode(&TextModeState {
        vram: &vram,
        cols: COLS,
        rows: ROWS,
        start_cell: 0,
        cursor_cell: 0,
        cursor_visible: false,
    })
}

/// Classify how the BIOS run stopped into a [`StopReason`], reading back the bytes
/// at the stopping CS:IP so an unlifted/faulting instruction can be disassembled
/// for the report (§3.1). Addresses live in `detail` for humans only.
fn classify_bios_stop(
    machine: &Machine<X86JitCpu>,
    result: Result<RunOutcome, MachineError>,
) -> StopReason {
    let ip = machine.cpu().reg(CpuReg::Rip);
    let cs = machine.cpu().code_segment();
    let linear = (cs << 4).wrapping_add(ip) & 0xF_FFFF;
    let mut bytes = [0u8; 16];
    let hex = if machine.cpu().read_mem(linear, &mut bytes).is_ok() {
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        "<unreadable>".to_string()
    };
    let site = format!("cs:ip {cs:#06x}:{ip:#06x} (linear {linear:#07x}) bytes [{hex}]");
    match result {
        Ok(RunOutcome::Halted) => StopReason::Halt,
        Ok(RunOutcome::RetiredCapReached) => StopReason::BudgetExhausted,
        Err(MachineError::Cpu(CpuError::UnsupportedInstruction { addr })) => {
            StopReason::Unsupported {
                detail: format!("unsupported instruction at {addr:#x}; {site}"),
            }
        }
        Err(MachineError::Cpu(CpuError::Unmapped { addr })) => StopReason::Unmapped {
            detail: format!("unmapped guest access at {addr:#x}; {site}"),
        },
        Err(MachineError::Cpu(CpuError::Memory(msg))) => StopReason::Unmapped {
            detail: format!("guest memory error: {msg}; {site}"),
        },
        Err(MachineError::Cpu(e @ CpuError::UnexpectedExit { .. })) => StopReason::Unmapped {
            detail: format!("{e}; {site}"),
        },
        Err(MachineError::Bus(e)) => StopReason::Unmapped {
            detail: format!("{e}; {site}"),
        },
        Err(MachineError::Exception { vector }) => StopReason::Exception { vector },
    }
}

/// Drain a POST checkpoint byte stream from the port device at `index`, keeping
/// only writes to `post_port` (§3.2 tracing seam).
fn drain_post_at(machine: &mut Machine<X86JitCpu>, index: usize, post_port: u16) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(dev) = machine.bus_mut().device_mut(index) {
        for event in dev.take_trace() {
            if let TraceEvent::Port {
                port,
                dir: AccessDir::Write,
                value,
                ..
            } = event
            {
                if port == post_port {
                    out.push(value as u8);
                }
            }
        }
    }
    out
}

/// A cleared 80×25 text grid, for a BIOS run that failed before any video output.
fn blank_screen() -> TextGrid {
    let vram = vec![0u8; TEXT_BUFFER_LEN];
    render_text_mode(&TextModeState {
        vram: &vram,
        cols: COLS,
        rows: ROWS,
        start_cell: 0,
        cursor_cell: 0,
        cursor_visible: false,
    })
}

// ---------------------------------------------------------------------------
// Fast floppy-boot demo: drive the FDC + 8237 to load a boot sector, no BIOS.
// ---------------------------------------------------------------------------

/// Load offset of the hand-encoded floppy-boot loader (CS = 0, so linear too).
pub const FLOPPY_LOADER_IP: u16 = 0x7E00;
/// Where the loader deposits and jumps to the boot sector (classic boot address).
pub const FLOPPY_BOOT_TARGET: u16 = 0x7C00;
/// The byte the minimal boot sector transmits to COM1 to prove it executed.
pub const FLOPPY_BOOT_MARKER: u8 = b'B';

/// A hand-encoded real-mode loader (no assembler) that reads sector 0 of the
/// floppy into `0x7C00` and far-jumps to it — a self-contained stand-in for the
/// BIOS INT 13h/INT 19h boot path, so the FDC → 8237 → memory → IRQ6 plumbing is
/// exercised deterministically without a multi-minute BIOS POST. Every byte is
/// documented in the `floppy_boot_golden` integration test.
///
/// It programs 8237 channel 2 for a device→memory transfer to physical `0x07C00`
/// (512 bytes), enables the FDC (DOR: motor 0, DMA, out of reset), issues READ
/// DATA for C0/H0/R1, then `jmp 0x0000:0x7C00`. The machine runs the DMA transfer
/// as the READ command completes, so the sector is in memory before the jump.
#[rustfmt::skip]
pub const FLOPPY_LOADER: &[u8] = &[
    // ---- program 8237 channel 2 (device -> memory, phys 0x07C00, 512 bytes) ----
    0xB0, 0x06, 0xE6, 0x0A, // mov al,0x06 ; out 0x0A,al  (mask channel 2)
    0xB0, 0x00, 0xE6, 0x0C, // mov al,0x00 ; out 0x0C,al  (clear flip-flop)
    0xB0, 0x46, 0xE6, 0x0B, // mov al,0x46 ; out 0x0B,al  (mode: ch2, write, single)
    0xB0, 0x00, 0xE6, 0x04, // mov al,0x00 ; out 0x04,al  (address low)
    0xB0, 0x7C, 0xE6, 0x04, // mov al,0x7C ; out 0x04,al  (address high -> 0x7C00)
    0xB0, 0x00, 0xE6, 0x81, // mov al,0x00 ; out 0x81,al  (channel 2 page = 0)
    0xB0, 0xFF, 0xE6, 0x05, // mov al,0xFF ; out 0x05,al  (count low  = 0x01FF)
    0xB0, 0x01, 0xE6, 0x05, // mov al,0x01 ; out 0x05,al  (count high -> 512 bytes)
    0xB0, 0x02, 0xE6, 0x0A, // mov al,0x02 ; out 0x0A,al  (unmask channel 2)
    // ---- enable the floppy controller ----
    0xBA, 0xF2, 0x03,       // mov dx,0x03F2
    0xB0, 0x1C,             // mov al,0x1C  (motor 0 on, DMA enable, out of reset)
    0xEE,                   // out dx,al
    // ---- issue READ DATA (C0 H0 R1, N2, EOT1) via the data register ----
    0xBA, 0xF5, 0x03,       // mov dx,0x03F5
    0xB0, 0x06, 0xEE,       // mov al,0x06 ; out dx,al  (READ DATA opcode)
    0xB0, 0x00, 0xEE,       // mov al,0x00 ; out dx,al  (drive 0, head 0)
    0xB0, 0x00, 0xEE,       // mov al,0x00 ; out dx,al  (cylinder 0)
    0xB0, 0x00, 0xEE,       // mov al,0x00 ; out dx,al  (head 0)
    0xB0, 0x01, 0xEE,       // mov al,0x01 ; out dx,al  (sector 1)
    0xB0, 0x02, 0xEE,       // mov al,0x02 ; out dx,al  (N = 2 -> 512 bytes)
    0xB0, 0x01, 0xEE,       // mov al,0x01 ; out dx,al  (EOT = 1)
    0xB0, 0x1B, 0xEE,       // mov al,0x1B ; out dx,al  (GPL)
    0xB0, 0xFF, 0xEE,       // mov al,0xFF ; out dx,al  (DTL)
    // ---- hand control to the loaded boot sector ----
    0xEA, 0x00, 0x7C, 0x00, 0x00, // jmp 0x0000:0x7C00
];

/// The minimal boot sector carried at LBA 0 of [`bootable_floppy_image`]: write
/// `'B'` to COM1 and halt.
#[rustfmt::skip]
const FLOPPY_BOOT_SECTOR: &[u8] = &[
    0xBA, 0xF8, 0x03, // mov dx,0x03F8   (COM1 THR)
    0xB0, b'B',       // mov al,'B'
    0xEE,             // out dx,al
    0xF4,             // hlt
];

/// Build a minimal bootable 1.44 MB floppy image: the boot sector at offset 0
/// (writing `'B'` to COM1, then `hlt`) with the `0x55 0xAA` signature at offset
/// 510, and the rest zeroed. A real FreeDOS image is separate later work; this
/// keeps the boot test self-contained.
#[must_use]
pub fn bootable_floppy_image() -> Vec<u8> {
    let mut image = vec![0u8; FLOPPY_IMAGE_BYTES];
    image[..FLOPPY_BOOT_SECTOR.len()].copy_from_slice(FLOPPY_BOOT_SECTOR);
    image[510] = 0x55;
    image[511] = 0xAA;
    image
}

/// Assemble a minimal real-mode machine — COM1 UART, 8237 DMA, µPD765 floppy
/// controller backed by [`bootable_floppy_image`] — run the [`FLOPPY_LOADER`],
/// and return the bytes the guest sent to COM1 (§7.7).
///
/// The loader drives the controller to read the boot sector into `0x7C00` over
/// DMA and jumps to it; the sector transmits [`FLOPPY_BOOT_MARKER`] and halts, so
/// a successful boot returns exactly that byte. This proves the whole path — FDC
/// command state machine, 8237 transfer into guest memory, and the boot jump —
/// deterministically and fast.
pub fn run_floppy_boot() -> Result<Vec<u8>> {
    let config = assembly::floppy_boot(FLOPPY_LOADER.to_vec(), bootable_floppy_image());
    let AssembledMachine {
        mut machine,
        device_index,
    } = assemble(&config).context("assembling floppy-boot machine")?;

    machine.run().context("running machine")?;

    Ok(drain_serial(&mut machine, device_index[&DeviceId::Uart]))
}

/// Assemble the serial machine, run `blob` in Long64, and return the bytes the
/// guest sent to COM1 (§3.1).
pub fn run_serial(blob: &[u8]) -> Result<Vec<u8>> {
    // The Long64 serial skeleton is a single 16550 UART at COM1 and nothing else.
    // It is assembled inline (trait-free) rather than through a config preset: it
    // is the one Long64 machine, and giving the declarative config a mode axis for
    // a single user would not earn its keep.
    let devices: Vec<Box<dyn Device>> = vec![Box::new(Uart16550::new(COM1_BASE))];
    let map = DecodeMap {
        ports: vec![PortRange {
            ports: COM1_BASE..=COM1_TOP,
            device: 0,
        }],
        mmio: Vec::new(),
    };
    let bus = Bus::new(devices, map);

    let cpu = X86JitCpu::new(RAM_BYTES, ENTRY, blob).context("building cpu")?;
    // No interrupt hardware: a null controller acknowledges nothing, and the
    // `None` idle limit terminates on the guest's `hlt`.
    let mut machine = Machine::new(
        cpu,
        bus,
        Box::new(NoInterruptController),
        QUANTUM,
        Vec::new(),
        None,
    );

    machine.run().context("running machine")?;

    Ok(drain_serial(&mut machine, 0))
}

/// Assemble the PC/AT machine, run `blob` in real mode, and return the bytes the
/// guest sent to COM1 (§5, §7).
///
/// The PIT counter-0 OUT line (bus device index `2`) is wired to PIC input IRQ0;
/// the machine edge-detects it and acknowledges/injects the timer vector.
pub fn run_boot(blob: &[u8]) -> Result<Vec<u8>> {
    let config = assembly::at_boot(blob.to_vec());
    let AssembledMachine {
        mut machine,
        device_index,
    } = assemble(&config).context("assembling boot machine")?;

    // Natural run to quiescence under the demo's idle limit — relocated from the
    // machine's construction into the run policy. No retired cap: the demo masks
    // its timer and halts on its own.
    assembly::run(
        &mut machine,
        &RunPolicy::natural(None, Some(BOOT_IDLE_LIMIT)),
    )
    .context("running machine")?;

    Ok(drain_serial(&mut machine, device_index[&DeviceId::Uart]))
}

/// Assemble the PC/AT machine with `scancodes` queued in the keyboard, run `blob`
/// in real mode, and return the bytes the guest sent to COM1 (§5, §7).
///
/// The 8042's keyboard-byte-ready line (bus device index [`AT_I8042_INDEX`]) is
/// wired to PIC input IRQ1; the machine edge-detects it and acknowledges/injects
/// the keyboard vector once the guest enables keyboard interrupts and sets `IF`.
pub fn run_keyboard(blob: &[u8], scancodes: &[u8]) -> Result<Vec<u8>> {
    let config = assembly::at_keyboard(blob.to_vec());
    let AssembledMachine {
        mut machine,
        device_index,
    } = assemble(&config).context("assembling keyboard machine")?;

    // Queue the scancodes into the 8042 before the run, exactly as the legacy
    // chipset seeded them at construction — the guest raises IRQ1 for each once it
    // enables keyboard interrupts.
    inject_scancodes(&mut machine, device_index[&DeviceId::Keyboard], scancodes);

    // Natural run under the demo's idle limit (relocated into the run policy).
    assembly::run(
        &mut machine,
        &RunPolicy::natural(None, Some(KBD_IDLE_LIMIT)),
    )
    .context("running machine")?;

    Ok(drain_serial(&mut machine, device_index[&DeviceId::Uart]))
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
    let config = assembly::at_rtc(blob.to_vec(), RTC_SECOND_TICKS, RTC_UIP_TICKS);
    let AssembledMachine {
        mut machine,
        device_index,
    } = assemble(&config).context("assembling rtc machine")?;

    // Natural run under the demo's idle limit (relocated into the run policy).
    assembly::run(
        &mut machine,
        &RunPolicy::natural(None, Some(RTC_IDLE_LIMIT)),
    )
    .context("running machine")?;

    Ok(drain_serial(&mut machine, device_index[&DeviceId::Uart]))
}

/// Assemble the floppy-WRITE machine, run `program` in real mode, and return the
/// captured COM1 bytes (§7.7, §7.8).
///
/// The machine mounts a blank writable floppy; `program` drives the µPD765 and
/// 8237 to write a preloaded sector to media and read it back, emitting
/// [`FDC_WRITE_MARKER`] on a match. Nothing runs a wall clock, so the capture is
/// deterministic: a successful write→read cycle yields exactly `b"W"`.
pub fn run_fdc_write(program: &[u8]) -> Result<Vec<u8>> {
    let config = assembly::at_fdc_write(program.to_vec(), fdc_write_pattern(), FDC_WRITE_SRC);
    let AssembledMachine {
        mut machine,
        device_index,
    } = assemble(&config).context("assembling fdc-write machine")?;

    assembly::run(
        &mut machine,
        &RunPolicy::natural(Some(FDC_WRITE_RETIRED_CAP), None),
    )
    .context("running machine")?;

    Ok(drain_serial(&mut machine, device_index[&DeviceId::Uart]))
}

/// Assemble the PC/AT machine, run `blob` in real mode, then render the VGA text
/// card into a [`TextGrid`] and present it to a [`CaptureFrameSink`] (§9.2).
///
/// DS/ES are seeded to [`VGA_TEXT_SEGMENT`] so the guest's near stores reach the
/// text buffer, and the `0xB8000` window is mapped as trapped MMIO so those
/// stores surface on the bus and reach the card. Nothing runs a wall clock, so
/// the produced grid is deterministic.
pub fn run_vga_text(blob: &[u8]) -> Result<TextGrid> {
    let config = assembly::at_vga_text(blob.to_vec());
    let AssembledMachine {
        mut machine,
        device_index,
    } = assemble(&config).context("assembling vga-text machine")?;

    machine.run().context("running machine")?;

    let grid = render_vga(&mut machine, device_index[&DeviceId::Vga]);
    // Exercise the presentation seam: hand the frame to a capture sink and read it
    // back, exactly as a terminal or windowed presenter would receive it.
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
/// are read back through the tracing seam rather than a concrete-device downcast.
/// Every MMIO write into the text window is replayed
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

/// Drain the COM1 byte stream from the UART's transmitted-output sink (§7.2).
///
/// The UART already distinguishes THR writes from divisor-latch writes. Reading
/// its output through the frontend's downcast seam keeps that distinction in
/// one place; replaying port writes would also capture baud-rate programming.
fn drain_serial(machine: &mut Machine<X86JitCpu>, uart_index: usize) -> Vec<u8> {
    if let Some(dev) = machine.bus_mut().device_mut(uart_index) {
        if let Some(uart) = dev.as_any_mut().and_then(|a| a.downcast_mut::<Uart16550>()) {
            return uart.take_output();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_serial_excludes_divisor_writes() {
        let bus = Bus::new(
            vec![Box::new(Uart16550::new(COM1_BASE))],
            DecodeMap {
                ports: vec![PortRange {
                    ports: COM1_BASE..=COM1_TOP,
                    device: 0,
                }],
                mmio: Vec::new(),
            },
        );
        let cpu = X86JitCpu::new(RAM_BYTES, ENTRY, &[0xF4]).unwrap();
        let mut machine = Machine::new(
            cpu,
            bus,
            Box::new(NoInterruptController),
            QUANTUM,
            Vec::new(),
            None,
        );
        let uart = machine.bus_mut().device_mut(0).unwrap();
        uart.port_write(COM1_BASE + 3, 1, 0x83).unwrap();
        uart.port_write(COM1_BASE, 1, 0x42).unwrap();
        uart.port_write(COM1_BASE + 1, 1, 0x01).unwrap();
        uart.port_write(COM1_BASE + 3, 1, 0x03).unwrap();
        for &byte in b"OK\0\xff" {
            uart.port_write(COM1_BASE, 1, u32::from(byte)).unwrap();
        }

        assert_eq!(drain_serial(&mut machine, 0), b"OK\0\xff");
        assert!(drain_serial(&mut machine, 0).is_empty());

        let uart = machine.bus_mut().device_mut(0).unwrap();
        uart.port_write(COM1_BASE, 1, u32::from(b'!')).unwrap();
        let _ = uart.take_trace();
        assert_eq!(drain_serial(&mut machine, 0), b"!");
        assert!(drain_serial(&mut machine, 0).is_empty());
    }
}
