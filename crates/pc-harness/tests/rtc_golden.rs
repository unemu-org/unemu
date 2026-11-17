// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real-mode RTC (IRQ8) interrupt golden test (§5, §7, §10).
//!
//! The first end-to-end path where an interrupt reaches the CPU **across the
//! master/slave 8259 cascade**. A hand-encoded **real-mode** guest runs on the
//! PC/AT machine shape (UART + cascaded 8259 pair + PIT + 8042 + VGA + MC146818 RTC)
//! through the real `X86JitCpu` in `CpuMode::Real16`. It:
//!
//! 1. writes an IRQ8 handler address into IVT entry `0x70` (`0x0000:0x01C0`);
//! 2. programs the master 8259 (ICW1–ICW4, vector base `0x08`) and unmasks IRQ2
//!    for the cascade;
//! 3. programs the slave 8259 (ICW1–ICW4, vector base `0x70`) and unmasks IRQ8;
//! 4. enables the RTC's update-ended interrupt (register B bit 4, UIE);
//! 5. `sti`, then loops on `hlt`.
//!
//! The RTC is built with a small per-second tick ratio, so one RTC second ticks
//! a few dozen PIT clocks into the `hlt` loop. The RTC asserts IRQ8; the machine
//! forwards its level to the slave PIC, whose INT level drives master IRQ2
//! (Intel 8259A datasheet, Cascading the 8259A and Edge and Level Triggered Modes).
//! The machine injects vector `0x70`. The handler writes a marker byte `R` to the UART
//! (COM1 THR, `0x3F8`), ends the cascaded interrupt with an EOI to **both** the
//! slave (`0xA0`) and the master (`0x20`) — the real cascaded-EOI sequence —
//! reads register C of the RTC (`0x71`) to deassert IRQ8, masks all master IRQs,
//! and `iret`s. With the cascade line masked the source falls quiescent and the
//! machine terminates.
//!
//! The captured serial output is the golden (`b"R"`), fully deterministic because
//! nothing in the path reads a wall clock: virtual time is the retired-instruction
//! count and the RTC ticks off it (§6).
//!
//! The segment registers and stack pointer are seeded by the embedder
//! (`X86JitCpu::new_real16_bare`) because the `x86jit` core does not lift
//! `mov Sreg, r16`; the guest starts straight at device setup.
//!
//! ## Pinned CPU revision
//!
//! Golden traces are only valid against a known CPU revision (AGENTS.md); this
//! path runs the same pinned `x86jit-core` rev as the timer boot golden — the rev
//! in `Cargo.lock`. Re-baseline when that rev moves.

use pc_harness::{run_rtc, RTC_HANDLER_OFFSET, RTC_MARKER, RTC_PROGRAM};
use testkit::golden::assert_bytes_eq;

#[test]
fn rtc_update_interrupt_emits_marker_across_cascade() {
    let serial = run_rtc(RTC_PROGRAM).expect("machine run");
    // Exactly one marker: the IRQ8 handler runs once, then masks the cascade so
    // the machine falls quiescent. Proving one byte proves the IRQ8 → slave →
    // master IRQ2 cascade path end to end and that it terminates (no spin).
    assert_bytes_eq(&serial, &[RTC_MARKER]);
}

/// MC146818A datasheet, Periodic Interrupt Selection and Registers A–C:
/// PIE alone must wake HLT through IRQ8, with UIE/AIE disabled.
#[test]
fn fidelity_periodic_interrupt_wakes_hlt_across_cascade() {
    let mut program = RTC_PROGRAM.to_vec();
    program[4..6].copy_from_slice(&(RTC_HANDLER_OFFSET + 8).to_le_bytes());
    assert_eq!(program[57], 0x12);
    program[57] = 0x42; // 24-hour mode + PIE, without UIE or AIE.
    program.splice(
        52..52,
        [
            0xB0, 0x0A, 0xE6, 0x70, // select register A
            0xB0, 0x2F, 0xE6, 0x71, // 32,768 Hz time base, 2 Hz periodic tap
        ],
    );
    let serial = run_rtc(&program).expect("periodic machine run");
    assert_bytes_eq(&serial, &[RTC_MARKER]);
}

/// Guard the hand-encoded program bytes so an accidental edit is caught
/// independently of the run, and document every instruction's exact bytes.
///
/// Loaded at linear `0x7C00` (segment `0x0000`, IP `0x7C00`; CS = 0, so a linear
/// address equals its offset). The handler sits immediately after the 64-byte
/// main code at `0x7C40` — the value poked into IVT entry `0x70` (vector `0x70`
/// → IVT offset `0x70 * 4 = 0x01C0`).
///
/// ```text
/// main @ 0x7C00 (segments + SP seeded by the embedder):
///   7C00: C7 06 C0 01 40 7C  mov word [0x01C0], 0x7C40   ; IVT[0x70].IP = handler
///   7C06: C7 06 C2 01 00 00  mov word [0x01C2], 0x0000   ; IVT[0x70].CS = 0
///   7C0C: B0 11              mov al, 0x11                 ; master ICW1: init + ICW4
///   7C0E: E6 20              out 0x20, al                 ;   -> master command
///   7C10: B0 08              mov al, 0x08                 ; master ICW2: vector base 0x08
///   7C12: E6 21              out 0x21, al                 ;   -> master data
///   7C14: B0 04              mov al, 0x04                 ; master ICW3: slave on IRQ2 (bit2)
///   7C16: E6 21              out 0x21, al
///   7C18: B0 01              mov al, 0x01                 ; master ICW4: 8086 mode
///   7C1A: E6 21              out 0x21, al
///   7C1C: B0 FB              mov al, 0xFB                 ; master IMR: unmask only IRQ2
///   7C1E: E6 21              out 0x21, al
///   7C20: B0 11              mov al, 0x11                 ; slave ICW1: init + ICW4
///   7C22: E6 A0              out 0xA0, al                 ;   -> slave command
///   7C24: B0 70              mov al, 0x70                 ; slave ICW2: vector base 0x70
///   7C26: E6 A1              out 0xA1, al                 ;   -> slave data
///   7C28: B0 02              mov al, 0x02                 ; slave ICW3: identity 2
///   7C2A: E6 A1              out 0xA1, al
///   7C2C: B0 01              mov al, 0x01                 ; slave ICW4: 8086 mode
///   7C2E: E6 A1              out 0xA1, al
///   7C30: B0 FE              mov al, 0xFE                 ; slave IMR: unmask only IRQ8
///   7C32: E6 A1              out 0xA1, al
///   7C34: B0 0B              mov al, 0x0B                 ; select RTC register B
///   7C36: E6 70              out 0x70, al                 ;   -> RTC index (NMI enabled)
///   7C38: B0 12              mov al, 0x12                 ; reg B = 24-hour | UIE
///   7C3A: E6 71              out 0x71, al                 ;   -> RTC data
///   7C3C: FB                 sti                          ; enable interrupts
///   7C3D: F4                 hlt                          ; wait for IRQ8  <- target
///   7C3E: EB FD              jmp $-3 (-> 0x7C3D)          ; hlt loop
/// handler @ 0x7C40:
///   7C40: 50                 push ax
///   7C41: 52                 push dx
///   7C42: B0 52              mov al, 0x52 ('R')           ; marker byte
///   7C44: BA F8 03           mov dx, 0x03F8               ; COM1 THR
///   7C47: EE                 out dx, al                   ; transmit marker
///   7C48: B0 20              mov al, 0x20                 ; EOI (non-specific)
///   7C4A: E6 A0              out 0xA0, al                 ;   -> slave command (EOI slave)
///   7C4C: B0 20              mov al, 0x20                 ; EOI (non-specific)
///   7C4E: E6 20              out 0x20, al                 ;   -> master command (EOI master)
///   7C50: B0 0C              mov al, 0x0C                 ; select RTC register C
///   7C52: E6 70              out 0x70, al                 ;   -> RTC index
///   7C54: BA 71 00           mov dx, 0x0071               ; RTC data port
///   7C57: EC                 in al, dx                    ; read reg C -> deassert IRQ8
///   7C58: B0 FF              mov al, 0xFF                 ; mask ALL master IRQs
///   7C5A: E6 21              out 0x21, al                 ;   -> master data
///   7C5C: 5A                 pop dx
///   7C5D: 58                 pop ax
///   7C5E: CF                 iret
/// ```
#[test]
fn rtc_program_bytes_are_the_documented_encoding() {
    #[rustfmt::skip]
    let expected: &[u8] = &[
        // main @ 0x7C00
        0xC7, 0x06, 0xC0, 0x01, 0x40, 0x7C, // mov word [0x01C0],0x7C40
        0xC7, 0x06, 0xC2, 0x01, 0x00, 0x00, // mov word [0x01C2],0x0000
        0xB0, 0x11, 0xE6, 0x20,             // mov al,0x11 ; out 0x20,al  (master ICW1)
        0xB0, 0x08, 0xE6, 0x21,             // mov al,0x08 ; out 0x21,al  (master ICW2)
        0xB0, 0x04, 0xE6, 0x21,             // mov al,0x04 ; out 0x21,al  (master ICW3)
        0xB0, 0x01, 0xE6, 0x21,             // mov al,0x01 ; out 0x21,al  (master ICW4)
        0xB0, 0xFB, 0xE6, 0x21,             // mov al,0xFB ; out 0x21,al  (master IMR)
        0xB0, 0x11, 0xE6, 0xA0,             // mov al,0x11 ; out 0xA0,al  (slave ICW1)
        0xB0, 0x70, 0xE6, 0xA1,             // mov al,0x70 ; out 0xA1,al  (slave ICW2)
        0xB0, 0x02, 0xE6, 0xA1,             // mov al,0x02 ; out 0xA1,al  (slave ICW3)
        0xB0, 0x01, 0xE6, 0xA1,             // mov al,0x01 ; out 0xA1,al  (slave ICW4)
        0xB0, 0xFE, 0xE6, 0xA1,             // mov al,0xFE ; out 0xA1,al  (slave IMR)
        0xB0, 0x0B, 0xE6, 0x70,             // mov al,0x0B ; out 0x70,al  (select reg B)
        0xB0, 0x12, 0xE6, 0x71,             // mov al,0x12 ; out 0x71,al  (reg B = 24h|UIE)
        0xFB,                               // sti
        0xF4,                               // hlt
        0xEB, 0xFD,                         // jmp $-3
        // handler @ 0x7C40
        0x50,                               // push ax
        0x52,                               // push dx
        0xB0, 0x52,                         // mov al,0x52
        0xBA, 0xF8, 0x03,                   // mov dx,0x03F8
        0xEE,                               // out dx,al
        0xB0, 0x20, 0xE6, 0xA0,             // mov al,0x20 ; out 0xA0,al  (EOI slave)
        0xB0, 0x20, 0xE6, 0x20,             // mov al,0x20 ; out 0x20,al  (EOI master)
        0xB0, 0x0C, 0xE6, 0x70,             // mov al,0x0C ; out 0x70,al  (select reg C)
        0xBA, 0x71, 0x00,                   // mov dx,0x0071
        0xEC,                               // in al,dx
        0xB0, 0xFF, 0xE6, 0x21,             // mov al,0xFF ; out 0x21,al  (mask master)
        0x5A,                               // pop dx
        0x58,                               // pop ax
        0xCF,                               // iret
    ];
    assert_eq!(RTC_PROGRAM, expected);
    // The handler starts right after the 64-byte main program.
    assert_eq!(RTC_HANDLER_OFFSET, 0x7C00 + 64);
    // The IVT-entry-0x70 immediate the main program pokes must equal the handler
    // offset: bytes at index 4..6 are the little-endian handler address.
    let ivt_imm = u16::from_le_bytes([RTC_PROGRAM[4], RTC_PROGRAM[5]]);
    assert_eq!(ivt_imm, RTC_HANDLER_OFFSET);
    // The marker the handler transmits is the byte the golden asserts.
    assert_eq!(RTC_MARKER, b'R');
}
