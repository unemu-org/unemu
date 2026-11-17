// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real-mode keyboard-interrupt golden test (§5, §7, §10).
//!
//! The machine takes a **keyboard hardware interrupt** (IRQ1). A hand-encoded
//! **real-mode** guest runs on the PC/AT machine shape (UART + 8259 PIC + 8253 PIT +
//! 8042 keyboard + VGA) through the real `X86JitCpu` in `CpuMode::Real16`. It:
//!
//! 1. writes an IRQ1 handler address into IVT entry 9 (`0x0000:0x0024`);
//! 2. programs the 8259 PIC (ICW1–ICW4, vector base `0x08`, IRQ1 unmasked);
//! 3. enables keyboard interrupts in the 8042 (command byte bit 0);
//! 4. `sti`, then loops on `hlt`.
//!
//! A single scancode is queued into the keyboard before the run. Once the guest
//! enables keyboard interrupts the 8042 holds IRQ1 high; the machine forwards
//! its level to the PIC, which detects edges and requires IR high until INTA
//! (Intel 8259A datasheet, Edge and Level Triggered Modes). The machine injects
//! the acknowledged keyboard vector. The handler reads the scancode
//! from port `0x60`, emits it to the UART (COM1 THR, `0x3F8`), sends EOI
//! (`out 0x20, 0x20`), masks all IRQs, and `iret`s. With the source masked the
//! machine falls quiescent and terminates.
//!
//! The captured serial output is the golden (the queued scancode), fully
//! deterministic because nothing in the path reads a wall clock (§6).
//!
//! ## Pinned CPU revision
//!
//! Golden traces are only valid against a known CPU revision (AGENTS.md); this
//! path runs the same pinned `x86jit-core` rev as the timer boot golden — the rev
//! in `Cargo.lock`. Re-baseline when that rev moves.

use pc_harness::{run_keyboard, KBD_HANDLER_OFFSET, KBD_PROGRAM, KBD_SCANCODE};
use testkit::golden::assert_bytes_eq;

#[test]
fn keyboard_interrupt_emits_queued_scancode() {
    let serial = run_keyboard(KBD_PROGRAM, &[KBD_SCANCODE]).expect("machine run");
    // The IRQ1 handler transmits exactly the one queued scancode to COM1.
    assert_bytes_eq(&serial, &[KBD_SCANCODE]);
}

/// Guard the hand-encoded program bytes so an accidental edit is caught
/// independently of the run, and document every instruction's exact bytes.
///
/// Loaded at linear `0x7C00` (segment `0x0000`, IP `0x7C00`; CS = 0, so a linear
/// address equals its offset). The handler sits immediately after the 44-byte
/// main code at `0x7C2C` — the value poked into IVT entry 9.
///
/// ```text
/// main @ 0x7C00 (segments + SP seeded by the embedder):
///   7C00: C7 06 24 00 2C 7C  mov word [0x0024], 0x7C2C   ; IVT[9].IP = handler
///   7C06: C7 06 26 00 00 00  mov word [0x0026], 0x0000   ; IVT[9].CS = 0
///   7C0C: B0 11              mov al, 0x11                 ; ICW1: init + ICW4
///   7C0E: E6 20              out 0x20, al                 ;   -> PIC command
///   7C10: B0 08              mov al, 0x08                 ; ICW2: vector base 0x08
///   7C12: E6 21              out 0x21, al                 ;   -> PIC data
///   7C14: B0 04              mov al, 0x04                 ; ICW3: cascade (ignored)
///   7C16: E6 21              out 0x21, al
///   7C18: B0 01              mov al, 0x01                 ; ICW4: 8086 mode
///   7C1A: E6 21              out 0x21, al
///   7C1C: B0 FD              mov al, 0xFD                 ; OCW1: unmask only IRQ1
///   7C1E: E6 21              out 0x21, al
///   7C20: B0 60              mov al, 0x60                 ; 8042: write command byte
///   7C22: E6 64              out 0x64, al                 ;   -> keyboard command port
///   7C24: B0 01              mov al, 0x01                 ; command byte: KBD int enable
///   7C26: E6 60              out 0x60, al                 ;   -> keyboard data port
///   7C28: FB                 sti                          ; enable interrupts
///   7C29: F4                 hlt                          ; wait for IRQ1  <- target
///   7C2A: EB FD              jmp $-3 (-> 0x7C29)          ; hlt loop
/// handler @ 0x7C2C:
///   7C2C: 50                 push ax
///   7C2D: 52                 push dx
///   7C2E: BA 60 00           mov dx, 0x0060               ; keyboard data port
///   7C31: EC                 in al, dx                    ; read scancode
///   7C32: BA F8 03           mov dx, 0x03F8               ; COM1 THR
///   7C35: EE                 out dx, al                   ; transmit scancode
///   7C36: B0 FF              mov al, 0xFF                 ; mask ALL IRQs
///   7C38: E6 21              out 0x21, al                 ;   -> PIC data
///   7C3A: B0 20              mov al, 0x20                 ; EOI
///   7C3C: E6 20              out 0x20, al                 ;   -> PIC command
///   7C3E: 5A                 pop dx
///   7C3F: 58                 pop ax
///   7C40: CF                 iret
/// ```
#[test]
fn keyboard_program_bytes_are_the_documented_encoding() {
    #[rustfmt::skip]
    let expected: &[u8] = &[
        // main @ 0x7C00
        0xC7, 0x06, 0x24, 0x00, 0x2C, 0x7C, // mov word [0x0024],0x7C2C
        0xC7, 0x06, 0x26, 0x00, 0x00, 0x00, // mov word [0x0026],0x0000
        0xB0, 0x11, 0xE6, 0x20,             // mov al,0x11 ; out 0x20,al
        0xB0, 0x08, 0xE6, 0x21,             // mov al,0x08 ; out 0x21,al
        0xB0, 0x04, 0xE6, 0x21,             // mov al,0x04 ; out 0x21,al
        0xB0, 0x01, 0xE6, 0x21,             // mov al,0x01 ; out 0x21,al
        0xB0, 0xFD, 0xE6, 0x21,             // mov al,0xFD ; out 0x21,al
        0xB0, 0x60, 0xE6, 0x64,             // mov al,0x60 ; out 0x64,al
        0xB0, 0x01, 0xE6, 0x60,             // mov al,0x01 ; out 0x60,al
        0xFB,                               // sti
        0xF4,                               // hlt
        0xEB, 0xFD,                         // jmp $-3
        // handler @ 0x7C2C
        0x50,                               // push ax
        0x52,                               // push dx
        0xBA, 0x60, 0x00,                   // mov dx,0x0060
        0xEC,                               // in al,dx
        0xBA, 0xF8, 0x03,                   // mov dx,0x03F8
        0xEE,                               // out dx,al
        0xB0, 0xFF, 0xE6, 0x21,             // mov al,0xFF ; out 0x21,al
        0xB0, 0x20, 0xE6, 0x20,             // mov al,0x20 ; out 0x20,al
        0x5A,                               // pop dx
        0x58,                               // pop ax
        0xCF,                               // iret
    ];
    assert_eq!(KBD_PROGRAM, expected);
    // The handler starts right after the 44-byte main program.
    assert_eq!(KBD_HANDLER_OFFSET, 0x7C00 + 44);
    // The IVT-entry-9 immediate the main program pokes must equal the handler
    // offset: bytes at index 4..6 are the little-endian handler address.
    let ivt_imm = u16::from_le_bytes([KBD_PROGRAM[4], KBD_PROGRAM[5]]);
    assert_eq!(ivt_imm, KBD_HANDLER_OFFSET);
}
