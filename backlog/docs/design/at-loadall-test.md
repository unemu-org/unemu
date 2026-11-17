# Open AT LOADALL test

`vendor/at-loadall-test/at-loadall-test.asm` is the open check, in the
public test suite, for the undocumented 80286 LOADALL instruction (0F 05).
An extended-memory manager written for the 80286 (HIMEM.SYS 3.x, for one)
copies extended memory with it: from real mode it loads a descriptor cache
whose base lies above 1 MiB and runs REP MOVSW through it, with no switch to
protected mode and no reset to come back. A CPU without LOADALL raises
interrupt 6 there, and DOS stops with "Invalid Opcode" at the first move.
This program does the same move on its own, so the path is tested without
any proprietary guest.

It is a companion of the protected-mode return test
([`at-pm-test.md`](at-pm-test.md)) and follows its conventions: build and
fixture details in
[`vendor/at-loadall-test/README.md`](../../../vendor/at-loadall-test/README.md),
the golden in `crates/pc-harness/tests/at_loadall_test_golden.rs`, the commands in
[`commands.md`](../commands.md#ci).

Evidence labels: **READ** (seen in the named source), **MEASURED** (run in
this repository at the CPU pin in `Cargo.lock`), **ASSUMED** (expected, not
yet run).

## References

- **Intel, "Undocumented iAPX 286 Test Instruction"** (the CPU core keeps it
  as an oracle; cited by its printed page numbers). LOADALL is 0F 05 (p. 2).
  In real mode, or in protected mode at privilege level 0, it reads the
  102-byte table at physical `800h` and loads every register from it: MSW,
  TR, FLAGS, IP, LDTR, the DS, SS, CS and ES selectors, the eight word
  registers, the ES, CS, SS and DS descriptor caches, GDTR, the LDT cache,
  IDTR and the TSS cache (pp. 1–2, "LOADALL Memory Area Format"). A cache
  entry is the 24-bit base (least significant byte first), the access rights
  byte and the 16-bit limit; GDTR and IDTR have a zero byte in place of the
  rights (p. 3). A loaded cache addresses memory until its segment register
  is loaded again; a real-mode load resets the limit to FFFFh (p. 3). SS must
  be writable data and CS code (p. 4). The document also calls the
  instruction 0F04H on p. 13; the CPU core follows p. 2 and keeps 0F 04
  undefined, so 0F 04 is reported for information only.
- **Intel 80286 Programmer's Reference Manual**, Appendix B: SMSW, SGDT and
  SIDT run in real mode; SGDT and SIDT store the limit, then the 24-bit
  base. An undefined opcode raises interrupt 6 with CS:IP on the
  instruction.
- **IBM Personal Computer AT Technical Reference**, System Board, Keyboard
  Controller (output port bit 1 gates A20; command D1h), and BIOS listing,
  GATE_A20 (data DFh turns A20 on).

## Program and output

The boot sector loads the program to `0800:0000h`. It initializes COM1
(`3F8h`, divisor 1, 8N1), points interrupt 6 at its own handler (which
marks the event and resumes after the two opcode bytes, so a CPU without
LOADALL still reaches the report), turns A20 on through the 8042 and checks
that `FFFF:0010h` no longer aliases `0000:0000h`. Interrupts stay disabled
from building each table until DS and ES are loaded again, so no handler
can reload a segment while a loaded cache is in use. Each line goes to the
BIOS teletype (INT 10h AH=0Eh) and the same bytes to COM1:

```text
at-loadall-test
a20 ok
loadall write ok
loadall read ok
0f04 int 06
failures 0
```

A failed line prints `FAIL` and a four-digit hexadecimal mask of the failed
checks. The program then halts with interrupts disabled.

The block is 128 words. The source at `0800:1000h` holds a numbered pattern
(each word 0101h more than the one before); a read-back buffer at
`0800:1400h` starts zeroed. The target is physical `150000h`, above the high
memory area, so no real-mode address reaches it. The table gives its cache
the selector `5000h`, whose real-mode base, `50000h`, is also the A20-masked
alias of `150000h`; that area holds a guard word. A move that used the
selector instead of the loaded base, or that A20 masked, lands on the guard.

Both moves build the table the same way: the current MSW (SMSW), GDTR
(SGDT) and IDTR (SIDT); CS and SS as they are, with caches at the program's
base (rights 9Bh and 93h, limit FFFFh); LDTR, TR and their caches zero
(invalid; real mode never uses them); FLAGS 0003h (CF set as a marker, IF
clear); SP as it is; marker values in AX, BX, DX and BP; CX the word count;
IP the instruction after LOADALL. Then:

| Line | What it does | Mask bits |
|---|---|---|
| `a20` | D1h/DFh, then the wrap test of the protected-mode test. | 1 `FFFF:0010h` still aliases `0000:0000h` |
| `loadall write` | DS is the program segment; ES is selector `5000h` with a cache at base `150000h`. SI and DI point at the source and offset 0. After LOADALL it checks interrupt 6 and the markers, runs REP MOVSW, loads DS and ES from CS, checks the CF that LOADALL loaded and then the guard at `5000:0000h`. | 1 0F 05 raised interrupt 6 (the move is skipped), 2 AX, BX, CX, DX or BP not loaded (the move is skipped), 4 CF not loaded, 8 the guard changed |
| `loadall read` | DS is selector `5000h` with a cache at base `150000h`; ES is the program segment. REP MOVSW copies the block back to the read-back buffer, which must equal the source. | 1, 2 and 4 as above, 10h the buffer differs from the source |
| `0f04` (informational) | Executes 0F 04 and prints `int 06` if it raised interrupt 6, else `none`. Never counted. | none |

The guard check runs after `mov es, ax` reloaded ES with `5000h`. If that
real-mode load did not end the loaded cache, the check would read the block
at `150000h` instead of the guard and fail, so the line also covers the
end of a LOADALL cache. Between LOADALL and REP MOVSW the program runs
comparisons, a call and a return through SS, so the cache outlives other
instructions, as an extended-memory manager needs.

## Results

**Open profile — MEASURED (2026-11-17, x86jit `cf32ec2e`).** The golden
runs the at-games-open BIOS (unemu-at-bios, as the protected-mode test does)
on the same board, memory map (640 KiB + 1 MiB extended), RTC seed and
display switch, without the game cards. Every counted line passes:
`a20 ok`, `loadall write ok`, `loadall read ok`, then `0f04 int 06` and
`failures 0`. The run halts after 6,404,154 retired instructions: one POST,
one floppy boot and the program.

**Previous CPU pin — MEASURED.** At x86jit `ae12d43e`, which raised
interrupt 6 for 0F 05 on the 80286, the same image printed
`loadall write FAIL 0005` and `loadall read FAIL 0015` (interrupt 6, CF not
loaded, and for the read the buffer still zero), `0f04 int 06` and
`failures 2`, after 6,407,327 retired instructions. The golden fails there,
as it should.
