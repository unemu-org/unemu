; SPDX-License-Identifier: AGPL-3.0-or-later
;
; at-loadall-test: an open 80286 LOADALL test for PC/AT machines.
;
; A bootable 1.2M floppy. The boot sector loads the program. In real mode,
; it turns A20 on through the 8042, then copies a block of conventional
; memory to physical 150000h, above 1 MiB, and back, the way a 286
; extended-memory manager does without leaving real mode: it writes a
; LOADALL table at physical 800h whose ES (or DS) descriptor cache has a
; base above 1 MiB, executes LOADALL (0F 05) and runs REP MOVSW through
; that cache. A real-mode segment load then ends the cache. Last, it
; reports what 0F 04 does, for information. Results go to the BIOS teletype
; and, byte for byte, to COM1. The program halts with interrupts disabled
; after its last line. See vendor/at-loadall-test/README.md.
;
; References:
; - Intel, "Undocumented iAPX 286 Test Instruction": LOADALL is 0F 05
;   (p. 2); in real mode, or in protected mode at privilege level 0, it
;   reads 102 bytes from physical 000800H and loads every register,
;   including the hidden descriptor caches, from them (pp. 1-2, "LOADALL
;   Memory Area Format"); a cache entry is the 24-bit base, least
;   significant byte first, the access rights byte and the 16-bit limit;
;   GDTR and IDTR use the same layout with a zero byte in place of the
;   rights; a loaded cache addresses memory until its segment register is
;   loaded again, and a real-mode load resets the limit to FFFFH (p. 3);
;   SS must be a writable data segment and CS code (p. 4).
; - Intel 80286 Programmer's Reference Manual, Real Address Mode: the
;   interrupt table lies where IDTR points (SIDT and SGDT store the limit,
;   then the 24-bit base); Appendix B: SMSW, SGDT and SIDT run in real mode;
;   an undefined opcode raises interrupt 6 with CS:IP on the instruction.
; - IBM Personal Computer AT Technical Reference, System Board, Keyboard
;   Controller: command D1h writes the output port; bit 1 gates A20. BIOS
;   listing, GATE_A20 (BIOS1): output-port data DFh turns A20 on.
; - Intel UPI-41A/41AH/42/42AH User's Manual, Data Bus Buffer status:
;   wait for IBF clear before a write.
; - National Semiconductor PC16550D datasheet, Line Control Register (DLAB),
;   Line Status Register bit 5 (THRE) and Transmitter Holding Register.

cpu 286
bits 16

LOAD_SEG        equ 0x0800              ; program segment, physical 08000h
LOAD_BASE       equ LOAD_SEG * 16
PROG_SECTORS    equ 4                   ; sectors 2-5 of track 0, head 0
IMAGE_SIZE      equ 1228800             ; 80 cylinders, 2 heads, 15 sectors
STACK_TOP       equ 0xFFF0

; The block, in the program segment past the loaded program: a source and a
; buffer for the copy read back from above 1 MiB.
MOVE_WORDS      equ 128
MOVE_SRC        equ 0x1000
MOVE_BACK       equ 0x1400
MOVE_PATTERN    equ 0x4C05              ; first word; each next one adds 0101h

; The block above 1 MiB, past the high memory area, so no real-mode address
; reaches it. Its A20-masked alias is also the real-mode base of the
; selector the table gives with it, so one guard area catches a move that
; ignored the loaded base and one that A20 masked.
HIGH_BASE       equ 0x150000
ALIAS_SEG       equ (HIGH_BASE & 0xFFFFF) / 16
GUARD_WORD      equ 0x6DB6

; The LOADALL table: physical addresses from the Intel document, p. 2.
T_MSW           equ 0x806
T_TR            equ 0x816
T_FLAGS         equ 0x818
T_IP            equ 0x81A
T_LDTR          equ 0x81C
T_DS            equ 0x81E
T_SS            equ 0x820
T_CS            equ 0x822
T_ES            equ 0x824
T_DI            equ 0x826
T_SI            equ 0x828
T_BP            equ 0x82A
T_SP            equ 0x82C
T_BX            equ 0x82E
T_DX            equ 0x830
T_CX            equ 0x832
T_AX            equ 0x834
T_ES_CACHE      equ 0x836
T_CS_CACHE      equ 0x83C
T_SS_CACHE      equ 0x842
T_DS_CACHE      equ 0x848
T_GDTR          equ 0x84E
T_LDT_CACHE     equ 0x854
T_IDTR          equ 0x85A
T_TSS_CACHE     equ 0x860
TABLE           equ 0x800
TABLE_END       equ 0x866               ; 102 bytes

; Access rights bytes: present, DPL 0, accessed; execute/read code and
; writable data.
RIGHTS_CODE     equ 0x9B
RIGHTS_DATA     equ 0x93

; FLAGS the table loads: bit 1 is always set; CF set as a marker; IF clear,
; so no interrupt handler reloads DS or ES while a loaded cache is in use.
TABLE_FLAGS     equ 0x0003

; Values the table loads into the registers the move does not use.
MARK_AX         equ 0x4C41
MARK_BX         equ 0x1BB1
MARK_DX         equ 0x2DD2
MARK_BP         equ 0x4BB4

; Result bits of a LOADALL line.
LA_UD           equ 0x0001              ; 0F 05 raised interrupt 6
LA_REGS         equ 0x0002              ; AX, BX, CX, DX or BP not loaded
LA_FLAGS        equ 0x0004              ; CF not loaded
LA_ALIAS        equ 0x0008              ; the guard at the alias changed
LA_DATA         equ 0x0010              ; the block read back differs

A20_WRAP        equ 0x0001              ; FFFF:0010h still aliases 0000:0000h

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
boot_error      db 'at-loadall-test: boot read error', 13, 10, 0

        times 510 - ($ - $$) db 0
        dw 0xAA55

;--------------------------------------------------------------------------
; Program, loaded at LOAD_SEG:0000h.
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
        mov si, msg_header
        call print

        ; Interrupt 6 marks an undefined opcode and resumes after its two
        ; bytes, so a CPU without LOADALL still reaches the report.
        xor ax, ax
        mov es, ax
        mov word [es:6 * 4], ud_handler
        mov [es:6 * 4 + 2], cs

        ; A20 on: physical 150000h has address bit 20 set.
        mov al, 0xDF
        call kbc_write_output
        call wrap_test
        xor dx, dx
        test al, al
        jz .a20_on
        or dx, A20_WRAP
.a20_on:
        mov si, name_a20
        call report

        ; The source holds a numbered pattern, the read-back buffer zeros,
        ; the alias the guard word.
        mov ax, ds
        mov es, ax
        mov di, MOVE_SRC
        mov cx, MOVE_WORDS
        mov ax, MOVE_PATTERN
.fill:
        stosw
        add ax, 0x0101
        loop .fill
        mov di, MOVE_BACK
        mov cx, MOVE_WORDS
        xor ax, ax
        rep stosw
        mov ax, ALIAS_SEG
        mov es, ax
        xor di, di
        mov cx, MOVE_WORDS
        mov ax, GUARD_WORD
        rep stosw

;--------------------------------------------------------------------------
; Write: DS is the program segment, ES's cache has base HIGH_BASE under the
; selector ALIAS_SEG.
;--------------------------------------------------------------------------
        mov word [phase_bits], 0
        cli
        xor ax, ax
        mov es, ax
        call table_common
        mov word [es:T_DS], LOAD_SEG
        mov di, T_DS_CACHE
        mov ax, LOAD_BASE & 0xFFFF
        mov dl, LOAD_BASE >> 16
        call put_data_cache
        mov word [es:T_ES], ALIAS_SEG
        mov di, T_ES_CACHE
        mov ax, HIGH_BASE & 0xFFFF
        mov dl, HIGH_BASE >> 16
        call put_data_cache
        mov word [es:T_SI], MOVE_SRC
        mov word [es:T_DI], 0
        mov word [es:T_IP], write_loaded
        mov byte [ud_seen], 0
        mov [es:T_SP], sp
        db 0x0F, 0x05                   ; LOADALL
write_loaded:
        pushf
        call check_loaded
        jnz .no_move
        rep movsw
.no_move:
        call end_caches

        ; The alias still holds the guard words. The ES load above ended
        ; the cache, so this reads conventional memory again.
        mov ax, ALIAS_SEG
        mov es, ax
        xor di, di
        mov cx, MOVE_WORDS
        mov ax, GUARD_WORD
        repe scasw
        je .guard
        or word [phase_bits], LA_ALIAS
.guard:
        mov dx, [phase_bits]
        mov si, name_write
        call report

;--------------------------------------------------------------------------
; Read back: DS's cache has base HIGH_BASE under the selector ALIAS_SEG,
; ES is the program segment.
;--------------------------------------------------------------------------
        mov word [phase_bits], 0
        cli
        xor ax, ax
        mov es, ax
        call table_common
        mov word [es:T_DS], ALIAS_SEG
        mov di, T_DS_CACHE
        mov ax, HIGH_BASE & 0xFFFF
        mov dl, HIGH_BASE >> 16
        call put_data_cache
        mov word [es:T_ES], LOAD_SEG
        mov di, T_ES_CACHE
        mov ax, LOAD_BASE & 0xFFFF
        mov dl, LOAD_BASE >> 16
        call put_data_cache
        mov word [es:T_SI], 0
        mov word [es:T_DI], MOVE_BACK
        mov word [es:T_IP], read_loaded
        mov byte [ud_seen], 0
        mov [es:T_SP], sp
        db 0x0F, 0x05                   ; LOADALL
read_loaded:
        pushf
        call check_loaded
        jnz .no_move
        rep movsw
.no_move:
        call end_caches

        mov ax, ds
        mov es, ax
        mov si, MOVE_SRC
        mov di, MOVE_BACK
        mov cx, MOVE_WORDS
        repe cmpsw
        je .same
        or word [phase_bits], LA_DATA
.same:
        mov dx, [phase_bits]
        mov si, name_read
        call report

;--------------------------------------------------------------------------
; 0F 04, for information: the vector it raised, or "none".
;--------------------------------------------------------------------------
        mov si, name_0f04
        call print
        mov byte [ud_seen], 0
        db 0x0F, 0x04
        cmp byte [ud_seen], 0
        je .no_vector
        mov si, msg_int06
        jmp .vector_done
.no_vector:
        mov si, msg_none
.vector_done:
        call print

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
; LOADALL support.
;--------------------------------------------------------------------------

; Fill the table at ES:TABLE (ES = 0) with the state that both moves share:
; the current MSW, GDTR and IDTR, CS and SS as they are, FLAGS, the
; register markers and the word count. LDTR, TR and their caches are zero,
; which marks the caches invalid; real mode never uses them.
; In: DS = program segment. The caller fills DS, ES, SI, DI, IP and SP.
table_common:
        mov di, TABLE
        mov cx, (TABLE_END - TABLE) / 2
        xor ax, ax
        rep stosw
        smsw ax
        mov [es:T_MSW], ax
        mov word [es:T_FLAGS], TABLE_FLAGS
        mov [es:T_CS], cs
        mov [es:T_SS], ss
        mov word [es:T_AX], MARK_AX
        mov word [es:T_BX], MARK_BX
        mov word [es:T_CX], MOVE_WORDS
        mov word [es:T_DX], MARK_DX
        mov word [es:T_BP], MARK_BP
        mov di, T_CS_CACHE
        mov ax, LOAD_BASE & 0xFFFF
        mov dl, LOAD_BASE >> 16
        mov bl, RIGHTS_CODE
        call put_cache
        mov di, T_SS_CACHE
        mov ax, LOAD_BASE & 0xFFFF
        mov dl, LOAD_BASE >> 16
        call put_data_cache
        sgdt [table_reg]
        mov di, T_GDTR
        call put_table_reg
        sidt [table_reg]
        mov di, T_IDTR
        jmp put_table_reg

; In: ES:DI = cache entry, AX = base bits 15:0, DL = base bits 23:16.
put_data_cache:
        mov bl, RIGHTS_DATA
; In: as above, BL = access rights. The limit is FFFFh, as in real mode.
put_cache:
        mov [es:di], ax
        mov [es:di + 2], dl
        mov [es:di + 3], bl
        mov word [es:di + 4], 0xFFFF
        ret

; In: ES:DI = GDTR or IDTR entry, table_reg = what SGDT or SIDT stored
; (limit, then base). The entry is base, a zero byte, then limit.
put_table_reg:
        mov ax, [table_reg + 2]
        mov [es:di], ax
        mov al, [table_reg + 4]
        mov [es:di + 2], al
        mov byte [es:di + 3], 0
        mov ax, [table_reg]
        mov [es:di + 4], ax
        ret

; Right after LOADALL: DS or ES may address memory above 1 MiB, so every
; variable is reached through SS, whose cache holds the program segment.
; Out: ZF set when LOADALL loaded the markers and the word count, so the
; move may run; otherwise phase_bits records why not. Keeps CX, SI and DI.
check_loaded:
        cmp byte [ss:ud_seen], 0
        je .loaded
        or word [ss:phase_bits], LA_UD
        ret
.loaded:
        cmp ax, MARK_AX
        jne .regs
        cmp bx, MARK_BX
        jne .regs
        cmp dx, MARK_DX
        jne .regs
        cmp bp, MARK_BP
        jne .regs
        cmp cx, MOVE_WORDS
        jne .regs
        ret
.regs:
        or word [ss:phase_bits], LA_REGS
        ret

; A real-mode load of DS and ES ends the caches LOADALL loaded. Then the
; FLAGS that LOADALL loaded, pushed right after it, must have CF set.
; In: the FLAGS word just below the return address.
end_caches:
        mov ax, cs
        mov ds, ax
        mov es, ax
        mov bp, sp
        test byte [bp + 2], 1
        jnz .cf
        or word [phase_bits], LA_FLAGS
.cf:
        ret 2

; Interrupt 6: count it and resume after the two-byte opcode. The return
; address is the faulting instruction. DS may not be the program segment.
ud_handler:
        push bp
        mov bp, sp
        add word [bp + 2], 2
        mov byte [ss:ud_seen], 1
        pop bp
        iret

;--------------------------------------------------------------------------
; 8042 and COM1 access.
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

; Print "<stage> ok", or "<stage> FAIL XXXX" and count the failure.
; In: SI = stage name, DX = failure bits (0 = passed).
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
phase_bits      dw 0
ud_seen         db 0
table_reg       times 6 db 0

msg_header      db 'at-loadall-test', 13, 10, 0
msg_failures    db 'failures ', 0
msg_ok          db ' ok', 13, 10, 0
msg_fail        db ' FAIL ', 0
msg_crlf        db 13, 10, 0
msg_int06       db ' int 06', 13, 10, 0
msg_none        db ' none', 13, 10, 0
name_a20        db 'a20', 0
name_write      db 'loadall write', 0
name_read       db 'loadall read', 0
name_0f04       db '0f04', 0

prog_end:
        ; Fails to assemble if the program outgrows the sectors the boot
        ; sector loads, or reaches the block buffers.
        times PROG_SECTORS * 512 - (prog_end - prog_start) db 0
%if PROG_SECTORS * 512 > MOVE_SRC
%error "the program overlaps the block buffers"
%endif
        times IMAGE_SIZE - 512 - PROG_SECTORS * 512 db 0
