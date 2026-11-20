// SPDX-License-Identifier: AGPL-3.0-or-later
//! The NES core compiled into both programs opens a cartridge image the way
//! Open… does: the file goes through `core_launch::prepare` and `start` over
//! the release's list of cores, and the machine runs with frame stops
//! (core-contract.md §13.3, §13.6).
//!
//! The cartridge is built here from bytes of our own, nothing proprietary:
//! an NROM (mapper 0) image whose program waits for two vertical blanks,
//! writes a palette, fills the first nametable with a checkerboard of two
//! tiles and turns the background on; its NMI handler counts frames. The
//! program, its listing and the tiles are copied from the frame golden of
//! unemunes (`crates/nes-core/tests/golden.rs`), Copyright (c) 2026
//! Mikołaj Mikołajczyk, licensed MIT OR Apache-2.0.
//!
//! The frame count and the picture's digest are pinned for one revision of
//! unemunes, as the PC's goldens are pinned for one revision of the CPU
//! core: moving the NES core's pin re-pins them here.

use std::path::Path;

use core_launch::{prepare, start, Cores, HostDirs, Outcome};
use image_store::test_support::TempDir;
use unemu_core_api::{DisplayId, RunLimit, Stop};

/// The unemunes revision the frame count and the digest were taken from.
const NES_REV: &str = "3aa955802a568a58b9bf8f5c29a0e23f44866a45";

/// The retired instructions the machine runs, stopping at every frame.
const STEPS: u64 = 80_000;
/// The frames the NES finishes within [`STEPS`].
const FRAMES: usize = 8;
/// FNV-1a over the last frame's pixels, each as a little-endian `u32`.
const GOLDEN: u64 = 0xFCDF_D186_11FE_D325;

/// The program at $8000, assembled by hand (listing: address, label,
/// instruction).
const PROGRAM: &[u8] = &[
    0x78, // $8000 reset: SEI
    0xD8, // $8001 CLD
    0xA2, 0xFF, // $8002 LDX #$FF
    0x9A, // $8004 TXS
    0x2C, 0x02, 0x20, // $8005 vblank1: BIT $2002
    0x10, 0xFB, // $8008 BPL vblank1
    0x2C, 0x02, 0x20, // $800A vblank2: BIT $2002
    0x10, 0xFB, // $800D BPL vblank2
    0xA9, 0x3F, // $800F LDA #$3F
    0x8D, 0x06, 0x20, // $8011 STA $2006
    0xA9, 0x00, // $8014 LDA #$00
    0x8D, 0x06, 0x20, // $8016 STA $2006
    0xA2, 0x00, // $8019 LDX #$00
    0xBD, 0x6B, 0x80, // $801B pal: LDA palette,X
    0x8D, 0x07, 0x20, // $801E STA $2007
    0xE8, // $8021 INX
    0xE0, 0x04, // $8022 CPX #$04
    0xD0, 0xF5, // $8024 BNE pal
    0xA9, 0x20, // $8026 LDA #$20
    0x8D, 0x06, 0x20, // $8028 STA $2006
    0xA9, 0x00, // $802B LDA #$00
    0x8D, 0x06, 0x20, // $802D STA $2006
    0xA0, 0x00, // $8030 LDY #$00
    0xA2, 0x00, // $8032 row: LDX #$00
    0x84, 0x00, // $8034 col: STY $00
    0x8A, // $8036 TXA
    0x18, // $8037 CLC
    0x65, 0x00, // $8038 ADC $00
    0x29, 0x01, // $803A AND #$01
    0x8D, 0x07, 0x20, // $803C STA $2007
    0xE8, // $803F INX
    0xE0, 0x20, // $8040 CPX #$20
    0xD0, 0xF0, // $8042 BNE col
    0xC8, // $8044 INY
    0xC0, 0x1E, // $8045 CPY #$1E
    0xD0, 0xE9, // $8047 BNE row
    0xA2, 0x40, // $8049 LDX #$40
    0xA9, 0x00, // $804B LDA #$00
    0x8D, 0x07, 0x20, // $804D attr: STA $2007
    0xCA, // $8050 DEX
    0xD0, 0xFA, // $8051 BNE attr
    0xA9, 0x00, // $8053 LDA #$00
    0x8D, 0x05, 0x20, // $8055 STA $2005
    0x8D, 0x05, 0x20, // $8058 STA $2005
    0xA9, 0x0A, // $805B LDA #$0A
    0x8D, 0x01, 0x20, // $805D STA $2001
    0xA9, 0x80, // $8060 LDA #$80
    0x8D, 0x00, 0x20, // $8062 STA $2000
    0x4C, 0x65, 0x80, // $8065 loop: JMP loop
    0xE6, 0x01, // $8068 nmi: INC $01
    0x40, // $806A RTI
    0x0F, 0x16, 0x2A, 0x12, // $806B palette: .byte $0F,$16,$2A,$12
];
const NMI: u16 = 0x8068;
const RESET: u16 = 0x8000;

/// An iNES image: one 16 KiB program bank (NOPs past the program, the
/// vectors at its end) and one 8 KiB character bank with two tiles, tile 0
/// in colour 1 and tile 1 in colour 2.
fn image() -> Vec<u8> {
    let mut prg = vec![0xEA; 0x4000];
    prg[..PROGRAM.len()].copy_from_slice(PROGRAM);
    let vectors = [NMI.to_le_bytes(), RESET.to_le_bytes(), NMI.to_le_bytes()].concat();
    prg[0x3FFA..].copy_from_slice(&vectors);
    let mut chr = vec![0u8; 0x2000];
    chr[0..8].fill(0xFF);
    chr[24..32].fill(0xFF);
    let mut bytes = b"NES\x1A\x01\x01\x00\x00\0\0\0\0\0\0\0\0".to_vec();
    bytes.extend(prg);
    bytes.extend(chr);
    bytes
}

/// FNV-1a over the pixels, little-endian.
fn digest(pixels: &[u32]) -> u64 {
    pixels
        .iter()
        .flat_map(|pixel| pixel.to_le_bytes())
        .fold(0xCBF2_9CE4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3)
        })
}

fn host_dirs(root: &Path) -> HostDirs {
    HostDirs {
        saves: root.join("saves"),
        states: root.join("states"),
    }
}

#[test]
fn the_pinned_unemunes_revision_is_the_one_in_the_lock_file() {
    assert!(
        include_str!("../../../Cargo.lock").contains(&format!(
            "git+https://github.com/unemu-org/unemunes?rev={NES_REV}#{NES_REV}"
        )),
        "the NES core's pin moved: re-pin the frame count and the digest for it"
    );
}

#[test]
fn a_cartridge_image_opens_through_the_release_cores_and_draws_its_golden() {
    let temp = TempDir::new("platforms-nes-cartridge");
    let path = temp.path().join("checkerboard.nes");
    std::fs::write(&path, image()).unwrap();
    let cores = Cores::new(platforms::cores());
    let outcome = prepare(&path, &cores, None, &host_dirs(temp.path())).unwrap();
    let Outcome::Ready(prepared) = outcome else {
        panic!("more than one compiled-in core opens the cartridge");
    };
    assert_eq!(prepared.core().description().id, "unemunes");
    assert_eq!(prepared.platform(), "nes");
    assert_eq!(
        prepared.platform_name(),
        "Nintendo Entertainment System (NTSC)"
    );
    assert_eq!(prepared.kind(), Some("ines"));
    assert_eq!(prepared.title(), "checkerboard.nes");

    let mut machine = start(&prepared).unwrap();
    let mut digests = Vec::new();
    loop {
        match machine
            .run(RunLimit::steps(STEPS).with_stop_on_frame(true))
            .unwrap()
        {
            Stop::Frame(DisplayId(0)) => {
                digests.push(digest(machine.frame(DisplayId(0)).unwrap().pixels));
            }
            Stop::Steps => break,
            other => panic!("the run stopped with {other:?}"),
        }
    }
    machine.finish().unwrap();

    // Frames 0 and 1 are the backdrop while the program waits for two
    // vertical blanks; it turns the background on during frame 2, so frame
    // 2 is partly drawn; from frame 3 on the checkerboard is whole and stays.
    assert_eq!(digests.len(), FRAMES, "frames within {STEPS} steps");
    assert_eq!(digests[0], digests[1]);
    assert_ne!(digests[1], digests[2]);
    assert_ne!(digests[2], digests[3]);
    assert!(
        digests[3..].iter().all(|each| *each == GOLDEN),
        "the pinned picture changed: {digests:#018X?}"
    );
}
