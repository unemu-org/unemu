# Open AT privilege-level test

`vendor/at-privilege-test/at-privilege-test.asm` is the open check, in the
public test suite, for 80286 protected-mode code at CPL 3 and the
transitions between ring 0 and ring 3. A protected-mode kernel for the 286
(Windows 3.x Standard mode, for one) starts its programs with an IRET from
ring 0 to ring 3 and takes interrupts and exceptions back to ring 0 on the
ring-0 stack its TSS names; Windows 3.1 Standard mode also calls ring 0 from
ring 3 through a call gate. A CPU core that runs only at CPL 0 stops at the
first such IRET, and one without call gates at the first such CALL. This
program makes the same transitions on its own, so the path is tested without
any proprietary guest.

It is a companion of the protected-mode return test
([`at-pm-test.md`](at-pm-test.md)) and the LOADALL test
([`at-loadall-test.md`](at-loadall-test.md)) and follows their conventions:
build and fixture details in
[`vendor/at-privilege-test/README.md`](../../../vendor/at-privilege-test/README.md),
the golden in `crates/pc-harness/tests/at_privilege_test_golden.rs`, the
commands in [`commands.md`](../commands.md#ci).

Evidence labels: **READ** (seen in the named source), **MEASURED** (run in
this repository at the CPU pin in `Cargo.lock`), **ASSUMED** (expected, not
yet run).

## References

- **Intel 80286 Programmer's Reference Manual**, Protection: the CPL is the
  RPL of CS; a data segment register may be loaded with a descriptor whose
  DPL is at least the larger of CPL and RPL; the access rights of code and
  data segments and of system descriptors (type 1 an available 286 TSS,
  type 6 a 286 interrupt gate).
- **Intel 80286 Programmer's Reference Manual**, Interrupts and Exceptions:
  INT n through a gate whose DPL is below CPL raises #GP; exceptions ignore
  the gate's DPL. A gate to a more privileged code segment switches to the
  stack of the new level, taken from the TSS, and pushes the old SS, SP,
  FLAGS, CS and IP, then the error code of #GP. An interrupt gate clears IF.
- **Intel 80286 Programmer's Reference Manual**, §7.5.1.1 "Call Gates" and §7.5.1.4: a
  286 call gate (type 4) holds the target offset, the target code selector
  and, in bits 4–0 of byte 4, a word count. A CALL through it from a less
  privileged level to a non-conforming code segment (§7.5.1.4, pp. 7-20–21)
  switches to the stack of the target's level from the TSS, pushes the old
  SS and SP, copies the word count's parameters from the old stack in their
  order, and pushes the return CS and IP.
- **Intel 80286 Programmer's Reference Manual**, Appendix B, CALL
  (pp. B-24–25) and RET (pp. B-94–95): the gate's DPL must be at least CPL
  and the selector's RPL; RET n to an outer level pops IP and CS, releases n
  bytes, then pops the SP and SS of that level. That the outer SS:SP is then
  adjusted by the n bytes too is §7.5.2 "Inter-Level Returns" (p. 7-21) and
  Table 7-4 (p. 7-22).
- **Intel 80286 Programmer's Reference Manual**, Multitasking: the 286 TSS
  holds SP0, SS0, SP1, SS1, SP2 and SS2 at offsets 2 to 0Ch; LTR loads TR
  and marks the TSS busy.
- **Intel 80286 Programmer's Reference Manual**, Appendix B (IRET, p. B-52)
  and §7.5.2 "Inter-Level Returns" (p. 7-21): IRET to an outer level pops IP,
  CS and FLAGS, then the SP and SS of that level, and loads the null selector
  into a data segment register whose descriptor's DPL is below the new CPL
  (the pseudocode's validity test reads "DPL ≥ CPL, or DPL ≥ RPL", which
  taken literally would never null; the prose of §7.5.2 gives the rule);
  IOPL is loaded only at CPL 0. CLI raises #GP(0) when CPL is above IOPL.
- **IBM Personal Computer AT Technical Reference**, BIOS listing, VIDEO_IO:
  the video mode byte at `0040:0049h`; mode 7 uses the monochrome buffer at
  `B0000h`, the others `B8000h`.

## Program and output

The boot sector loads the program to `0800:0000h`. In real mode it
initializes COM1 (`3F8h`, divisor 1, 8N1), picks the text buffer from the
BIOS video mode and fills the IDT, then loads GDTR and IDTR, sets PE with
LMSW and jumps to the ring-0 code segment. Interrupts stay disabled to the
end; no FLAGS image sets IF, so no hardware interrupt reaches the IDT.

| Selector | Segment |
|---|---|
| `08h` | ring-0 code, base `8000h` (the program), rights 9Bh |
| `10h` | ring-0 data, base `8000h`, rights 93h |
| `18h` | ring-0 stack, base `20000h`, rights 93h: SS0 |
| `20h` | text buffer, base `B0000h` or `B8000h`, rights 93h |
| `28h` | 286 TSS in the program, limit 2Bh: SS0:SP0 = `0018:0800h`, the rest zero |
| `33h` | ring-3 code, base `8000h`, rights FBh |
| `3Bh` | ring-3 data, base `8000h`, rights F3h |
| `43h` | ring-3 stack, base `30000h`, rights F3h |
| `4Bh` | 286 call gate, rights E4h (DPL 3), 2 words, to `08h`:handler |

Every IDT entry up to 41h is a DPL-0 interrupt gate to a stub that reports
its vector as unexpected, except 0Dh (#GP, DPL 0), 40h and 41h (DPL 3).
Code and data share the program's base, so ring 3 writes what it sees into
the program's variables through `3Bh`. At CPL 0 the program loads TR with
`28h`, clears the screen and prints the header. Then:

| Line | What it does | Mask bits |
|---|---|---|
| `iret to ring 3` | (a) With DS and ES `3Bh`, IRET with the frame `43h`:`2000h`, FLAGS 3002h (IOPL 3, IF clear), `33h`:entry. Ring 3 records CS, SS, SP and FLAGS, then executes CLI, which IOPL 3 allows. | 1 not reached, 2 CS not `33h`, 4 SS not `43h`, 8 SP not `2000h`, 10h IOPL not 3 or IF set |
| `int to ring 0` | (b) Ring 3 loads FLAGS 3003h (CF as a marker) with POPF and executes INT 40h (a DPL-3 gate to ring 0). The handler records its CS, SS and SP and the five words the CPU pushed. | 1 not reached, 2 CS not `08h`, 4 SS:SP not `0018:07F6h` (SS0:SP0 less five words), 8 pushed CS:IP not the instruction after INT 40h, 10h pushed FLAGS not 3003h, 20h pushed SS:SP not `0043:2000h` |
| `iret nulls es` | (c) The handler clears IOPL in the pushed FLAGS, loads DS with `3Bh` (DPL 3) and ES with `10h` (DPL 0) and returns with IRET. Ring 3 records CS, SS, SP, FLAGS, DS and ES. | 1 not reached, 2 CS, SS or SP not ring 3's, 4 DS not `3Bh`, 8 ES not null, 10h FLAGS not 0003h |
| `cli gp at ring 3` | (d) Ring 3, now at IOPL 0, executes CLI. The #GP handler records its SS:SP and the six words the CPU pushed, then returns to ring 3 past the CLI with IRET (frame `43h`:`2000h`, FLAGS 0003h, DS and ES `3Bh`) for case (e). | 1 no #GP (ring 3 then goes on to (e) by itself), 2 error code not 0, 4 pushed CS:IP not the CLI, 8 pushed SS:SP not `0043:2000h`, 10h SS:SP not `0018:07F4h`, 20h pushed FLAGS not 0003h |
| `call gate to ring 0` | (e) Ring 3 pushes E1A5h, then E25Ah, and makes a far CALL through `4Bh` to the DPL-0 non-conforming code `08h`. The handler records its CS, SS and SP and the six words on its stack, and returns with RETF 4. Ring 3 records CS, SS and SP, then leaves through INT 41h. | 1 not reached, 2 CS not `08h`, 4 SS:SP not `0018:07F4h` (SS0:SP0 less six words), 8 pushed CS:IP not the instruction after the CALL, 10h the words above it not E25Ah, E1A5h (the copy keeps their order), 20h pushed SS:SP not `0043:1FFCh` (the ring-3 stack with the two words), 40h after RETF 4 CS, SS or SP not `33h`, `43h`, `2000h` (the words not released) |

Each line goes to COM1 and to the text screen. A failed line prints `FAIL`
and a four-digit hexadecimal mask of the failed checks. Any exception or
interrupt that no case expects ends the cases: the program prints
`unexpected` with the vector and the first three words of its frame,
counts a failure and goes on to the report. The program then prints the
failure count and halts in protected mode with interrupts disabled:

```text
at-privilege-test
iret to ring 3 ok
int to ring 0 ok
iret nulls es ok
cli gp at ring 3 ok
call gate to ring 0 ok
failures 0
```

## Results

**Open profile — MEASURED (2026-11-18, x86jit `a11353e6`).** The golden
runs the at-games-open BIOS (unemu-at-bios, as the protected-mode and
LOADALL tests do) on the same board, memory map (640 KiB + 1 MiB
extended), RTC seed and display switch, without the game cards. Every
line passes and the text screen shows the same lines as COM1. The run
halts after 6,391,138 retired instructions: one POST, one floppy boot and
the program. Image SHA-256
`ac7ba0ab687b574c0908e88bb9708183c4851f01f1a695ac56e2438fd1f79f04`.

**Previous CPU pin — MEASURED.** At x86jit `b4e64dff`, which ran
protected-mode code at CPL 1–3 but refused call gates, the image stops at
the CALL of case (e) with `cpu: unsupported cpu system operation
LegacyGateOrTaskTransfer at 0033:00f1`. The golden fails there, as it
should. Cases (a)–(d) without (e) passed at that pin in 6,390,307 retired
instructions.

**Older CPU pin — MEASURED with the image before case (e).** At x86jit
`cf32ec2e`, which ran protected-mode code only at CPL 0, the program stopped
at the IRET of case (a) with `cpu: unsupported cpu system operation
LegacyPrivilegeTransition at 0008:00a2`.
