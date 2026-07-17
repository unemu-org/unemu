// SPDX-License-Identifier: AGPL-3.0-or-later
//! Serial golden test (§10).
//!
//! A **Long64** guest writes the ASCII bytes of "hello" to COM1 THR (port
//! `0x3F8`) via `out dx, al`, then halts, through the real `X86JitCpu` + `Bus` +
//! `Uart16550`. The captured serial output is the golden; it is fully
//! deterministic because nothing in the path reads a wall clock (§6).
//!
//! The guest program is hand-encoded (no `nasm` in the toolchain) and verified
//! against the `x86jit-core` iced disassembler; see [`cli::HELLO_PROGRAM`].

use cli::{run_serial, HELLO_PROGRAM};
use testkit::golden::assert_bytes_eq;

/// The committed golden: the exact serial output the "hello" program must emit.
const GOLDEN: &[u8] = b"hello";

#[test]
fn hello_over_com1_matches_golden() {
    let serial = run_serial(HELLO_PROGRAM).expect("machine run");
    assert_bytes_eq(&serial, GOLDEN);
}

/// Guard the hand-encoded program bytes so an accidental edit to the encoding is
/// caught independently of the run (documents the exact opcodes).
#[test]
fn hello_program_bytes_are_the_documented_encoding() {
    #[rustfmt::skip]
    let expected: &[u8] = &[
        0x66, 0xBA, 0xF8, 0x03, // mov dx, 0x3F8
        0xB0, b'h', 0xEE,       // mov al,'h' ; out dx,al
        0xB0, b'e', 0xEE,       // mov al,'e' ; out dx,al
        0xB0, b'l', 0xEE,       // mov al,'l' ; out dx,al
        0xB0, b'l', 0xEE,       // mov al,'l' ; out dx,al
        0xB0, b'o', 0xEE,       // mov al,'o' ; out dx,al
        0xF4,                   // hlt
    ];
    assert_eq!(HELLO_PROGRAM, expected);
}
