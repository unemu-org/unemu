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
use bus::{Bus, DecodeMap, MmioRange, PortRange};
use chipset::{
    AtChipset, CascadedPic, Chipset, NoInterruptController, SerialChipset, AT_CMOS_RTC_INDEX,
    AT_I8042_INDEX, AT_UART_INDEX, AT_VGA_INDEX, CMOS_DATA_PORT, CMOS_INDEX_PORT, COM1_BASE,
    COM1_TOP, KBD_CMD, KBD_DATA, PIC_COMMAND, PIC_DATA, PIT_BASE, PIT_TOP, SLAVE_PIC_COMMAND,
    SLAVE_PIC_DATA,
};
use cpu::x86jit_backend::RealSegments;
use cpu::{Cpu, CpuError, CpuReg, X86JitCpu};
use device::{AccessDir, Device, TraceEvent};
use devices::post_port::POST_PORT;
use devices::sys_control::SYS_CONTROL_PORT;
use devices::upd765::FLOPPY_IMAGE_BYTES;
use devices::vga_text::{
    CGA_MODE_CONTROL_PORT, CGA_STATUS_PORT, COLS, CRTC_CURSOR_HIGH_INDEX, CRTC_CURSOR_LOW_INDEX,
    CRTC_CURSOR_START_INDEX, CRTC_DATA_PORT, CRTC_INDEX_PORT, CRTC_START_HIGH_INDEX,
    CRTC_START_LOW_INDEX, ROWS, TEXT_BUFFER_BASE, TEXT_BUFFER_LEN,
};
use devices::{
    CmosConfig, CmosRtc, Dma8237, Pit8253, PostPort, SysControl, Uart16550, Upd765, VgaText, I8042,
};
use machine::{DmaWiring, IrqLine, Machine, MachineError, RunOutcome};
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
#[derive(Clone, Debug)]
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
    // Minimal device set: POST port (index 0) + COM1 UART (index 1). No PIC and
    // no IRQ wiring — the guest masks interrupts, so an undeliverable IRQ would
    // stall termination.
    let devices: Vec<Box<dyn device::Device>> = vec![
        Box::new(PostPort::new()),
        Box::new(Uart16550::new(COM1_BASE)),
    ];
    let map = DecodeMap {
        ports: vec![
            PortRange {
                ports: POST_PORT..=POST_PORT,
                device: 0,
            },
            PortRange {
                ports: COM1_BASE..=COM1_TOP,
                device: 1,
            },
        ],
        mmio: Vec::new(),
    };
    let bus = Bus::new(devices, map);

    // Load the image at 0xF0000 (RIP left at 0), then point execution at the
    // reset vector so the far jump there enters the POST sequence.
    let cpu = X86JitCpu::new_real16(
        TEST386_SPACE,
        TEST386_LOAD_SEG,
        0x0000,
        blob,
        RealSegments::flat(TEST386_LOAD_SEG),
    );
    let mut cpu = match cpu {
        Ok(cpu) => cpu,
        Err(e) => {
            return Test386Run {
                post: Vec::new(),
                stop: StopReason::Unmapped {
                    detail: format!("building cpu: {e}"),
                },
            }
        }
    };
    cpu.set_reg(CpuReg::Rip, u64::from(TEST386_RESET_IP));

    // No interrupt source is wired, so `None` idle limit terminates on the first
    // halt; the retired cap guards against a spin that never halts.
    let mut machine = Machine::new(
        cpu,
        bus,
        Box::new(NoInterruptController),
        TEST386_QUANTUM,
        Vec::new(),
        None,
    );

    let result = machine.run_bounded(Some(TEST386_MAX_RETIRED));
    // Record where execution stopped for the human-readable report (never an
    // assertion target — stop addresses shift with the assembler toolchain).
    let rip = machine.cpu().reg(CpuReg::Rip);
    let stop = match result {
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
        Err(MachineError::Bus(e)) => StopReason::Unmapped {
            detail: format!("{e} (rip {rip:#x})"),
        },
        Err(MachineError::Exception { vector }) => StopReason::Exception { vector },
    };

    // Drain the POST checkpoint bytes from the tracing seam regardless of how the
    // run ended — the whole point is to see progress even on an error stop.
    let post = drain_post(&mut machine, 0);
    Test386Run { post, stop }
}

/// Reconstruct the POST checkpoint byte stream from the POST port's trace (§3.2
/// tracing seam). Each guest write to `0x190` surfaces as a `TraceEvent::Port`
/// write; the machine core sees only `&dyn Device`, never a concrete downcast.
fn drain_post(machine: &mut Machine<X86JitCpu>, post_index: usize) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(dev) = machine.bus_mut().device_mut(post_index) {
        for event in dev.take_trace() {
            if let TraceEvent::Port {
                port,
                dir: AccessDir::Write,
                value,
                ..
            } = event
            {
                if port == POST_PORT {
                    out.push(value as u8);
                }
            }
        }
    }
    out
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

// Device indices in the BIOS machine's device set. Distinct from the AtChipset
// layout because this set adds the system-control port and a BIOS POST port.
const BIOS_UART_INDEX: usize = 0;
const BIOS_MASTER_PIC_INDEX: usize = 1;
const BIOS_PIT_INDEX: usize = 2;
const BIOS_I8042_INDEX: usize = 3;
const BIOS_VGA_INDEX: usize = 4;
const BIOS_SLAVE_PIC_INDEX: usize = 5;
const BIOS_RTC_INDEX: usize = 6;
const BIOS_SYS_CONTROL_INDEX: usize = 7;
const BIOS_POST_INDEX: usize = 8;
// Appended only when a floppy is attached, so the indices above stay stable.
const BIOS_FDC_INDEX: usize = 9;
const BIOS_DMA_INDEX: usize = 10;

/// The result of a full BIOS boot run (§3.1): the POST checkpoint stream, the
/// rendered text screen, and why execution stopped.
#[derive(Clone, Debug)]
pub struct BiosRun {
    /// Checkpoint bytes the BIOS wrote to POST port `0x80`, in order.
    pub post: Vec<u8>,
    /// The VGA text screen as rendered from the card after the run.
    pub screen: TextGrid,
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
    run_bios_inner(rom, None)
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
    run_bios_inner(rom, Some(floppy))
}

fn run_bios_inner(rom: &[u8], floppy: Option<Vec<u8>>) -> BiosRun {
    // Each PIC chip's device face is cloned off the one live pair; the pair itself
    // is the machine's interrupt controller, so a guest EOI to either chip reaches
    // the state the machine's INTA set.
    let pic = CascadedPic::new();
    let master_device = pic.master_device();
    let slave_device = pic.slave_device();
    let controller = Box::new(pic);

    let rtc = CmosRtc::new().with_config(CmosConfig::default());

    let mut devices: Vec<Box<dyn Device>> = vec![
        Box::new(Uart16550::new(COM1_BASE)), // 0
        Box::new(master_device),             // 1
        Box::new(Pit8253::new()),            // 2
        Box::new(I8042::new()),              // 3
        Box::new(VgaText::new()),            // 4
        Box::new(slave_device),              // 5
        Box::new(rtc),                       // 6
        Box::new(SysControl::new()),         // 7
        Box::new(PostPort::with_port(0x80)), // 8
    ];
    if let Some(image) = floppy.clone() {
        devices.push(Box::new(Upd765::with_image(image))); // 9
        devices.push(Box::new(Dma8237::new())); // 10
    }

    let mut ports = vec![
        PortRange {
            ports: COM1_BASE..=COM1_TOP,
            device: BIOS_UART_INDEX,
        },
        PortRange {
            ports: PIC_COMMAND..=PIC_DATA,
            device: BIOS_MASTER_PIC_INDEX,
        },
        PortRange {
            ports: PIT_BASE..=PIT_TOP,
            device: BIOS_PIT_INDEX,
        },
        // The 8042 decodes two non-adjacent ports (0x60 and 0x64).
        PortRange {
            ports: KBD_DATA..=KBD_DATA,
            device: BIOS_I8042_INDEX,
        },
        PortRange {
            ports: KBD_CMD..=KBD_CMD,
            device: BIOS_I8042_INDEX,
        },
        // Port 0x61 system-control latch (refresh bit the BIOS times against).
        PortRange {
            ports: SYS_CONTROL_PORT..=SYS_CONTROL_PORT,
            device: BIOS_SYS_CONTROL_INDEX,
        },
        // POST checkpoint port 0x80 (see the DMA-overlap note above).
        PortRange {
            ports: 0x80..=0x80,
            device: BIOS_POST_INDEX,
        },
        PortRange {
            ports: CMOS_INDEX_PORT..=CMOS_DATA_PORT,
            device: BIOS_RTC_INDEX,
        },
        PortRange {
            ports: CRTC_INDEX_PORT..=CRTC_DATA_PORT,
            device: BIOS_VGA_INDEX,
        },
        PortRange {
            ports: CGA_MODE_CONTROL_PORT..=CGA_STATUS_PORT,
            device: BIOS_VGA_INDEX,
        },
        PortRange {
            ports: SLAVE_PIC_COMMAND..=SLAVE_PIC_DATA,
            device: BIOS_SLAVE_PIC_INDEX,
        },
    ];
    if floppy.is_some() {
        // The floppy controller and the 8237 DMA it transfers through. Port 0x80
        // stays the POST checkpoint above; the DMA's page latches decode from 0x81
        // up, so the overlap at 0x80 resolves to POST and 0x81 to channel 2's page.
        ports.push(PortRange {
            ports: 0x3F0..=0x3F7,
            device: BIOS_FDC_INDEX,
        });
        ports.push(PortRange {
            ports: 0x00..=0x0F,
            device: BIOS_DMA_INDEX,
        });
        ports.push(PortRange {
            ports: 0x81..=0x8F,
            device: BIOS_DMA_INDEX,
        });
    }
    let map = DecodeMap {
        ports,
        mmio: vec![MmioRange {
            addrs: TEXT_BUFFER_BASE..=TEXT_BUFFER_BASE + TEXT_BUFFER_LEN as u64 - 1,
            device: BIOS_VGA_INDEX,
        }],
    };
    let bus = Bus::new(devices, map);

    // Lay out the full 1 MiB mosaic (no unmapped holes), then load the ROM.
    let cpu = match build_bios_cpu(rom) {
        Ok(cpu) => cpu,
        Err(e) => {
            return BiosRun {
                post: Vec::new(),
                screen: blank_screen(),
                com1: Vec::new(),
                stop: StopReason::Unmapped {
                    detail: format!("building cpu: {e}"),
                },
            }
        }
    };
    // Wire the interrupt sources the BIOS's POST and INT layer rely on.
    let mut irq_lines = vec![
        IrqLine {
            device: BIOS_PIT_INDEX,
            irq: 0,
        },
        IrqLine {
            device: BIOS_I8042_INDEX,
            irq: 1,
        },
        IrqLine {
            device: BIOS_RTC_INDEX,
            irq: 8,
        },
    ];
    let mut dma_wirings = Vec::new();
    if floppy.is_some() {
        // IRQ6 is a master-PIC line; the controller drives it on command
        // completion. The floppy controller masters the bus through the 8237.
        irq_lines.push(IrqLine {
            device: BIOS_FDC_INDEX,
            irq: 6,
        });
        dma_wirings.push(DmaWiring {
            requester: BIOS_FDC_INDEX,
            controller: BIOS_DMA_INDEX,
        });
    }
    let mut machine = Machine::new(
        cpu,
        bus,
        controller,
        BIOS_QUANTUM,
        irq_lines,
        Some(BIOS_IDLE_LIMIT),
    )
    .with_dma_wirings(dma_wirings);

    let result = machine.run_bounded(Some(BIOS_MAX_RETIRED));
    let stop = classify_bios_stop(&machine, result);

    let post = drain_post_at(&mut machine, BIOS_POST_INDEX, 0x80);
    let screen = render_vga(&mut machine, BIOS_VGA_INDEX);
    let com1 = drain_serial(&mut machine, BIOS_UART_INDEX);
    BiosRun {
        post,
        screen,
        com1,
        stop,
    }
}

/// Build the BIOS machine's CPU with the full 1 MiB mosaic address space and the
/// ROM loaded at `0xF8000`, pointed at the reset vector.
fn build_bios_cpu(rom: &[u8]) -> Result<X86JitCpu, CpuError> {
    // Segments start flat at the reset CS; the BIOS reloads them via its reset
    // far jump, which the real-mode core now lifts. SP is left at the default; the
    // BIOS sets up its own stack before pushing.
    let mut cpu = X86JitCpu::new_real16_bare(
        BIOS_SPACE,
        BIOS_RESET_CS,
        BIOS_RESET_IP,
        RealSegments::flat(BIOS_RESET_CS),
    )?;
    // 640 KiB conventional RAM + the graphics-adapter probe window below the text
    // buffer, as one RAM region up to the trapped video page.
    cpu.map_ram(0, TEXT_BUFFER_BASE)?;
    // The colour text page is trapped MMIO (mapped on the bus to the VGA card);
    // it must be a hole in RAM, so RAM resumes above it.
    cpu.map_mmio(TEXT_BUFFER_BASE, TEXT_BUFFER_LEN as u64)?;
    // Zeroed option-ROM scan area: [0xC0000, 0xF8000). No 0x55AA signature, so the
    // BIOS's option-ROM scan finds nothing and moves on.
    cpu.map_ram(BIOS_OPTION_ROM_BASE, BIOS_ROM_BASE - BIOS_OPTION_ROM_BASE)?;
    // The masked 32 KiB BIOS ROM at the top of the space.
    cpu.map_rom(BIOS_ROM_BASE, rom)?;
    Ok(cpu)
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
    let devices: Vec<Box<dyn Device>> = vec![
        Box::new(Uart16550::new(COM1_BASE)),                   // 0
        Box::new(Dma8237::new()),                              // 1
        Box::new(Upd765::with_image(bootable_floppy_image())), // 2
    ];
    let map = DecodeMap {
        ports: vec![
            PortRange {
                ports: COM1_BASE..=COM1_TOP,
                device: 0,
            },
            PortRange {
                ports: 0x00..=0x0F,
                device: 1,
            },
            PortRange {
                ports: 0x80..=0x8F,
                device: 1,
            },
            PortRange {
                ports: 0x3F0..=0x3F7,
                device: 2,
            },
        ],
        mmio: Vec::new(),
    };
    let bus = Bus::new(devices, map);

    let cpu = X86JitCpu::new_real16(
        RAM_BYTES,
        BOOT_CS,
        FLOPPY_LOADER_IP,
        FLOPPY_LOADER,
        RealSegments::flat(BOOT_CS),
    )
    .context("building real-mode cpu")?;

    // No interrupt wiring: the DMA transfer is synchronous, so the loader jumps to
    // the boot sector without waiting on IRQ6; `None` idle limit halts on the
    // boot sector's `hlt`.
    let mut machine = Machine::new(
        cpu,
        bus,
        Box::new(NoInterruptController),
        QUANTUM,
        Vec::new(),
        None,
    )
    .with_dma_wirings(vec![DmaWiring {
        requester: 2,
        controller: 1,
    }]);

    machine.run().context("running machine")?;

    Ok(drain_serial(&mut machine, 0))
}

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
