; SPDX-License-Identifier: AGPL-3.0-or-later
;
; at-privilege-test: an open 80286 privilege-level test for PC/AT machines.
;
; A bootable 1.2M floppy. The boot sector loads the program. In real mode it
; builds a GDT with ring-0 and ring-3 code, data and stack segments, an IDT
; and a 286 TSS whose ring-0 stack is SS0:SP0, then enters protected mode
; and loads TR. At CPL 0 it then:
; (a) returns with IRET to CPL 3 with IOPL 3, the way a protected-mode
;     kernel starts its first ring-3 program;
; (b) from ring 3 raises INT 40h through a DPL-3 interrupt gate to a
;     ring-0 handler, which records the stack it runs on and the frame;
; (c) returns from that handler with IRET to ring 3 with IOPL 0, ES holding
;     a DPL-0 data segment and DS a DPL-3 one;
; (d) at ring 3 with IOPL 0 executes CLI, which raises #GP(0) to a ring-0
;     handler.
; Ring 3 records what it sees in the program's variables through its own
; DPL-3 data segment. Back at CPL 0, the program prints one line per case
; and a failure count, to COM1 and straight into the text screen, then
; halts with interrupts disabled. It never returns to real mode. See
; vendor/at-privilege-test/README.md.
;
; References:
; - Intel 80286 Programmer's Reference Manual, Protection: a descriptor's
;   DPL, a selector's RPL, the CPL as the RPL of CS (Privilege; Descriptor
;   Privilege); the access rights byte of code and data segments (present,
;   DPL, S, type) and of system descriptors (type 1: available 286 TSS;
;   type 6: 286 interrupt gate).
; - Intel 80286 Programmer's Reference Manual, Interrupts and Exceptions:
;   an interrupt gate to a more privileged code segment switches to the
;   stack of the new level from the TSS and pushes the old SS, SP, FLAGS,
;   CS and IP, then an error code for #GP; an interrupt gate clears IF;
;   INT n faults #GP when the gate's DPL is below CPL; exceptions ignore
;   the gate's DPL.
; - Intel 80286 Programmer's Reference Manual, Multitasking: the 286 TSS
;   holds SP0, SS0, SP1, SS1, SP2 and SS2 at offsets 2 to 0Ch; LTR loads TR.
; - Intel 80286 Programmer's Reference Manual, Appendix B, IRET: a return
;   to an outer level pops IP, CS, FLAGS, then SP and SS of that level, and
;   loads DS and ES with the null selector when they hold a data segment
;   whose DPL is below the new CPL; IOPL loads only at CPL 0. CLI and STI
;   raise #GP(0) when CPL is above IOPL.
; - Intel 80286 Programmer's Reference Manual, System Control and
;   Initialization: LMSW sets PE, followed by an intersegment jump.
; - IBM Personal Computer AT Technical Reference, BIOS listing, VIDEO_IO:
;   0040:0049h is the video mode; mode 7 uses the monochrome buffer at
;   B0000h, the others the colour buffer at B8000h.
; - National Semiconductor PC16550D datasheet, Line Control Register (DLAB),
;   Line Status Register bit 5 (THRE) and Transmitter Holding Register.

cpu 286
bits 16

LOAD_SEG        equ 0x0800              ; program segment, physical 08000h
LOAD_BASE       equ LOAD_SEG * 16
PROG_SECTORS    equ 6                   ; sectors 2-7 of track 0, head 0
IMAGE_SIZE      equ 1228800             ; 80 cylinders, 2 heads, 15 sectors
STACK_TOP       equ 0xFFF0

; Selectors. Ring-3 selectors carry RPL 3.
SEL_CODE0       equ 0x08
SEL_DATA0       equ 0x10
SEL_STACK0      equ 0x18
SEL_VIDEO       equ 0x20
SEL_TSS         equ 0x28
SEL_CODE3       equ 0x33
SEL_DATA3       equ 0x3B
SEL_STACK3      equ 0x43
GDT_ENTRIES     equ 9

; The ring stacks lie apart from the program, so a frame on the wrong one
; shows in its selector and its offset.
STACK0_BASE     equ 0x20000
STACK3_BASE     equ 0x30000
SP0             equ 0x0800
RING3_SP        equ 0x2000

; Access rights bytes.
RIGHTS_CODE0    equ 0x9B                ; present, DPL 0, execute/read
RIGHTS_DATA0    equ 0x93                ; present, DPL 0, read/write
RIGHTS_CODE3    equ 0xFB                ; present, DPL 3, execute/read
RIGHTS_DATA3    equ 0xF3                ; present, DPL 3, read/write
RIGHTS_TSS      equ 0x81                ; present, DPL 0, available 286 TSS
GATE_INT0       equ 0x86                ; present, DPL 0, 286 interrupt gate
GATE_INT3       equ 0xE6                ; present, DPL 3, 286 interrupt gate

VEC_TO_RING0    equ 0x40                ; case (b)
VEC_LEAVE       equ 0x41                ; ring 3 gives up: no #GP in case (d)
IDT_ENTRIES     equ 0x42

; FLAGS images. Bit 1 always reads 1; IF stays clear throughout, so no
; hardware interrupt reaches the IDT.
FLAGS_RING3     equ 0x3002              ; case (a): IOPL 3
FLAGS_INT       equ 0x3003              ; POPF before INT 40h: CF as a marker
FLAGS_IOPL0     equ 0x0003              ; case (c): the handler clears IOPL

; Result bits, per line.
NOT_REACHED     equ 0x0001
A_CS            equ 0x0002              ; CS is not the ring-3 selector
A_SS            equ 0x0004              ; SS is not the ring-3 stack
A_SP            equ 0x0008              ; SP is not the ring-3 SP
A_FLAGS         equ 0x0010              ; IOPL not 3, or IF set
B_CS            equ 0x0002              ; the handler's CS is not ring 0
B_STACK         equ 0x0004              ; not on SS0:SP0 less the frame
B_RETURN        equ 0x0008              ; pushed CS:IP not the ring-3 return
B_FLAGS         equ 0x0010              ; pushed FLAGS not the ring-3 FLAGS
B_OUTER         equ 0x0020              ; pushed SS:SP not the ring-3 stack
C_STACK         equ 0x0002              ; CS, SS or SP not ring 3's
C_DS            equ 0x0004              ; DS (DPL 3) did not keep its value
C_ES            equ 0x0008              ; ES (DPL 0) was not nulled
C_FLAGS         equ 0x0010              ; FLAGS not the handler's image
D_ERROR         equ 0x0002              ; error code not 0
D_RETURN        equ 0x0004              ; pushed CS:IP not the CLI
D_OUTER         equ 0x0008              ; pushed SS:SP not the ring-3 stack
D_STACK         equ 0x0010              ; not on SS0:SP0 less the frame
D_FLAGS         equ 0x0020              ; pushed FLAGS not ring 3's

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
boot_error      db 'at-privilege-test: boot read error', 13, 10, 0

        times 510 - ($ - $$) db 0
        dw 0xAA55

;--------------------------------------------------------------------------
; Program, loaded at LOAD_SEG:0000h. Every segment below has base
; LOAD_BASE except the stacks and the screen, so one offset names the same
; byte at every level.
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
        call uart_init

        ; The screen descriptor follows the BIOS video mode.
        push ds
        mov ax, 0x0040
        mov ds, ax
        mov al, [0x0049]
        pop ds
        cmp al, 7
        je .mono
        mov byte [gdt + SEL_VIDEO + 4], 0x0B
        mov word [gdt + SEL_VIDEO + 2], 0x8000
.mono:

        ; Every vector first goes to its own stub, which reports it as
        ; unexpected; the vectors the cases use are then replaced.
        mov di, idt
        mov ax, unexpected_stubs
        mov cx, IDT_ENTRIES
.gate:
        mov bl, GATE_INT0
        call put_gate
        add ax, STUB_SIZE
        loop .gate
        mov di, idt + 13 * 8
        mov ax, gp_handler
        call put_gate
        mov di, idt + VEC_TO_RING0 * 8
        mov ax, to_ring0_handler
        mov bl, GATE_INT3
        call put_gate
        mov di, idt + VEC_LEAVE * 8
        mov ax, leave_handler
        mov bl, GATE_INT3
        call put_gate

        lgdt [gdtr]
        lidt [idtr]
        smsw ax
        or ax, 1
        lmsw ax
        jmp SEL_CODE0:protected

;--------------------------------------------------------------------------
; Protected mode, CPL 0.
;--------------------------------------------------------------------------
protected:
        mov ax, SEL_DATA0
        mov ds, ax
        mov es, ax
        mov ss, ax
        mov sp, STACK_TOP
        mov ax, SEL_TSS
        ltr ax
        call screen_clear
        mov si, msg_header
        call print

;--------------------------------------------------------------------------
; (a) IRET from CPL 0 to CPL 3 with IOPL 3. DS and ES hold the DPL-3 data
; segment, which an outward return keeps.
;--------------------------------------------------------------------------
        mov ax, SEL_DATA3
        mov ds, ax
        mov es, ax
        push word SEL_STACK3
        push word RING3_SP
        push word FLAGS_RING3
        push word SEL_CODE3
        push word ring3_entry
        iret

;--------------------------------------------------------------------------
; Ring 3. DS is the DPL-3 data segment, so the variables are reachable.
;--------------------------------------------------------------------------
ring3_entry:
        mov ax, cs
        mov [a_seen + 0], ax
        mov [a_seen + 2], ss
        mov [a_seen + 4], sp
        pushf
        pop word [a_seen + 6]
        mov byte [a_reached], 1
        ; IOPL 3 lets ring 3 execute CLI; a #GP here is reported as
        ; unexpected.
        cli

        ; (b) INT 40h through the DPL-3 gate, with known FLAGS.
        push word FLAGS_INT
        popf
        int VEC_TO_RING0
ring3_after_int:
        ; (c) Back from the handler's IRET.
        mov ax, cs
        mov [c_seen + 0], ax
        mov [c_seen + 2], ss
        mov [c_seen + 4], sp
        pushf
        pop word [c_seen + 6]
        mov [c_seen + 8], ds
        mov [c_seen + 10], es
        mov byte [c_reached], 1

        ; (d) CLI at ring 3 with IOPL 0.
        mov byte [gp_expected], 1
ring3_cli:
        cli
        mov byte [gp_expected], 0
        int VEC_LEAVE

;--------------------------------------------------------------------------
; Ring-0 handlers.
;--------------------------------------------------------------------------

; (b) INT 40h from ring 3. The interrupted DS is the DPL-3 data segment,
; which ring 0 may also use; the frame is copied before anything else
; moves SP.
to_ring0_handler:
        mov [b_seen + 0], ss
        mov [b_seen + 2], sp
        mov ax, cs
        mov [b_seen + 4], ax
        mov bp, sp
        mov ax, [ss:bp + 0]
        mov [b_frame + 0], ax
        mov ax, [ss:bp + 2]
        mov [b_frame + 2], ax
        mov ax, [ss:bp + 4]
        mov [b_frame + 4], ax
        mov ax, [ss:bp + 6]
        mov [b_frame + 6], ax
        mov ax, [ss:bp + 8]
        mov [b_frame + 8], ax
        mov byte [b_reached], 1
        ; (c) Return to ring 3 with IOPL 0, ES holding the DPL-0 data
        ; segment and DS the DPL-3 one.
        and word [ss:bp + 4], 0xCFFF
        mov ax, SEL_DATA3
        mov ds, ax
        mov ax, SEL_DATA0
        mov es, ax
        iret

; #GP. Only case (d) expects one; any other is unexpected. The handler
; never returns to ring 3: it continues at CPL 0 on the program's stack.
gp_handler:
        push ds
        push ax
        mov ax, SEL_DATA0
        mov ds, ax
        cmp byte [gp_expected], 0
        jne .expected
        pop ax
        pop ds
        push word 13
        jmp unexpected
.expected:
        mov byte [gp_expected], 0
        mov bp, sp
        lea ax, [bp + 4]
        mov [d_seen + 0], ss
        mov [d_seen + 2], ax
        ; Error code, IP, CS, FLAGS, SP and SS.
        mov ax, [ss:bp + 4]
        mov [d_frame + 0], ax
        mov ax, [ss:bp + 6]
        mov [d_frame + 2], ax
        mov ax, [ss:bp + 8]
        mov [d_frame + 4], ax
        mov ax, [ss:bp + 10]
        mov [d_frame + 6], ax
        mov ax, [ss:bp + 12]
        mov [d_frame + 8], ax
        mov ax, [ss:bp + 14]
        mov [d_frame + 10], ax
        mov byte [d_reached], 1
        jmp ring0_resume

; INT 41h: ring 3 ran past the CLI without a #GP.
leave_handler:
        jmp ring0_resume

; One stub per vector: push the vector, then report it.
STUB_SIZE       equ 5
unexpected_stubs:
%assign vector 0
%rep IDT_ENTRIES
        push byte vector
        jmp strict near unexpected
%assign vector vector + 1
%endrep

; An exception or interrupt no case expects. The stack holds the vector,
; then the frame the CPU pushed; the line shows the vector and three words.
unexpected:
        mov ax, SEL_DATA0
        mov ds, ax
        ; A second one came from the report itself: stop there.
        cmp byte [u_reached], 0
        jne halt_forever
        mov bp, sp
        mov ax, [ss:bp + 0]
        mov [u_seen + 0], ax
        mov ax, [ss:bp + 2]
        mov [u_seen + 2], ax
        mov ax, [ss:bp + 4]
        mov [u_seen + 4], ax
        mov ax, [ss:bp + 6]
        mov [u_seen + 6], ax
        mov byte [u_reached], 1
        ; fall through

; Back to CPL 0 on the program's stack, whatever ring ran last.
ring0_resume:
        mov ax, SEL_DATA0
        mov ds, ax
        mov es, ax
        mov ss, ax
        mov sp, STACK_TOP

;--------------------------------------------------------------------------
; Report, at CPL 0.
;--------------------------------------------------------------------------
        ; (a)
        mov dx, NOT_REACHED
        cmp byte [a_reached], 0
        je .a_report
        xor dx, dx
        cmp word [a_seen + 0], SEL_CODE3
        je .a_cs
        or dx, A_CS
.a_cs:
        cmp word [a_seen + 2], SEL_STACK3
        je .a_ss
        or dx, A_SS
.a_ss:
        cmp word [a_seen + 4], RING3_SP
        je .a_sp
        or dx, A_SP
.a_sp:
        mov ax, [a_seen + 6]
        and ax, 0x3200
        cmp ax, 0x3000
        je .a_report
        or dx, A_FLAGS
.a_report:
        mov si, name_a
        call report

        ; (b)
        mov dx, NOT_REACHED
        cmp byte [b_reached], 0
        je .b_report
        xor dx, dx
        cmp word [b_seen + 4], SEL_CODE0
        je .b_cs
        or dx, B_CS
.b_cs:
        cmp word [b_seen + 0], SEL_STACK0
        jne .b_stack_bad
        cmp word [b_seen + 2], SP0 - 10
        je .b_stack
.b_stack_bad:
        or dx, B_STACK
.b_stack:
        cmp word [b_frame + 0], ring3_after_int
        jne .b_return_bad
        cmp word [b_frame + 2], SEL_CODE3
        je .b_return
.b_return_bad:
        or dx, B_RETURN
.b_return:
        cmp word [b_frame + 4], FLAGS_INT
        je .b_flags
        or dx, B_FLAGS
.b_flags:
        cmp word [b_frame + 6], RING3_SP
        jne .b_outer_bad
        cmp word [b_frame + 8], SEL_STACK3
        je .b_report
.b_outer_bad:
        or dx, B_OUTER
.b_report:
        mov si, name_b
        call report

        ; (c)
        mov dx, NOT_REACHED
        cmp byte [c_reached], 0
        je .c_report
        xor dx, dx
        cmp word [c_seen + 0], SEL_CODE3
        jne .c_stack_bad
        cmp word [c_seen + 2], SEL_STACK3
        jne .c_stack_bad
        cmp word [c_seen + 4], RING3_SP
        je .c_stack
.c_stack_bad:
        or dx, C_STACK
.c_stack:
        cmp word [c_seen + 6], FLAGS_IOPL0
        je .c_flags
        or dx, C_FLAGS
.c_flags:
        cmp word [c_seen + 8], SEL_DATA3
        je .c_ds
        or dx, C_DS
.c_ds:
        cmp word [c_seen + 10], 0
        je .c_report
        or dx, C_ES
.c_report:
        mov si, name_c
        call report

        ; (d)
        mov dx, NOT_REACHED
        cmp byte [d_reached], 0
        je .d_report
        xor dx, dx
        cmp word [d_frame + 0], 0
        je .d_error
        or dx, D_ERROR
.d_error:
        cmp word [d_frame + 2], ring3_cli
        jne .d_return_bad
        cmp word [d_frame + 4], SEL_CODE3
        je .d_return
.d_return_bad:
        or dx, D_RETURN
.d_return:
        cmp word [d_frame + 6], FLAGS_IOPL0
        je .d_flags
        or dx, D_FLAGS
.d_flags:
        cmp word [d_frame + 8], RING3_SP
        jne .d_outer_bad
        cmp word [d_frame + 10], SEL_STACK3
        je .d_outer
.d_outer_bad:
        or dx, D_OUTER
.d_outer:
        cmp word [d_seen + 0], SEL_STACK0
        jne .d_stack_bad
        cmp word [d_seen + 2], SP0 - 12
        je .d_report
.d_stack_bad:
        or dx, D_STACK
.d_report:
        mov si, name_d
        call report

        cmp byte [u_reached], 0
        je .counted
        inc word [fails]
        mov si, msg_unexpected
        call print
        mov ax, [u_seen + 0]
        call print_hex4
        mov cx, 3
        mov bx, u_seen + 2
.word:
        mov al, ' '
        call put_char
        mov ax, [bx]
        call print_hex4
        add bx, 2
        loop .word
        mov si, msg_crlf
        call print
.counted:
        mov si, msg_failures
        call print
        mov ax, [fails]
        call print_dec
        mov si, msg_crlf
        call print
halt_forever:
        cli
        hlt
        jmp halt_forever

;--------------------------------------------------------------------------
; Real-mode setup helper.
;--------------------------------------------------------------------------

; In: ES:DI = IDT entry, AX = handler offset in the ring-0 code segment,
; BL = access rights. Keeps AX and BL; DI moves past the entry.
put_gate:
        stosw
        push ax
        mov ax, SEL_CODE0
        stosw
        xor al, al
        stosb
        mov al, bl
        stosb
        xor ax, ax
        stosw
        pop ax
        ret

;--------------------------------------------------------------------------
; Output at CPL 0: COM1 and the text screen. DS = SEL_DATA0.
;--------------------------------------------------------------------------

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

; Blank 80x25 cells, attribute 07h; the cursor goes to the top left.
screen_clear:
        push es
        mov ax, SEL_VIDEO
        mov es, ax
        xor di, di
        mov cx, 80 * 25
        mov ax, 0x0720
        rep stosw
        pop es
        mov word [screen_pos], 0
        mov word [screen_line], 0
        ret

; In: AL = character. Sends it to COM1 and writes it on the screen: CR
; returns to the line start, LF moves to the next line (the last line is
; reused), anything else fills the next cell.
put_char:
        pusha
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
        mov ax, SEL_VIDEO
        mov es, ax
        cmp bl, 13
        je .cr
        cmp bl, 10
        je .lf
        mov di, [screen_pos]
        mov al, bl
        mov ah, 0x07
        stosw
        mov [screen_pos], di
        jmp .done
.lf:
        cmp word [screen_line], 24 * 160
        jae .cr
        add word [screen_line], 160
.cr:
        mov ax, [screen_line]
        mov [screen_pos], ax
.done:
        pop es
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

; In: AX = value, printed as four hex digits.
print_hex4:
        pusha
        mov bx, ax
        mov cx, 4
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

; Print "<case> ok", or "<case> FAIL XXXX" and count the failure.
; In: SI = case name, DX = failure bits (0 = passed).
report:
        pusha
        call print
        test dx, dx
        jnz .fail
        mov si, msg_ok
        call print
        jmp .done
.fail:
        inc word [fails]
        mov si, msg_fail
        call print
        mov ax, dx
        call print_hex4
        mov si, msg_crlf
        call print
.done:
        popa
        ret

;--------------------------------------------------------------------------
; Data.
;--------------------------------------------------------------------------
fails           dw 0
screen_pos      dw 0
screen_line     dw 0
gp_expected     db 0
a_reached       db 0
b_reached       db 0
c_reached       db 0
d_reached       db 0
u_reached       db 0
a_seen          times 4 dw 0            ; CS, SS, SP, FLAGS
b_seen          times 3 dw 0            ; SS, SP, CS in the handler
b_frame         times 5 dw 0            ; IP, CS, FLAGS, SP, SS
c_seen          times 6 dw 0            ; CS, SS, SP, FLAGS, DS, ES
d_seen          times 2 dw 0            ; SS, SP of the frame
d_frame         times 6 dw 0            ; error, IP, CS, FLAGS, SP, SS
u_seen          times 4 dw 0            ; vector, three frame words

msg_header      db 'at-privilege-test', 13, 10, 0
msg_failures    db 'failures ', 0
msg_ok          db ' ok', 13, 10, 0
msg_fail        db ' FAIL ', 0
msg_crlf        db 13, 10, 0
msg_unexpected  db 'unexpected ', 0
name_a          db 'iret to ring 3', 0
name_b          db 'int to ring 0', 0
name_c          db 'iret nulls es', 0
name_d          db 'cli gp at ring 3', 0

; Pseudo-descriptors for LGDT and LIDT: limit, then the 24-bit base. The
; program lies below 64 KiB of physical memory, so the top byte is zero.
gdtr            dw GDT_ENTRIES * 8 - 1, gdt + LOAD_BASE
                db 0, 0
idtr            dw IDT_ENTRIES * 8 - 1, idt + LOAD_BASE
                db 0, 0

; The 286 TSS. Only the ring-0 stack is used; the task never switches, so
; the rest stays zero.
tss:
                dw 0                    ; back link
                dw SP0, SEL_STACK0      ; SP0, SS0
                times 44 - ($ - tss) db 0

        align 8
; Descriptor: limit, base bits 15:0, base bits 23:16, rights, reserved.
gdt:
                times 8 db 0
                dw 0xFFFF, LOAD_BASE    ; SEL_CODE0
                db 0, RIGHTS_CODE0
                dw 0
                dw 0xFFFF, LOAD_BASE    ; SEL_DATA0
                db 0, RIGHTS_DATA0
                dw 0
                dw 0xFFFF, STACK0_BASE & 0xFFFF ; SEL_STACK0
                db STACK0_BASE >> 16, RIGHTS_DATA0
                dw 0
                dw 80 * 25 * 2 - 1, 0   ; SEL_VIDEO: B0000h, set at start
                db 0x0B, RIGHTS_DATA0
                dw 0
                dw 44 - 1, tss + LOAD_BASE ; SEL_TSS
                db 0, RIGHTS_TSS
                dw 0
                dw 0xFFFF, LOAD_BASE    ; SEL_CODE3
                db 0, RIGHTS_CODE3
                dw 0
                dw 0xFFFF, LOAD_BASE    ; SEL_DATA3
                db 0, RIGHTS_DATA3
                dw 0
                dw 0xFFFF, STACK3_BASE & 0xFFFF ; SEL_STACK3
                db STACK3_BASE >> 16, RIGHTS_DATA3
                dw 0

; The IDT, filled at start.
idt:
                times IDT_ENTRIES * 8 db 0

prog_end:
        ; Fails to assemble if the program outgrows the sectors the boot
        ; sector loads.
        times PROG_SECTORS * 512 - (prog_end - prog_start) db 0
        times IMAGE_SIZE - 512 - PROG_SECTORS * 512 db 0
