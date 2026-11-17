; SPDX-License-Identifier: AGPL-3.0-or-later
;
; Boot sector of every diskette the FAT12 builder makes: a data disk. A
; machine started from it says that the disk holds no system, waits for a
; key, and asks the BIOS to load a boot sector again (INT 19h), so the user
; can swap in a system disk without resetting. Written for this project; it
; contains no byte of any other boot sector.
;
; The builder writes the BIOS Parameter Block and the extended boot record
; (offsets 3 to 61) for the diskette format; this source leaves them zero,
; and the code never reads or writes them. The test beside the builder
; assembles this file with `nasm -f bin` and compares the result with the
; bytes boot.rs holds.
;
; Interfaces used (IBM Personal Computer AT Technical Reference, BIOS):
; the bootstrap loader reads this sector to 0000:7C00h and jumps to it;
; INT 10h AH=0Eh writes a character as a teletype; INT 16h AH=00h waits
; for a keystroke; INT 19h reloads the boot sector. Only 8086 instructions
; are used, so the sector runs on 8088 machines and on the AT alike.

        cpu     8086
        bits    16
        org     0x7C00

        jmp     short start
        nop
        times   62 - ($ - $$) db 0      ; BIOS Parameter Block, filled by the builder

start:
        ; Some BIOSes enter with CS = 07C0h, others with CS = 0; the code
        ; addresses everything through DS = 0 with absolute offsets, so it
        ; never depends on CS. The stack goes just below the sector.
        cli
        xor     ax, ax
        mov     ds, ax
        mov     ss, ax
        mov     sp, 0x7C00
        sti
        cld
        mov     si, message
.print:
        lodsb
        or      al, al
        jz      .wait
        mov     ah, 0x0E
        mov     bx, 0x0007              ; page 0; grey in graphics modes
        int     0x10
        jmp     short .print
.wait:
        xor     ah, ah
        int     0x16
        int     0x19
        ; INT 19h does not return; stop here should a BIOS return anyway.
.halt:
        hlt
        jmp     short .halt

message:
        ; Each line fits a 40-column screen.
        db      13, 10, "This is not a system disk.", 13, 10
        db      "Replace it and press any key.", 13, 10, 0

        times   510 - ($ - $$) db 0
        dw      0xAA55
