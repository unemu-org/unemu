// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real-mode timer-interrupt boot golden test (§5, §7, §10).
//!
//! This is the M3 milestone: the first time the machine takes a **hardware
//! interrupt**. A hand-encoded **real-mode** guest runs on the [`AtChipset`]
//! (16550 UART + 8259 PIC + 8253 PIT) through the real `X86JitCpu` in
//! `CpuMode::Real16`. It:
//!
//! 1. writes an IRQ0 handler address into IVT entry 8 (`0x0000:0x0020`);
//! 2. programs the 8259 PIC (ICW1–ICW4, vector base `0x08`, IRQ0 unmasked);
//! 3. programs PIT counter 0 (mode 2, divisor 20);
//! 4. `sti`, then loops on `hlt`.
//!
//! The IRQ0 handler writes a marker byte `.` to the UART (COM1 THR, `0x3F8`),
//! sends EOI (`out 0x20, 0x20`) and `iret`s. After five markers it masks all
//! IRQs, so the interrupt source falls quiescent and the machine terminates.
//!
//! The captured serial output is the golden (`b"....."`), fully deterministic
//! because nothing in the path reads a wall clock: virtual time is the
//! retired-instruction count and the PIT ticks off it (§6).
//!
//! The segment registers and stack pointer are seeded by the embedder
//! (`X86JitCpu::new_real16`) because the `x86jit` core does not lift
//! `mov Sreg, r16`; the guest starts straight at device setup.
//!
//! ## Pinned CPU revision
//!
//! Golden traces are only valid against a known CPU revision (AGENTS.md). This
//! golden was recorded against `x86jit-core` (branch `pc-emulator`) rev
//! **`a4f6ae598118c1b204837c6ba4ef5b59cf31a85f`** — the rev pinned in
//! `Cargo.lock`. Re-baseline the golden when that rev moves.

use cli::{run_boot, BOOT_HANDLER_OFFSET, BOOT_PROGRAM};
use testkit::golden::assert_bytes_eq;

/// The committed golden: five `.` markers, one per serviced timer interrupt.
const GOLDEN: &[u8] = b".....";

#[test]
fn timer_interrupt_boot_matches_golden() {
    let serial = run_boot(BOOT_PROGRAM).expect("machine run");
    assert_bytes_eq(&serial, GOLDEN);
}

/// Guard the hand-encoded program bytes so an accidental edit is caught
/// independently of the run, and document every instruction's exact bytes.
///
/// Loaded at linear `0x7C00` (segment `0x0000`, IP `0x7C00`; CS = 0, so a linear
/// address equals its offset). The handler sits immediately after the main code
/// at `0x7C30` — the value poked into IVT entry 8.
///
/// ```text
/// main @ 0x7C00 (segments + SP seeded by the embedder):
///   7C00: C7 06 20 00 30 7C  mov word [0x0020], 0x7C30   ; IVT[8].IP = handler
///   7C06: C7 06 22 00 00 00  mov word [0x0022], 0x0000   ; IVT[8].CS = 0
///   7C0C: B0 11              mov al, 0x11                 ; ICW1: init + ICW4
///   7C0E: E6 20              out 0x20, al                 ;   -> PIC command
///   7C10: B0 08              mov al, 0x08                 ; ICW2: vector base 0x08
///   7C12: E6 21              out 0x21, al                 ;   -> PIC data
///   7C14: B0 04              mov al, 0x04                 ; ICW3: cascade (ignored)
///   7C16: E6 21              out 0x21, al
///   7C18: B0 01              mov al, 0x01                 ; ICW4: 8086 mode
///   7C1A: E6 21              out 0x21, al
///   7C1C: B0 FE              mov al, 0xFE                 ; OCW1: unmask only IRQ0
///   7C1E: E6 21              out 0x21, al
///   7C20: B0 34              mov al, 0x34                 ; PIT ch0: lo/hi, mode 2
///   7C22: E6 43              out 0x43, al                 ;   -> PIT command
///   7C24: B0 14              mov al, 0x14                 ; divisor low = 20
///   7C26: E6 40              out 0x40, al                 ;   -> PIT ch0
///   7C28: B0 00              mov al, 0x00                 ; divisor high = 0
///   7C2A: E6 40              out 0x40, al
///   7C2C: FB                 sti                          ; enable interrupts
///   7C2D: F4                 hlt                          ; wait for IRQ0  <- target
///   7C2E: EB FD              jmp $-3 (-> 0x7C2D)          ; hlt loop
/// handler @ 0x7C30:
///   7C30: 50                 push ax
///   7C31: 52                 push dx
///   7C32: FE 06 00 05        inc byte [0x0500]            ; ++interrupt counter
///   7C36: B0 2E              mov al, 0x2E ('.')           ; marker byte
///   7C38: BA F8 03           mov dx, 0x03F8               ; COM1 THR (0x3F8 > imm8)
///   7C3B: EE                 out dx, al                   ; transmit marker
///   7C3C: 80 3E 00 05 05     cmp byte [0x0500], 0x05      ; reached 5?
///   7C41: 75 04              jne +4 (-> 0x7C47)           ; if not, skip mask
///   7C43: B0 FF              mov al, 0xFF                 ; mask ALL IRQs
///   7C45: E6 21              out 0x21, al                 ;   -> PIC data
///   7C47: B0 20              mov al, 0x20                 ; EOI
///   7C49: E6 20              out 0x20, al                 ;   -> PIC command
///   7C4B: 5A                 pop dx
///   7C4C: 58                 pop ax
///   7C4D: CF                 iret
/// ```
#[test]
fn boot_program_bytes_are_the_documented_encoding() {
    #[rustfmt::skip]
    let expected: &[u8] = &[
        // main @ 0x7C00
        0xC7, 0x06, 0x20, 0x00, 0x30, 0x7C, // mov word [0x0020],0x7C30
        0xC7, 0x06, 0x22, 0x00, 0x00, 0x00, // mov word [0x0022],0x0000
        0xB0, 0x11, 0xE6, 0x20,             // mov al,0x11 ; out 0x20,al
        0xB0, 0x08, 0xE6, 0x21,             // mov al,0x08 ; out 0x21,al
        0xB0, 0x04, 0xE6, 0x21,             // mov al,0x04 ; out 0x21,al
        0xB0, 0x01, 0xE6, 0x21,             // mov al,0x01 ; out 0x21,al
        0xB0, 0xFE, 0xE6, 0x21,             // mov al,0xFE ; out 0x21,al
        0xB0, 0x34, 0xE6, 0x43,             // mov al,0x34 ; out 0x43,al
        0xB0, 0x14, 0xE6, 0x40,             // mov al,0x14 ; out 0x40,al
        0xB0, 0x00, 0xE6, 0x40,             // mov al,0x00 ; out 0x40,al
        0xFB,                               // sti
        0xF4,                               // hlt
        0xEB, 0xFD,                         // jmp $-3
        // handler @ 0x7C30
        0x50,                               // push ax
        0x52,                               // push dx
        0xFE, 0x06, 0x00, 0x05,             // inc byte [0x0500]
        0xB0, 0x2E,                         // mov al,0x2E
        0xBA, 0xF8, 0x03,                   // mov dx,0x03F8
        0xEE,                               // out dx,al
        0x80, 0x3E, 0x00, 0x05, 0x05,       // cmp byte [0x0500],0x05
        0x75, 0x04,                         // jne +4
        0xB0, 0xFF, 0xE6, 0x21,             // mov al,0xFF ; out 0x21,al
        0xB0, 0x20, 0xE6, 0x20,             // mov al,0x20 ; out 0x20,al
        0x5A,                               // pop dx
        0x58,                               // pop ax
        0xCF,                               // iret
    ];
    assert_eq!(BOOT_PROGRAM, expected);
    // The handler starts right after the 48-byte main program.
    assert_eq!(BOOT_HANDLER_OFFSET, 0x7C00 + 48);
    // The IVT-entry-8 immediate the main program pokes must equal the handler
    // offset: bytes at index 4..6 are the little-endian handler address.
    let ivt_imm = u16::from_le_bytes([BOOT_PROGRAM[4], BOOT_PROGRAM[5]]);
    assert_eq!(ivt_imm, BOOT_HANDLER_OFFSET);
}
