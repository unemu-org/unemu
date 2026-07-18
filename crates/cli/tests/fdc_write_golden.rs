// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real-mode floppy WRITE golden test (§7, §7.7, §7.8, §10).
//!
//! The first end-to-end path where a guest **writes to media**. A hand-encoded
//! real-mode guest runs on the PC/AT machine shape (UART + cascaded 8259 pair +
//! PIT + 8042 + VGA + CMOS/RTC, plus an 8237 DMA controller and a µPD765 floppy
//! controller backed by a **blank writable** floppy) through the real `X86JitCpu`
//! in `CpuMode::Real16`. It:
//!
//! 1. brings the µPD765 out of reset (DOR: not-reset + DMA-enable + motor);
//! 2. programs 8237 channel 2 for a **memory→device** transfer of one 512-byte
//!    sector from a preloaded source buffer holding a distinctive pattern, then
//!    issues **WRITE DATA** for drive 0 at C=0/H=0/R=1 and drains the result phase;
//! 3. programs 8237 channel 2 for a **device→memory** transfer into a separate
//!    read-back buffer, then issues **READ DATA** for the same CHS;
//! 4. compares the read-back bytes against the pattern and, on a match, emits the
//!    marker byte `W` to the UART (COM1 THR, `0x3F8`), then busy-spins on a port
//!    read until the run's retired-instruction cap stops it.
//!
//! The machine services the DMA on its run boundaries (both directions), exactly
//! as the floppy READ golden relies on — so the write is committed and the sector
//! is read back within the same run. Because the floppy starts **blank** (all
//! zeroes) and the compared pattern bytes are all non-zero, a match can only mean
//! the guest's own write reached media: a broken or no-op write leaves the sector
//! zero, the read-back mismatches, and no marker is emitted. The captured serial is
//! the golden (`b"W"`), fully deterministic because nothing in the path reads a
//! wall clock (§6).
//!
//! The segment registers and stack pointer are seeded by the embedder
//! (`X86JitCpu::new_real16_bare`) because the `x86jit` core does not lift
//! `mov Sreg, r16`; the guest starts straight at device setup.
//!
//! ## Pinned CPU revision
//!
//! Golden traces are only valid against a known CPU revision (AGENTS.md); this
//! path runs the same pinned `x86jit-core` rev as the other CPU-driven goldens —
//! the rev in `Cargo.lock`. Re-baseline when that rev moves.

use cli::{fdc_write_pattern, run_fdc_write, FDC_WRITE_MARKER, FDC_WRITE_PROGRAM};
use testkit::golden::assert_bytes_eq;

#[test]
fn fdc_write_commits_a_sector_and_reads_it_back() {
    let serial = run_fdc_write(FDC_WRITE_PROGRAM).expect("machine run");
    // Exactly one marker: the read-back sector matched the written pattern, which
    // (against a blank floppy) proves the WRITE DATA reached media through the
    // symmetric DMA seam.
    assert_bytes_eq(&serial, &[FDC_WRITE_MARKER]);
}

/// Guard the hand-encoded program bytes so an accidental edit is caught
/// independently of the run, and document every instruction's exact bytes.
///
/// Loaded at linear `0x7C00` (segment `0x0000`, IP `0x7C00`; CS = 0, so a linear
/// address equals its offset). The source pattern is preloaded at `0x0600`; the
/// read-back buffer is at `0x0A00`.
///
/// ```text
///   7C00: B0 1C              mov al,0x1C                  ; DOR: not-reset|DMA|motor0
///   7C02: BA F2 03           mov dx,0x03F2                ; DOR port
///   7C05: EE                 out dx,al
///   ; --- 8237 ch2: memory -> device (read), src 0x0600, 512 bytes ---
///   7C06: B0 06 E6 0A        mov al,0x06 ; out 0x0A,al    ; mask ch2
///   7C0A: B0 00 E6 0C        mov al,0x00 ; out 0x0C,al    ; clear flip-flop
///   7C0E: B0 4A E6 0B        mov al,0x4A ; out 0x0B,al    ; mode: ch2 read single
///   7C12: B0 00 E6 04        mov al,0x00 ; out 0x04,al    ; addr low
///   7C16: B0 06 E6 04        mov al,0x06 ; out 0x04,al    ; addr high -> 0x0600
///   7C1A: B0 00 E6 81        mov al,0x00 ; out 0x81,al    ; page ch2
///   7C1E: B0 FF E6 05        mov al,0xFF ; out 0x05,al    ; count low
///   7C22: B0 01 E6 05        mov al,0x01 ; out 0x05,al    ; count high -> 512
///   7C26: B0 02 E6 0A        mov al,0x02 ; out 0x0A,al    ; unmask ch2
///   ; --- WRITE DATA (0x05): drive 0, C=0 H=0 R=1 N=2 EOT=1 (DATA @ 0x3F5) ---
///   7C2A: BA F5 03           mov dx,0x03F5                ; DATA port
///   7C2D: B0 05 EE           mov al,0x05 ; out dx,al      ; WRITE DATA
///   7C30: B0 00 EE           mov al,0x00 ; out dx,al      ; drive/head
///   7C33: B0 00 EE           mov al,0x00 ; out dx,al      ; cylinder
///   7C36: B0 00 EE           mov al,0x00 ; out dx,al      ; head
///   7C39: B0 01 EE           mov al,0x01 ; out dx,al      ; sector R=1
///   7C3C: B0 02 EE           mov al,0x02 ; out dx,al      ; N=2 (512-byte sector)
///   7C3F: B0 01 EE           mov al,0x01 ; out dx,al      ; EOT=1
///   7C42: B0 1B EE           mov al,0x1B ; out dx,al      ; GPL
///   7C45: B0 FF EE           mov al,0xFF ; out dx,al      ; DTL
///   7C48: EC x7              in al,dx  (x7)               ; drain WRITE result phase
///   ; --- 8237 ch2: device -> memory (write), dst 0x0A00, 512 bytes ---
///   7C4F: B0 06 E6 0A        mov al,0x06 ; out 0x0A,al    ; mask ch2
///   7C53: B0 00 E6 0C        mov al,0x00 ; out 0x0C,al    ; clear flip-flop
///   7C57: B0 46 E6 0B        mov al,0x46 ; out 0x0B,al    ; mode: ch2 write single
///   7C5B: B0 00 E6 04        mov al,0x00 ; out 0x04,al    ; addr low
///   7C5F: B0 0A E6 04        mov al,0x0A ; out 0x04,al    ; addr high -> 0x0A00
///   7C63: B0 00 E6 81        mov al,0x00 ; out 0x81,al    ; page ch2
///   7C67: B0 FF E6 05        mov al,0xFF ; out 0x05,al    ; count low
///   7C6B: B0 01 E6 05        mov al,0x01 ; out 0x05,al    ; count high -> 512
///   7C6F: B0 02 E6 0A        mov al,0x02 ; out 0x0A,al    ; unmask ch2
///   ; --- READ DATA (0x06): same CHS, streams the sector into 0x0A00 ---
///   7C73: BA F5 03           mov dx,0x03F5                ; DATA port
///   7C76: B0 06 EE           mov al,0x06 ; out dx,al      ; READ DATA
///   7C79: B0 00 EE           mov al,0x00 ; out dx,al      ; drive/head
///   7C7C: B0 00 EE           mov al,0x00 ; out dx,al      ; cylinder
///   7C7F: B0 00 EE           mov al,0x00 ; out dx,al      ; head
///   7C82: B0 01 EE           mov al,0x01 ; out dx,al      ; sector R=1
///   7C85: B0 02 EE           mov al,0x02 ; out dx,al      ; N=2
///   7C88: B0 01 EE           mov al,0x01 ; out dx,al      ; EOT=1
///   7C8B: B0 1B EE           mov al,0x1B ; out dx,al      ; GPL
///   7C8E: B0 FF EE           mov al,0xFF ; out dx,al      ; DTL
///   ; --- compare read-back [0x0A00..0x0A04] to the pattern, emit 'W' on match ---
///   7C91: 80 3E 00 0A C0     cmp byte [0x0A00],0xC0
///   7C96: 75 1B              jne end (+27)
///   7C98: 80 3E 01 0A C1     cmp byte [0x0A01],0xC1
///   7C9D: 75 14              jne end (+20)
///   7C9F: 80 3E 02 0A C2     cmp byte [0x0A02],0xC2
///   7CA4: 75 0D              jne end (+13)
///   7CA6: 80 3E 03 0A C3     cmp byte [0x0A03],0xC3
///   7CAB: 75 06              jne end (+6)
///   7CAD: B0 57              mov al,0x57 ('W')
///   7CAF: BA F8 03           mov dx,0x03F8                ; COM1 THR
///   7CB2: EE                 out dx,al                    ; transmit marker
///   7CB3: EC                 in al,dx                     ; I/O-yielding spin  <- end
///   7CB4: EB FD              jmp $-3                      ; back to the in
/// ```
#[test]
fn fdc_write_program_bytes_are_the_documented_encoding() {
    #[rustfmt::skip]
    let expected: &[u8] = &[
        0xB0, 0x1C, 0xBA, 0xF2, 0x03, 0xEE,             // DOR out of reset
        0xB0, 0x06, 0xE6, 0x0A,                         // mask ch2
        0xB0, 0x00, 0xE6, 0x0C,                         // clear flip-flop
        0xB0, 0x4A, 0xE6, 0x0B,                         // mode: ch2 read single
        0xB0, 0x00, 0xE6, 0x04,                         // addr low
        0xB0, 0x06, 0xE6, 0x04,                         // addr high -> 0x0600
        0xB0, 0x00, 0xE6, 0x81,                         // page ch2
        0xB0, 0xFF, 0xE6, 0x05,                         // count low
        0xB0, 0x01, 0xE6, 0x05,                         // count high -> 512
        0xB0, 0x02, 0xE6, 0x0A,                         // unmask ch2
        0xBA, 0xF5, 0x03,                               // mov dx,0x03F5 (DATA)
        0xB0, 0x05, 0xEE,                               // WRITE DATA
        0xB0, 0x00, 0xEE,                               // drive/head
        0xB0, 0x00, 0xEE,                               // cylinder
        0xB0, 0x00, 0xEE,                               // head
        0xB0, 0x01, 0xEE,                               // sector R=1
        0xB0, 0x02, 0xEE,                               // N=2
        0xB0, 0x01, 0xEE,                               // EOT=1
        0xB0, 0x1B, 0xEE,                               // GPL
        0xB0, 0xFF, 0xEE,                               // DTL
        0xEC, 0xEC, 0xEC, 0xEC, 0xEC, 0xEC, 0xEC,       // drain WRITE result (7 bytes)
        0xB0, 0x06, 0xE6, 0x0A,                         // mask ch2
        0xB0, 0x00, 0xE6, 0x0C,                         // clear flip-flop
        0xB0, 0x46, 0xE6, 0x0B,                         // mode: ch2 write single
        0xB0, 0x00, 0xE6, 0x04,                         // addr low
        0xB0, 0x0A, 0xE6, 0x04,                         // addr high -> 0x0A00
        0xB0, 0x00, 0xE6, 0x81,                         // page ch2
        0xB0, 0xFF, 0xE6, 0x05,                         // count low
        0xB0, 0x01, 0xE6, 0x05,                         // count high -> 512
        0xB0, 0x02, 0xE6, 0x0A,                         // unmask ch2
        0xBA, 0xF5, 0x03,                               // mov dx,0x03F5 (DATA)
        0xB0, 0x06, 0xEE,                               // READ DATA
        0xB0, 0x00, 0xEE,                               // drive/head
        0xB0, 0x00, 0xEE,                               // cylinder
        0xB0, 0x00, 0xEE,                               // head
        0xB0, 0x01, 0xEE,                               // sector R=1
        0xB0, 0x02, 0xEE,                               // N=2
        0xB0, 0x01, 0xEE,                               // EOT=1
        0xB0, 0x1B, 0xEE,                               // GPL
        0xB0, 0xFF, 0xEE,                               // DTL
        0x80, 0x3E, 0x00, 0x0A, 0xC0, 0x75, 0x1B,       // cmp [0x0A00],0xC0 ; jne end
        0x80, 0x3E, 0x01, 0x0A, 0xC1, 0x75, 0x14,       // cmp [0x0A01],0xC1 ; jne end
        0x80, 0x3E, 0x02, 0x0A, 0xC2, 0x75, 0x0D,       // cmp [0x0A02],0xC2 ; jne end
        0x80, 0x3E, 0x03, 0x0A, 0xC3, 0x75, 0x06,       // cmp [0x0A03],0xC3 ; jne end
        0xB0, 0x57, 0xBA, 0xF8, 0x03, 0xEE,             // mov al,'W' ; mov dx,0x3F8 ; out dx,al
        0xEC, 0xEB, 0xFD,                               // in al,dx ; jmp $-3 (I/O-yielding spin, end)
    ];
    assert_eq!(FDC_WRITE_PROGRAM, expected);

    // The compare immediates must equal the pattern's first four bytes, or the
    // guest would look for the wrong bytes and the golden could pass on a broken
    // write. Every compared byte must also be non-zero, so a blank floppy cannot
    // spoof the match.
    let pattern = fdc_write_pattern();
    assert_eq!(pattern.len(), 512);
    // Immediates sit at the last byte of each 5-byte `cmp byte [imm16], imm8`
    // (the four compares begin at program offsets 145, 152, 159, 166).
    let compare_imms = [expected[149], expected[156], expected[163], expected[170]];
    assert_eq!(
        compare_imms,
        [pattern[0], pattern[1], pattern[2], pattern[3]]
    );
    assert!(
        compare_imms.iter().all(|&b| b != 0),
        "compared bytes are non-zero"
    );

    // The marker the guest transmits is the byte the golden asserts.
    assert_eq!(FDC_WRITE_MARKER, b'W');
}
