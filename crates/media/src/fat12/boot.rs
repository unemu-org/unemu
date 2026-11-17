// SPDX-License-Identifier: AGPL-3.0-or-later
//! The boot code of a data diskette, part of the IBM PC volume profile next
//! to the diskette formats: a machine started from a disk the builder made
//! says that the disk holds no system, waits for a key and loads a boot
//! sector again through INT 19h (IBM Personal Computer AT Technical
//! Reference, BIOS: bootstrap loader at 0000:7C00h, INT 10h AH=0Eh, INT 16h
//! AH=00h, INT 19h). Written for this project; no byte comes from another
//! boot sector. The source is `boot.asm` beside this file (8086 instructions
//! only, so 8088 machines run it as the AT does); the media crate's
//! `boot_sector_source` test assembles it with nasm and compares.

/// Where the code starts: right after the extended boot record, where the
/// jump at offset 0 (`EB 3C`) lands.
pub(super) const CODE_OFFSET: usize = 62;

/// Offset 62 onwards, loaded at 0000:7C3Eh. The code reads and writes no
/// byte of the BIOS Parameter Block.
#[rustfmt::skip]
pub(super) const CODE: [u8; 100] = [
    0xFA,                   // cli
    0x31, 0xC0,             // xor ax, ax
    0x8E, 0xD8,             // mov ds, ax
    0x8E, 0xD0,             // mov ss, ax
    0xBC, 0x00, 0x7C,       // mov sp, 7C00h
    0xFB,                   // sti
    0xFC,                   // cld
    0xBE, 0x64, 0x7C,       // mov si, message (7C64h)
    0xAC,                   // print: lodsb
    0x08, 0xC0,             // or al, al
    0x74, 0x09,             // jz wait
    0xB4, 0x0E,             // mov ah, 0Eh
    0xBB, 0x07, 0x00,       // mov bx, 0007h
    0xCD, 0x10,             // int 10h
    0xEB, 0xF2,             // jmp print
    0x30, 0xE4,             // wait: xor ah, ah
    0xCD, 0x16,             // int 16h
    0xCD, 0x19,             // int 19h
    0xF4,                   // halt: hlt
    0xEB, 0xFD,             // jmp halt
    // message: CR LF, two lines of at most 40 columns each ending CR LF,
    // then NUL.
    b'\r', b'\n', b'T', b'h', b'i', b's', b' ', b'i', b's', b' ', b'n', b'o',
    b't', b' ', b'a', b' ', b's', b'y', b's', b't', b'e', b'm', b' ', b'd',
    b'i', b's', b'k', b'.', b'\r', b'\n', b'R', b'e', b'p', b'l', b'a', b'c',
    b'e', b' ', b'i', b't', b' ', b'a', b'n', b'd', b' ', b'p', b'r', b'e',
    b's', b's', b' ', b'a', b'n', b'y', b' ', b'k', b'e', b'y', b'.', b'\r',
    b'\n', 0,
];
