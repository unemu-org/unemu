; SPDX-License-Identifier: AGPL-3.0-or-later
;
; at-pm-test: an open 80286 protected-mode return test for PC/AT machines.
;
; A bootable 1.2M floppy. The boot sector loads the program. It first asks
; the BIOS for the extended-memory size (INT 15h AH=88h) and moves a block
; above 1 MiB and back through the BIOS (INT 15h AH=87h), staying in real
; mode around each call. It then runs a fixed number of rounds. Each round
; enters protected mode, exercises it, and comes back to real mode the two
; ways an 80286 can: an 8042 pulse reset and a deliberate triple fault,
; both resumed by the BIOS through the CMOS shutdown byte and the doubleword
; at 0040:0067h. It then switches the A20 gate through the 8042 output port.
; After the rounds, one block move from a source the BIOS cannot read is
; reported for information. The machine paths are the ones a 286
; extended-memory manager and a 286 DOS extender use. Results go to the BIOS
; teletype and, byte for byte, to COM1. The program halts with interrupts
; disabled after its last line. See vendor/at-pm-test/README.md.
;
; References:
; - IBM Personal Computer AT Technical Reference, System Board, Keyboard
;   Controller: output port bit 0 (system reset) and bit 1 (gate A20);
;   commands D0h (read output port), D1h (write output port) and F0h-FFh
;   (pulse output port; FEh pulses bit 0, the processor reset).
; - IBM Personal Computer AT Technical Reference, System Board, Real-Time
;   Clock and CMOS RAM: address 0Fh is the shutdown status byte.
; - IBM Personal Computer AT Technical Reference, BIOS listing, POST reset
;   path (TEST1): after a reset with the 8042 system flag set, POST reads
;   CMOS 0Fh, clears it, and for code 05h (interrupt controllers initialized,
;   keyboard flushed, EOI) or 0Ah (no initialization) jumps through the
;   doubleword at 0040:0067h with SS:SP = 0000:0400h and DS = 0040h.
; - IBM Personal Computer AT Technical Reference, BIOS listing, GATE_A20
;   (BIOS1): command D1h, then output-port data DFh (A20 on) or DDh (A20 off).
; - IBM Personal Computer AT Technical Reference, BIOS listing, INT 15h
;   AH=88h (EXT_MEMORY, BIOS1): AX = CMOS 31h:30h, returned through IRET, so
;   the other flags are the caller's.
; - IBM Personal Computer AT Technical Reference, BIOS listing, INT 15h
;   AH=87h (BLOCKMOVE, BIOS1): CX words from the source to the target
;   descriptor of the caller's six-entry GDT at ES:SI, whose limits must be
;   at least 2 * CX - 1; the BIOS fills the GDT, code and stack entries. It
;   enables A20, sets CMOS 0Fh to 09h and returns through a processor reset;
;   POST (TEST1) clears the byte and jumps to SHUT9 (BIOS1), which turns A20
;   off. Status in AH: 00h done, 02h any exception during the move (every
;   vector of its IDT). It ends with CMP, STI and RETF 2: CF=0 and ZF=1 only
;   for 00h, IF set, and every other register restored.
; - Intel UPI-41A/41AH/42/42AH User's Manual, Data Bus Buffer status:
;   wait for IBF clear before a write, OBF set before a read.
; - Intel 80286 Programmer's Reference Manual, System Control and
;   Initialization: LMSW sets PE, followed by an intersegment jump.
; - Intel 80286 Programmer's Reference Manual, Interrupts and Exceptions:
;   a fault while delivering a fault raises the double fault (interrupt 8);
;   a fault while delivering the double fault shuts the processor down.
; - National Semiconductor PC16550D datasheet, Line Control Register (DLAB),
;   Line Status Register bit 5 (THRE) and Transmitter Holding Register.

cpu 286
bits 16

LOAD_SEG        equ 0x0800              ; program segment, physical 08000h
LOAD_BASE       equ LOAD_SEG * 16
PROG_SECTORS    equ 14                  ; sectors 2-15 of track 0, head 0
IMAGE_SIZE      equ 1228800             ; 80 cylinders, 2 heads, 15 sectors
STACK_TOP       equ 0xFFF0
ROUNDS          equ 16

; Protected-mode data test: a window above 1 MiB and its A20-masked alias.
HIGH_BASE       equ 0x120000
LOW_BASE        equ 0x020000
LOW_SEG         equ LOW_BASE / 16
LOW_PATTERN     equ 0x5AA5
HIGH_XOR        equ 0xC33C

; State that must survive a reset lives in the high memory area. The open
; BIOS cold-boots instead of resuming and tests only conventional memory, so
; the state survives that reboot. A BIOS that ignores the shutdown byte and
; also clears memory above 1 MiB loses it; start then reports "state lost".
; FFFF:FE10h is physical 10FE00h.
STATE_SEG       equ 0xFFFF
STATE_OFF       equ 0xFE10
STATE_BASE      equ STATE_SEG * 16 + STATE_OFF

ST_SIG          equ 0                   ; two words, SIG0 and SIG1
ST_ROUND        equ 4                   ; current round, 1-based
ST_PENDING      equ 6                   ; reset stage awaiting its resume
ST_CHECK        equ 8                   ; check value written before the reset
ST_PM           equ 10                  ; protected-mode checks that passed
ST_TF           equ 12                  ; triple-fault stage progress
ST_UNEXP        equ 14                  ; first unexpected vector, FFFFh = none
ST_FAILS        equ 16                  ; failure count
ST_PICMASK      equ 18                  ; master and slave 8259 masks
ST_SIZE         equ 20

SIG0            equ 0x4D50              ; "PM"
SIG1            equ 0x3638              ; "86"

P_NONE          equ 0
P_KBC           equ 1
P_TF            equ 2
P_INT15         equ 3                   ; a round-trip block move is running
P_INT15_FAULT   equ 4                   ; the faulting block move is running

ENTRY_BOOT      equ 1
ENTRY_VECTOR    equ 2

; CMOS shutdown codes; see the TEST1 reference above.
SHUT_JMP_EOI    equ 0x05
SHUT_JMP        equ 0x0A

; Protected-mode checks (ST_PM bits).
PM_CS           equ 0x0001              ; far jump loaded the code selector
PM_HIGH         equ 0x0002              ; data above 1 MiB reads back
PM_ALIAS        equ 0x0004              ; its alias below 1 MiB is untouched
PM_INT          equ 0x0008              ; INT 40h through the IDT
PM_GP           equ 0x0010              ; own #GP handler, error code 0
PM_ALL          equ 0x001F

TF_ENTERED      equ 0x0001              ; reached the fault in protected mode
TF_NO_SHUTDOWN  equ 0x0002              ; the fault did not shut down
TF_HANDLED      equ 0x0004              ; a #GP handler ran: IDT limit not 0

; Resume failure bits.
RF_PE           equ 0x0001              ; still in protected mode
RF_CMOS         equ 0x0002              ; BIOS left the shutdown byte set
RF_A20          equ 0x0004              ; 8042 A20 latch lost across reset
RF_RAM          equ 0x0008              ; conventional-memory copy differs
RF_CHECK        equ 0x0010              ; state check value differs
RF_UNEXP        equ 0x0020              ; unexpected protected-mode exception
RF_TF           equ 0x0040              ; triple fault not reached or survived
RF_STAGE        equ 0x0080              ; unknown pending stage
RF_PIC          equ 0x0100              ; 8259 masks do not fit the code
RF_REBOOT       equ 0x8000              ; BIOS rebooted instead of resuming

; A20 checks.
A20_OFF_READ    equ 0x0001              ; D0h reads bit 1 clear after DDh
A20_OFF_WRAP    equ 0x0002              ; FFFF:0010 aliases 0000:0000
A20_ON_READ     equ 0x0004              ; D0h reads bit 1 set after DFh
A20_ON_NOWRAP   equ 0x0008              ; FFFF:0010 is distinct memory
A20_ALL         equ 0x000F

; INT 15h AH=88h checks.
EXT_CF          equ 0x0001              ; CF set (called with CF clear)
EXT_AX          equ 0x0002              ; AX is not CMOS 31h:30h

; INT 15h AH=87h checks, ORed over the calls of a line.
MV_STATUS       equ 0x0001              ; AH is not the expected status
MV_CF           equ 0x0002              ; CF does not match the status
MV_ZF           equ 0x0004              ; ZF does not match the status
MV_IF           equ 0x0008              ; IF clear on return (called with IF set)
MV_REGS         equ 0x0010              ; AL, BX, CX, DX, SI, DI or BP changed
MV_SEGS         equ 0x0020              ; DS or ES changed
MV_STACK        equ 0x0040              ; SS:SP changed
MV_A20          equ 0x0080              ; A20 on after the call (off before it)
MV_CMOS         equ 0x0100              ; CMOS 0Fh not 00h after the call
MV_HIGH         equ 0x0200              ; the block above 1 MiB lacks the pattern
MV_BACK         equ 0x0400              ; the second buffer differs
MV_ALIAS        equ 0x0800              ; the A20 alias below 1 MiB was written
MV_PREP         equ 0x1000              ; A20 would not turn off before the call
MV_REARM        equ 0x2000              ; A20 would not turn on again; run stops

; AH=87h status codes; see the BLOCKMOVE reference above.
MOVE_OK         equ 0x00
MOVE_EXCEPTION  equ 0x02

; FLAGS for the AH=87h call: IF set, TF and DF clear (bit 1 is always set),
; CF and ZF the opposite of what the expected status returns.
FLAGS_FOR_OK    equ 0x0203              ; CF set, ZF clear
FLAGS_FOR_ERR   equ 0x0242              ; CF clear, ZF set

; Values the call must leave in the registers it does not return in.
MARK_AL         equ 0x5A
MARK_BX         equ 0x1BB1
MARK_DX         equ 0x2DD2
MARK_DI         equ 0x3DD3
MARK_BP         equ 0x4BB4
MARK_DS         equ 0x5DD5              ; never used to address memory

; Block-move buffers, past the loaded program in its own segment. The block
; above 1 MiB lies in the high memory area, so the program reads it back
; directly with A20 on; its A20-masked alias, 0800:4000h, is a guard area.
MOVE_WORDS      equ 128
MOVE_BYTES      equ MOVE_WORDS * 2
MOVE_SRC        equ 0x2000
MOVE_DST        equ 0x2400
MOVE_SRC_PHYS   equ LOAD_BASE + MOVE_SRC
MOVE_DST_PHYS   equ LOAD_BASE + MOVE_DST
MOVE_HIGH       equ 0x10C000
HMA_SEG         equ 0xFFFF
MOVE_HIGH_OFF   equ MOVE_HIGH - HMA_SEG * 16
MOVE_GUARD      equ MOVE_HIGH - 0x100000 - LOAD_BASE
MOVE_PATTERN    equ 0x1E87              ; first word; each next one adds 0101h
GUARD_WORD      equ 0x6DB6

; The caller's block-move GDT: six descriptors.
MG_SOURCE       equ 0x10
MG_TARGET       equ 0x18
MG_SIZE         equ 0x30

SEL_CODE        equ 0x08
SEL_DATA        equ 0x10
SEL_HIGH        equ 0x18
SEL_LOW         equ 0x20
SEL_SMALL       equ 0x28
SEL_STATE       equ 0x30

IDT_VECTORS     equ 0x41

%define ST(field) STATE_OFF + field

; 80286 segment descriptor: limit, 24-bit base, access byte, reserved word.
%macro DESC 3
        dw %2
        dw (%1) & 0xFFFF
        db ((%1) >> 16) & 0xFF
        db %3
        dw 0
%endmacro

; 80286 interrupt gate, DPL 0, through the code selector.
%macro GATE 1
        dw %1
        dw SEL_CODE
        db 0
        db 0x86
        dw 0
%endmacro

;--------------------------------------------------------------------------
; Boot sector, loaded by INT 19h at 0000:7C00h.
;--------------------------------------------------------------------------
section boot start=0 vstart=0x7C00

boot:
        cli
        xor ax, ax
        mov ds, ax
        mov ss, ax
        mov sp, 0x7C00
        sti
        cld
        mov [boot_drive], dl
        mov si, 3
.retry:
        mov ax, LOAD_SEG
        mov es, ax
        xor bx, bx
        mov ax, 0x0200 | PROG_SECTORS
        mov cx, 0x0002
        xor dh, dh
        mov dl, [boot_drive]
        int 0x13
        jnc .loaded
        xor ax, ax
        mov dl, [boot_drive]
        int 0x13
        dec si
        jnz .retry
        mov si, boot_error
.print:
        lodsb
        test al, al
        jz .halt
        mov ah, 0x0E
        mov bx, 0x0007
        int 0x10
        jmp .print
.halt:
        cli
        hlt
        jmp .halt
.loaded:
        jmp LOAD_SEG:start

boot_drive      db 0
boot_error      db 'at-pm-test: boot read error', 13, 10, 0

        times 510 - ($ - $$) db 0
        dw 0xAA55

;--------------------------------------------------------------------------
; Program, loaded at LOAD_SEG:0000h. Offsets equal protected-mode offsets:
; the code and data descriptors have base LOAD_BASE.
;--------------------------------------------------------------------------
section prog follows=boot vstart=0
prog_start:

start:
        cli
        cld
        mov ax, cs
        mov ds, ax
        mov es, ax
        mov ss, ax
        mov sp, STACK_TOP
        mov byte [entry], ENTRY_BOOT
        call uart_init
        mov al, 0xDF
        call kbc_write_output
        call state_valid
        jne .no_state
        mov ax, STATE_SEG
        mov es, ax
        mov bx, [es:ST(ST_PENDING)]
        cmp bx, P_NONE
        je fresh_start
        cmp bx, P_INT15
        je .int15
        cmp bx, P_INT15_FAULT
        jne .reset_stage
.int15:
        jmp int15_reboot
.reset_stage:
        ; A reset stage was pending, yet the BIOS booted the floppy again.
        mov dx, RF_REBOOT
        jmp resume_checks
.no_state:
        ; A shutdown code still set means the BIOS neither resumed nor kept
        ; memory above 1 MiB, so the round state is gone.
        mov al, 0x0F
        call cmos_read
        cmp al, SHUT_JMP_EOI
        je state_lost
        cmp al, SHUT_JMP
        je state_lost
        call nmi_enable

fresh_start:
        mov ax, STATE_SEG
        mov es, ax
        mov word [es:ST(ST_SIG)], SIG0
        mov word [es:ST(ST_SIG) + 2], SIG1
        mov word [es:ST(ST_ROUND)], 1
        mov word [es:ST(ST_PENDING)], P_NONE
        mov word [es:ST(ST_FAILS)], 0
        mov si, msg_header
        call print
        mov ax, ROUNDS
        call print_dec
        mov si, msg_crlf
        call print
        call stage_int15

round_begin:
        mov ax, STATE_SEG
        mov es, ax
        mov ax, [es:ST(ST_ROUND)]
        mov [round], ax
        ; Known contents at the address the high window aliases without A20.
        mov ax, LOW_SEG
        mov es, ax
        xor di, di
        mov ax, LOW_PATTERN
        mov cx, 8
        rep stosw
        mov bx, P_KBC
        mov cl, SHUT_JMP
        call arm_reset
        lgdt [gdtr]
        lidt [idtr]
        smsw ax
        or al, 1
        lmsw ax
        jmp SEL_CODE:pm_stage_a

stage_tf:
        mov bx, P_TF
        mov cl, SHUT_JMP_EOI
        call arm_reset
        lgdt [gdtr]
        lidt [idtr]
        smsw ax
        or al, 1
        lmsw ax
        jmp SEL_CODE:pm_stage_c

next_round:
        mov ax, STATE_SEG
        mov es, ax
        inc word [es:ST(ST_ROUND)]
        cmp word [es:ST(ST_ROUND)], ROUNDS
        ja rounds_done
        jmp round_begin

rounds_done:
        call stage_int15_fault

finish:
        mov ax, STATE_SEG
        mov es, ax
        mov word [es:ST(ST_SIG)], 0
        mov word [es:ST(ST_SIG) + 2], 0
        mov si, msg_failures
        call print
        mov ax, [es:ST(ST_FAILS)]
        call print_dec
        mov si, msg_crlf
        call print
halt_forever:
        cli
        hlt
        jmp halt_forever

; The round state is gone: report one failure, the count, and stop. The
; earlier count was part of the lost state.
state_lost:
        mov al, 0x0F
        xor ah, ah
        call cmos_write
        call nmi_enable
        mov si, msg_state_lost
        call print
        mov ax, 1
        jmp print_count_and_halt

; The BIOS booted the floppy during an AH=87h call instead of returning
; from it. A reboot in the round trip fails that line and the rounds still
; run; the faulting move comes after the rounds, so there the run ends.
; In: ES = state segment, BX = pending stage.
int15_reboot:
        mov word [es:ST(ST_PENDING)], P_NONE
        mov al, 0x0F
        xor ah, ah
        call cmos_write
        call nmi_enable
        mov dx, RF_REBOOT
        cmp bx, P_INT15
        jne .fault
        mov si, name_move
        call report_once
        jmp round_begin
.fault:
        mov si, name_fault
        call report_info
        jmp finish

; In: AX = failure count.
print_count_and_halt:
        mov si, msg_failures
        call print
        call print_dec
        mov si, msg_crlf
        call print
        jmp halt_forever

;--------------------------------------------------------------------------
; Resume. The BIOS enters resume_vector through 0040:0067h; start enters
; resume_checks with RF_REBOOT when the BIOS booted the floppy instead.
;--------------------------------------------------------------------------
resume_vector:
        cli
        cld
        mov ax, cs
        mov ds, ax
        mov es, ax
        mov ss, ax
        mov sp, STACK_TOP
        mov byte [entry], ENTRY_VECTOR
        ; Before anything restores them: code 05h masks every 8259 line,
        ; code 0Ah leaves the masks as they were.
        in al, 0x21
        mov [pic_seen], al
        in al, 0xA1
        mov [pic_seen + 1], al
        xor dx, dx
        ; The 8042 is not reset with the processor, so A20 must still be on.
        call kbc_read_output
        jc .a20_lost
        test al, 2
        jnz .a20_kept
.a20_lost:
        or dx, RF_A20
.a20_kept:
        ; POST clears the shutdown byte before it dispatches.
        mov al, 0x0F
        call cmos_read
        test al, al
        jz resume_checks
        or dx, RF_CMOS

resume_checks:
        mov al, 0x0F
        xor ah, ah
        call cmos_write
        call nmi_enable
        mov al, 0xDF
        call kbc_write_output
        smsw ax
        test al, 1
        jz .real
        or dx, RF_PE
.real:
        call state_valid
        je .state
        jmp state_lost
.state:
        mov ax, STATE_SEG
        mov es, ax
        mov ax, [es:ST(ST_ROUND)]
        mov bx, [es:ST(ST_PENDING)]
        mov word [es:ST(ST_PENDING)], P_NONE
        cmp byte [entry], ENTRY_VECTOR
        jne .ram_done
        cmp ax, [ram_round]
        jne .ram_bad
        push ax
        call check_value
        cmp ax, [ram_check]
        pop ax
        je .ram_done
.ram_bad:
        or dx, RF_RAM
.ram_done:
        mov [round], ax
        call check_value
        cmp ax, [es:ST(ST_CHECK)]
        je .check_ok
        or dx, RF_CHECK
.check_ok:
        mov ax, [es:ST(ST_UNEXP)]
        cmp ax, 0xFFFF
        je .no_unexp
        or dx, RF_UNEXP
        mov [unexp_vector], ax
.no_unexp:
        cmp byte [entry], ENTRY_VECTOR
        jne .pic_done
        mov ax, 0xFFFF
        cmp bx, P_TF
        je .pic_compare
        mov ax, [es:ST(ST_PICMASK)]
.pic_compare:
        cmp ax, [pic_seen]
        je .pic_done
        or dx, RF_PIC
.pic_done:
        ; Restore the masks after either code.
        mov al, [es:ST(ST_PICMASK)]
        out 0x21, al
        mov al, [es:ST(ST_PICMASK) + 1]
        out 0xA1, al
        cmp bx, P_KBC
        je .kbc
        cmp bx, P_TF
        je .tf
        or dx, RF_STAGE
        mov si, name_resume
        call report
        jmp finish
.kbc:
        push dx
        mov dx, [es:ST(ST_PM)]
        xor dx, PM_ALL
        mov si, name_pm
        call report
        pop dx
        mov si, name_kbc
        call report
        jmp stage_tf
.tf:
        cmp word [es:ST(ST_TF)], TF_ENTERED
        je .tf_ok
        or dx, RF_TF
.tf_ok:
        mov si, name_tf
        call report
        call stage_a20
        jmp next_round

;--------------------------------------------------------------------------
; A20 through the 8042 output port, checked by readback and by the wrap of
; FFFF:0010h onto 0000:0000h. Leaves A20 on.
;--------------------------------------------------------------------------
stage_a20:
        ; Keep the failure count while A20 is on: if A20 cannot be turned
        ; back on, the state block above 1 MiB is out of reach.
        push es
        mov ax, STATE_SEG
        mov es, ax
        mov ax, [es:ST(ST_FAILS)]
        mov [fails_before], ax
        pop es
        xor bx, bx
        mov al, 0xDD
        call kbc_write_output
        call kbc_read_output
        jc .off_read
        test al, 2
        jnz .off_read
        or bx, A20_OFF_READ
.off_read:
        call wrap_test
        cmp al, 1
        jne .off_wrap
        or bx, A20_OFF_WRAP
.off_wrap:
        mov al, 0xDF
        call kbc_write_output
        call kbc_read_output
        jc .on_read
        test al, 2
        jz .on_read
        or bx, A20_ON_READ
.on_read:
        call wrap_test
        test al, al
        jnz .on_wrap
        or bx, A20_ON_NOWRAP
.on_wrap:
        mov dx, bx
        xor dx, A20_ALL
        mov si, name_a20
        test bx, A20_ON_NOWRAP
        jz .stuck
        jmp report
.stuck:
        ; A20 stays masked: report without the state block, then stop.
        mov byte [a20_stuck], 1
        call report
        mov ax, [fails_before]
        inc ax
        jmp print_count_and_halt

; Out: AL = 1 when FFFF:0010h and 0000:0000h are the same memory, else 0.
; Both words are restored; interrupts stay disabled throughout.
wrap_test:
        push ds
        push es
        push bx
        push cx
        push dx
        xor ax, ax
        mov ds, ax
        mov ax, 0xFFFF
        mov es, ax
        mov dx, [0x0000]
        mov cx, [es:0x0010]
        mov word [0x0000], 0x1234
        mov word [es:0x0010], 0x5678
        mov bx, [0x0000]
        mov [es:0x0010], cx
        mov [0x0000], dx
        xor ax, ax
        cmp bx, 0x5678
        jne .done
        inc ax
.done:
        pop dx
        pop cx
        pop bx
        pop es
        pop ds
        ret

;--------------------------------------------------------------------------
; INT 15h AH=88h and the AH=87h round trip, once before the rounds. The
; program stays in real mode; the BIOS does the protected-mode work.
; In: DS = program segment.
;--------------------------------------------------------------------------
stage_int15:
        mov ax, ds
        mov es, ax
        mov al, 0x30
        call cmos_read
        mov [ext_cmos], al
        mov al, 0x31
        call cmos_read
        mov [ext_cmos + 1], al
        call nmi_enable
        mov ax, 0x8800
        ; The IBM BIOS leaves CF as the caller had it.
        clc
        int 0x15
        cli
        mov dx, 0                       ; keeps CF
        jnc .ext_cf
        or dx, EXT_CF
.ext_cf:
        cmp ax, [ext_cmos]
        je .ext_ax
        or dx, EXT_AX
.ext_ax:
        mov si, name_ext
        call report_once

        ; A numbered pattern goes above 1 MiB and back to a cleared buffer.
        mov di, MOVE_SRC
        mov ax, MOVE_PATTERN
        mov cx, MOVE_WORDS
.fill:
        stosw
        add ax, 0x0101
        loop .fill
        xor ax, ax
        mov di, MOVE_DST
        mov cx, MOVE_WORDS
        rep stosw
        push es
        mov bx, HMA_SEG
        mov es, bx
        mov di, MOVE_HIGH_OFF
        mov cx, MOVE_WORDS
        rep stosw
        pop es
        mov ax, GUARD_WORD
        mov di, MOVE_GUARD
        mov cx, MOVE_WORDS
        rep stosw

        mov word [move_line], name_move
        mov ax, MOVE_SRC_PHYS & 0xFFFF
        mov dl, MOVE_SRC_PHYS >> 16
        mov cx, MOVE_BYTES - 1
        mov bx, MOVE_HIGH & 0xFFFF
        mov dh, MOVE_HIGH >> 16
        call set_move_gdt
        mov al, MOVE_OK
        mov cx, MOVE_WORDS
        call bios_move
        mov [move_fails], dx
        push es
        mov ax, HMA_SEG
        mov es, ax
        mov si, MOVE_SRC
        mov di, MOVE_HIGH_OFF
        mov cx, MOVE_WORDS
        repe cmpsw
        pop es
        je .high
        or word [move_fails], MV_HIGH
.high:
        mov ax, MOVE_HIGH & 0xFFFF
        mov dl, MOVE_HIGH >> 16
        mov cx, MOVE_BYTES - 1
        mov bx, MOVE_DST_PHYS & 0xFFFF
        mov dh, MOVE_DST_PHYS >> 16
        call set_move_gdt
        mov al, MOVE_OK
        mov cx, MOVE_WORDS
        call bios_move
        or [move_fails], dx
        mov si, MOVE_SRC
        mov di, MOVE_DST
        mov cx, MOVE_WORDS
        repe cmpsw
        je .back
        or word [move_fails], MV_BACK
.back:
        ; Each call starts with A20 off. A BIOS that moves without turning
        ; it on writes the block to its alias.
        mov ax, GUARD_WORD
        mov di, MOVE_GUARD
        mov cx, MOVE_WORDS
        repe scasw
        je .alias
        or word [move_fails], MV_ALIAS
.alias:
        mov dx, [move_fails]
        mov si, name_move
        jmp report_once

;--------------------------------------------------------------------------
; INT 15h AH=87h from a source descriptor with limit 0, once after the
; rounds, so that a BIOS that hangs on it costs only this line. The first
; word read raises #GP inside the BIOS's move. The call breaks the entry
; condition the IBM listing documents (limits of at least 2 * CX - 1), so
; the result is the BIOS's own: printed against the IBM listing's status
; 02h, never counted. In: DS = program segment.
;--------------------------------------------------------------------------
stage_int15_fault:
        mov ax, ds
        mov es, ax
        mov word [move_line], name_fault
        mov ax, MOVE_SRC_PHYS & 0xFFFF
        mov dl, MOVE_SRC_PHYS >> 16
        xor cx, cx
        mov bx, MOVE_DST_PHYS & 0xFFFF
        mov dh, MOVE_DST_PHYS >> 16
        call set_move_gdt
        mov al, MOVE_EXCEPTION
        mov cx, 1
        call bios_move
        mov si, name_fault
        jmp report_info

; Build the caller's block-move GDT at move_gdt. The dummy, GDT, BIOS code
; and stack entries are zero for the BIOS to fill; the source and target are
; read/write data segments (access byte 93h). In: DL:AX = source base,
; CX = source limit, DH:BX = target base (limit MOVE_BYTES - 1), ES = DS.
set_move_gdt:
        push ax
        push cx
        push di
        mov di, move_gdt
        mov cx, MG_SIZE / 2
        xor ax, ax
        rep stosw
        pop di
        pop cx
        pop ax
        mov [move_gdt + MG_SOURCE], cx
        mov [move_gdt + MG_SOURCE + 2], ax
        mov [move_gdt + MG_SOURCE + 4], dl
        mov byte [move_gdt + MG_SOURCE + 5], 0x93
        mov word [move_gdt + MG_TARGET], MOVE_BYTES - 1
        mov [move_gdt + MG_TARGET + 2], bx
        mov [move_gdt + MG_TARGET + 4], dh
        mov byte [move_gdt + MG_TARGET + 5], 0x93
        ret

; One INT 15h AH=87h call with ES:SI = move_gdt. A20 is turned off first,
; so the BIOS must turn it on for the move. IF is set; CF and ZF start
; opposite to what the expected status returns, so a BIOS that leaves them
; alone fails; the other registers and DS hold markers. Afterwards the
; program's own A20 setting (on) is restored before the state block is
; touched again; if A20 stays off, the line is reported and the run stops.
; In: AL = expected status, CX = word count, [move_line] = line name,
; ES = DS = program segment. Out: DX = MV_* failure bits.
bios_move:
        mov [move_expect], al
        mov [move_count], cx
        mov word [move_flags], FLAGS_FOR_OK
        mov bx, P_INT15
        test al, al
        jz .flags
        mov word [move_flags], FLAGS_FOR_ERR
        mov bx, P_INT15_FAULT
.flags:
        ; A BIOS that boots the floppy instead of returning sends start to
        ; int15_reboot instead of starting over.
        push es
        mov ax, STATE_SEG
        mov es, ax
        mov [es:ST(ST_PENDING)], bx
        mov ax, [es:ST(ST_FAILS)]
        mov [fails_before], ax
        pop es
        ; A code left from an earlier shutdown would read as the BIOS's.
        mov al, 0x0F
        xor ah, ah
        call cmos_write
        call nmi_enable
        mov word [move_prep], 0
        mov al, 0xDD
        call kbc_write_output
        jc .still_on
        call wrap_test
        cmp al, 1
        je .a20_masked
.still_on:
        mov word [move_prep], MV_PREP
.a20_masked:
        mov [move_sp], sp
        mov ah, 0x87
        mov al, MARK_AL
        mov bx, MARK_BX
        mov cx, [move_count]
        mov dx, MARK_DX
        mov si, move_gdt
        mov di, MARK_DI
        mov bp, MARK_BP
        push word [move_flags]
        push word MARK_DS
        pop ds
        popf
        int 0x15
        ; Record the result through CS before the stack is used again.
        mov [cs:out_ax], ax
        mov [cs:out_bx], bx
        mov [cs:out_cx], cx
        mov [cs:out_dx], dx
        mov [cs:out_si], si
        mov [cs:out_di], di
        mov [cs:out_bp], bp
        mov [cs:out_ds], ds
        mov [cs:out_es], es
        mov [cs:out_ss], ss
        mov [cs:out_sp], sp
        mov ax, cs
        mov ss, ax
        mov sp, [cs:move_sp]
        pushf
        pop word [cs:out_flags]
        cli
        cld
        mov ds, ax
        mov es, ax
        mov dx, [move_prep]
        mov al, [out_ax + 1]
        cmp al, [move_expect]
        je .status
        or dx, MV_STATUS
.status:
        ; Status 00h: CF clear and ZF set; any other status: the reverse.
        mov bx, 0x0040
        cmp byte [move_expect], MOVE_OK
        je .want
        mov bx, 0x0001
.want:
        mov ax, [out_flags]
        xor ax, bx
        test al, 0x01
        jz .cf
        or dx, MV_CF
.cf:
        test al, 0x40
        jz .zf
        or dx, MV_ZF
.zf:
        test ah, 0x02
        jnz .if
        or dx, MV_IF
.if:
        cmp byte [out_ax], MARK_AL
        jne .regs
        cmp word [out_bx], MARK_BX
        jne .regs
        mov ax, [move_count]
        cmp [out_cx], ax
        jne .regs
        cmp word [out_dx], MARK_DX
        jne .regs
        cmp word [out_si], move_gdt
        jne .regs
        cmp word [out_di], MARK_DI
        jne .regs
        cmp word [out_bp], MARK_BP
        je .regs_kept
.regs:
        or dx, MV_REGS
.regs_kept:
        cmp word [out_ds], MARK_DS
        jne .segs
        mov ax, cs
        cmp [out_es], ax
        je .segs_kept
.segs:
        or dx, MV_SEGS
.segs_kept:
        mov ax, cs
        cmp [out_ss], ax
        jne .stack
        mov ax, [move_sp]
        cmp [out_sp], ax
        je .stack_kept
.stack:
        or dx, MV_STACK
.stack_kept:
        ; The IBM BIOS turns A20 off on the way out; a BIOS that restores
        ; the state it found leaves it off as well.
        call kbc_read_output
        jc .a20_on
        test al, 2
        jz .a20_off
.a20_on:
        or dx, MV_A20
.a20_off:
        mov al, 0xDF
        call kbc_write_output
        call wrap_test
        test al, al
        jnz .stuck
        mov al, 0x0F
        call cmos_read
        test al, al
        jz .cmos
        or dx, MV_CMOS
.cmos:
        call nmi_enable
        push es
        mov ax, STATE_SEG
        mov es, ax
        mov word [es:ST(ST_PENDING)], P_NONE
        pop es
        ret
.stuck:
        ; A20 stays masked, so the state block is out of reach: report the
        ; line without it and stop, as the a20 stage does.
        or dx, MV_REARM
        mov byte [a20_stuck], 1
        mov si, [move_line]
        call report_once
        mov ax, [fails_before]
        inc ax
        jmp print_count_and_halt

;--------------------------------------------------------------------------
; Prepare a reset stage: state, check values, resume vector, PIC masks and
; the shutdown byte. In: BX = pending stage, CL = shutdown code.
;--------------------------------------------------------------------------
arm_reset:
        push es
        mov ax, STATE_SEG
        mov es, ax
        mov [es:ST(ST_PENDING)], bx
        mov ax, [round]
        mov [ram_round], ax
        call check_value
        mov [es:ST(ST_CHECK)], ax
        mov [ram_check], ax
        mov word [es:ST(ST_UNEXP)], 0xFFFF
        mov word [es:ST(ST_PM)], 0
        mov word [es:ST(ST_TF)], 0
        in al, 0x21
        mov [es:ST(ST_PICMASK)], al
        in al, 0xA1
        mov [es:ST(ST_PICMASK) + 1], al
        pop es
        push ds
        mov ax, 0x0040
        mov ds, ax
        mov word [0x0067], resume_vector
        mov word [0x0069], LOAD_SEG
        pop ds
        mov al, 0x0F
        mov ah, cl
        call cmos_write
        ; NMI stays masked through protected mode and the reset; register D
        ; is selected so no stray data access reaches the shutdown byte.
        jmp nmi_disable

; In: AX = round, BX = stage. Out: AX = check value.
check_value:
        xchg al, ah
        or al, bl
        xor ax, 0xA55A
        ret

; Out: ZF set when the state block carries its signature.
state_valid:
        push es
        push ax
        mov ax, STATE_SEG
        mov es, ax
        cmp word [es:ST(ST_SIG)], SIG0
        jne .done
        cmp word [es:ST(ST_SIG) + 2], SIG1
.done:
        pop ax
        pop es
        ret

;--------------------------------------------------------------------------
; Protected mode, stage a: segment loads, memory above 1 MiB, INT through
; the IDT, a handled #GP, then the 8042 pulse reset.
;--------------------------------------------------------------------------
pm_stage_a:
        mov ax, SEL_DATA
        mov ss, ax
        mov ds, ax
        mov es, ax
        xor bx, bx
        mov ax, cs
        cmp ax, SEL_CODE
        jne .cs
        or bx, PM_CS
.cs:
        mov ax, SEL_HIGH
        mov es, ax
        mov ax, [round]
        xor ax, HIGH_XOR
        xor di, di
        mov cx, 8
        rep stosw
        xor di, di
        mov cx, 8
        repe scasw
        jne .high
        or bx, PM_HIGH
.high:
        mov ax, SEL_LOW
        mov es, ax
        mov ax, LOW_PATTERN
        xor di, di
        mov cx, 8
        repe scasw
        jne .alias
        or bx, PM_ALIAS
.alias:
        mov word [int_mark], 0
        int 0x40
        cmp word [int_mark], 0x4040
        jne .int
        or bx, PM_INT
.int:
        mov word [gp_count], 0
        mov word [gp_error], 0xFFFF
        mov ax, SEL_SMALL
        mov ds, ax
        ; Offset 100h is past the 256-byte limit: #GP(0). The handler
        ; resumes at gp_resume; without the fault, execution falls through.
        mov ax, [0x0100]
gp_resume:
        mov ax, SEL_DATA
        mov ds, ax
        cmp word [gp_count], 1
        jne .gp
        cmp word [gp_error], 0
        jne .gp
        or bx, PM_GP
.gp:
        mov ax, SEL_STATE
        mov es, ax
        mov [es:ST_PM], bx
        jmp kbc_reset

;--------------------------------------------------------------------------
; Protected mode, stage c: an IDT limit of 0 makes the #GP undeliverable,
; then the double fault too, and the processor shuts down.
;--------------------------------------------------------------------------
pm_stage_c:
        mov ax, SEL_DATA
        mov ss, ax
        mov ds, ax
        mov ax, SEL_STATE
        mov es, ax
        mov ax, cs
        cmp ax, SEL_CODE
        jne .no_fault
        or word [es:ST_TF], TF_ENTERED
        lidt [idtr_zero]
        mov ax, SEL_SMALL
        mov ds, ax
        mov ax, [0x0100]
.no_fault:
        or word [es:ST_TF], TF_NO_SHUTDOWN
        jmp kbc_reset

; Pulse the processor reset through the 8042 (command FEh). Works in either
; mode: it uses only port I/O and the stack.
kbc_reset:
        call kbc_wait_input
        mov al, 0xFE
        out 0x64, al
.wait:
        hlt
        jmp .wait

int40_handler:
        push ds
        push ax
        mov ax, SEL_DATA
        mov ds, ax
        mov word [int_mark], 0x4040
        pop ax
        pop ds
        iret

; Stack: error code, IP, CS, FLAGS. Records the fault and returns to
; gp_resume instead of retrying the faulting instruction.
gp_handler:
        push bp
        mov bp, sp
        push ax
        push ds
        ; The triple-fault stage must never get here; arming that stage
        ; clears this bit after the protected-mode stage sets it.
        mov ax, SEL_STATE
        mov ds, ax
        or word [ST_TF], TF_HANDLED
        mov ax, SEL_DATA
        mov ds, ax
        inc word [gp_count]
        mov ax, [bp + 2]
        mov [gp_error], ax
        mov word [bp + 4], gp_resume
        pop ds
        pop ax
        pop bp
        add sp, 2
        iret

; Any other vector records its number and returns to real mode through
; the pending reset stage, whose resume reports the failure.
unexpected_common:
        pop ax
        mov bx, SEL_STATE
        mov ds, bx
        cmp word [ST_UNEXP], 0xFFFF
        jne .seen
        mov [ST_UNEXP], ax
.seen:
        jmp kbc_reset

unexp_stubs:
%assign v 0
%rep IDT_VECTORS
        push word v
        jmp unexpected_common
        times (v + 1) * 8 - ($ - unexp_stubs) db 0x90
%assign v v + 1
%endrep

;--------------------------------------------------------------------------
; 8042, CMOS and COM1 access.
;--------------------------------------------------------------------------

; Out: CF set when the input buffer stays full.
kbc_wait_input:
        push cx
        xor cx, cx
.poll:
        in al, 0x64
        test al, 2
        jz .empty
        loop .poll
        pop cx
        stc
        ret
.empty:
        pop cx
        clc
        ret

; Out: CF set when no output byte arrives.
kbc_wait_output:
        push cx
        xor cx, cx
.poll:
        in al, 0x64
        test al, 1
        jnz .full
        loop .poll
        pop cx
        stc
        ret
.full:
        pop cx
        clc
        ret

; Discard stale output so the next read is the command's reply.
kbc_flush_output:
        push cx
        mov cx, 16
.poll:
        in al, 0x64
        test al, 1
        jz .done
        in al, 0x60
        loop .poll
.done:
        pop cx
        ret

; In: AL = output-port value. Out: CF on timeout.
kbc_write_output:
        push ax
        call kbc_wait_input
        mov al, 0xD1
        out 0x64, al
        call kbc_wait_input
        pop ax
        out 0x60, al
        jmp kbc_wait_input

; Out: AL = output port, CF on timeout.
kbc_read_output:
        call kbc_flush_output
        call kbc_wait_input
        mov al, 0xD0
        out 0x64, al
        call kbc_wait_output
        jc .done
        in al, 0x60
        clc
.done:
        ret

; In: AL = index. Out: AL = value. Leaves the index selected, NMI masked.
cmos_read:
        or al, 0x80
        out 0x70, al
        jmp short $ + 2                 ; I/O recovery between 70h and 71h
        in al, 0x71
        ret

; In: AL = index, AH = value. Leaves the index selected, NMI masked.
cmos_write:
        or al, 0x80
        out 0x70, al
        jmp short $ + 2                 ; I/O recovery between 70h and 71h
        mov al, ah
        out 0x71, al
        ret

; Select register D with NMI masked (port 70h bit 7 set) or enabled.
nmi_disable:
        mov al, 0x8D
        out 0x70, al
        ret

nmi_enable:
        mov al, 0x0D
        out 0x70, al
        ret

; COM1: 115200 baud (divisor 1), 8 data bits, no parity, 1 stop bit.
uart_init:
        mov dx, 0x3FB
        mov al, 0x80
        out dx, al
        mov dx, 0x3F8
        mov al, 1
        out dx, al
        inc dx
        xor al, al
        out dx, al
        mov dx, 0x3FB
        mov al, 0x03
        out dx, al
        mov dx, 0x3F9
        xor al, al
        out dx, al
        ret

; In: AL = character. Sends it to COM1 and the BIOS teletype.
put_char:
        pusha
        push ds
        push es
        mov bl, al
        mov dx, 0x3FD
        xor cx, cx
.thre:
        in al, dx
        test al, 0x20
        jnz .send
        loop .thre
.send:
        mov al, bl
        mov dx, 0x3F8
        out dx, al
        mov ah, 0x0E
        mov bx, 0x0007
        int 0x10
        pop es
        pop ds
        popa
        ret

; In: DS:SI = zero-terminated string.
print:
        push ax
        push si
.next:
        lodsb
        test al, al
        jz .done
        call put_char
        jmp .next
.done:
        pop si
        pop ax
        ret

; In: AX = unsigned value, printed in decimal.
print_dec:
        pusha
        mov bx, 10
        xor cx, cx
.digit:
        xor dx, dx
        div bx
        push dx
        inc cx
        test ax, ax
        jnz .digit
.out:
        pop ax
        add al, '0'
        call put_char
        loop .out
        popa
        ret

; In: AL = value, printed as two hex digits.
print_hex2:
        pusha
        mov bh, al
        mov cx, 2
        jmp print_hex_digits

; In: AX = value, printed as four hex digits.
print_hex4:
        pusha
        mov bx, ax
        mov cx, 4
; In: BX = value, its top CX nibbles printed; the caller saved with PUSHA.
print_hex_digits:
.digit:
        rol bx, 4
        mov al, bl
        and al, 0x0F
        add al, '0'
        cmp al, '9'
        jbe .out
        add al, 'A' - '0' - 10
.out:
        call put_char
        loop .digit
        popa
        ret

; Print "rNN <stage> ok" or a failure, counting failures in the state block.
; In: SI = stage name, DX = failure bits (0 = passed).
report:
        pusha
        mov al, 'r'
        call put_char
        mov ax, [round]
        cmp ax, 10
        jae .two
        mov al, '0'
        call put_char
        mov ax, [round]
.two:
        call print_dec
        mov al, ' '
        call put_char
        mov bp, 1                       ; a resume mask may name a vector
        jmp report_result

; The same without the round, for the stages that run once before the
; rounds. Their masks never name a vector.
report_once:
        pusha
        xor bp, bp
report_result:
        push es
        call print
        test dx, dx
        jnz .fail
        mov si, msg_ok
        call print
        jmp .done
.fail:
        cmp byte [a20_stuck], 0
        jne .counted
        mov ax, STATE_SEG
        mov es, ax
        inc word [es:ST(ST_FAILS)]
.counted:
        cmp dx, RF_REBOOT
        jne .bits
        mov si, msg_fail_reboot
        call print
        jmp .done
.bits:
        mov si, msg_fail
        call print
        mov ax, dx
        call print_hex4
        ; Only resume masks use this bit with a vector; the round stage
        ; masks stay below it.
        test bp, bp
        jz .eol
        test dx, RF_UNEXP
        jz .eol
        mov si, msg_vector
        call print
        mov al, [unexp_vector]
        call print_hex2
.eol:
        mov si, msg_crlf
        call print
.done:
        pop es
        popa
        ret

; Print "<stage> ah XX info XXXX", never counted: the result of a call
; outside the BIOS's documented contract, with the bits that differ from the
; IBM listing. A reboot prints "<stage> info reboot".
; In: SI = stage name, DX = differing bits or RF_REBOOT.
report_info:
        pusha
        call print
        cmp dx, RF_REBOOT
        je .reboot
        mov si, msg_ah
        call print
        mov al, [out_ax + 1]
        call print_hex2
        mov si, msg_info
        call print
        mov ax, dx
        call print_hex4
        jmp .eol
.reboot:
        mov si, msg_info
        call print
        mov si, msg_reboot
        call print
.eol:
        mov si, msg_crlf
        call print
        popa
        ret

;--------------------------------------------------------------------------
; Tables and data.
;--------------------------------------------------------------------------
        align 8
gdt:
        dw 0, 0, 0, 0
        DESC LOAD_BASE, 0xFFFF, 0x9A
        DESC LOAD_BASE, 0xFFFF, 0x92
        DESC HIGH_BASE, 0x0FFF, 0x92
        DESC LOW_BASE, 0x0FFF, 0x92
        DESC LOAD_BASE, 0x00FF, 0x92
        DESC STATE_BASE, ST_SIZE - 1, 0x92
gdt_end:

        align 8
idt:
%assign v 0
%rep IDT_VECTORS
%if v == 13
        GATE gp_handler
%elif v == 0x40
        GATE int40_handler
%else
        GATE unexp_stubs + v * 8
%endif
%assign v v + 1
%endrep
idt_end:

gdtr:
        dw gdt_end - gdt - 1
        dw (LOAD_BASE + (gdt - prog_start)) & 0xFFFF
        db (LOAD_BASE + (gdt - prog_start)) >> 16
        db 0
idtr:
        dw idt_end - idt - 1
        dw (LOAD_BASE + (idt - prog_start)) & 0xFFFF
        db (LOAD_BASE + (idt - prog_start)) >> 16
        db 0
idtr_zero:
        dw 0, 0, 0

entry           db 0
                db 0
round           dw 0
ram_round       dw 0
ram_check       dw 0
int_mark        dw 0
gp_count        dw 0
gp_error        dw 0
pic_seen        dw 0
unexp_vector    dw 0
fails_before    dw 0
a20_stuck       db 0
                db 0
move_expect     db 0
                db 0
move_count      dw 0
move_flags      dw 0
move_sp         dw 0
move_fails      dw 0
move_prep       dw 0
move_line       dw 0
ext_cmos        dw 0
out_ax          dw 0
out_bx          dw 0
out_cx          dw 0
out_dx          dw 0
out_si          dw 0
out_di          dw 0
out_bp          dw 0
out_ds          dw 0
out_es          dw 0
out_ss          dw 0
out_sp          dw 0
out_flags       dw 0

        align 8
move_gdt:
        times MG_SIZE db 0

msg_header      db 'at-pm-test rounds ', 0
msg_failures    db 'failures ', 0
msg_ok          db ' ok', 13, 10, 0
msg_fail        db ' FAIL ', 0
msg_fail_reboot db ' FAIL reboot', 13, 10, 0
msg_crlf        db 13, 10, 0
msg_state_lost  db 'resume FAIL state lost', 13, 10, 0
msg_vector      db ' vector ', 0
msg_ah          db ' ah ', 0
msg_info        db ' info ', 0
msg_reboot      db 'reboot', 0
name_pm         db 'pm', 0
name_kbc        db 'kbc', 0
name_tf         db 'tf', 0
name_a20        db 'a20', 0
name_resume     db 'resume', 0
name_ext        db 'int15 88h', 0
name_move       db 'int15 87h', 0
name_fault      db 'int15 87h fault', 0

prog_end:
        ; Fails to assemble if the program outgrows the sectors the boot
        ; sector loads.
        times PROG_SECTORS * 512 - (prog_end - prog_start) db 0
        times IMAGE_SIZE - 512 - PROG_SECTORS * 512 db 0
