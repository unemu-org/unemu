# Open AT protected-mode return test

`vendor/at-pm-test/at-pm-test.asm` is the open replacement, in the public
test suite, for the Windows 3.x Standard-mode reference test on the 286 AT.
That guest is proprietary and runs only in private goldens. On an 80286, its
DOS extender and the DOS extended-memory manager (HIMEM.SYS) depend on a small
set of machine paths, and this program exercises the same paths:

- entering protected mode;
- coming back to real mode. A 286 cannot clear PE, so the only ways back are
  a processor reset requested from the keyboard controller and a deliberate
  triple fault. The BIOS resumes both through the CMOS shutdown byte and the
  doubleword at `0040:0067h`;
- switching the A20 gate through the 8042 output port;
- the BIOS extended-memory services, which an extended-memory manager may
  use instead of its own protected-mode code: INT 15h AH=88h (size) and
  AH=87h (block move).

Build and fixture details are in
[`vendor/at-pm-test/README.md`](../../../vendor/at-pm-test/README.md), the
goldens in `crates/cli/tests/at_pm_test_golden.rs`, and the commands in
[`commands.md`](../commands.md#ci).

Evidence labels: **READ** (seen in the named source), **MEASURED** (run in
this repository at the CPU pin in `Cargo.lock`), **ASSUMED** (expected, not
yet run).

## References

- **IBM Personal Computer AT Technical Reference**, System Board, Keyboard
  Controller: output port bit 0 drives the processor reset and bit 1 gates
  A20. Commands D0h (read output port), D1h (write output port; the next byte
  written to port 60h is the new value) and F0h–FFh (pulse output-port bits
  0–3 low; FEh pulses only bit 0, the reset). Status register bits 0 (output
  buffer full), 1 (input buffer full) and 2 (system flag).
- **IBM Personal Computer AT Technical Reference**, System Board, Real-Time
  Clock and CMOS RAM: address 0Fh is the shutdown status byte.
- **IBM Personal Computer AT Technical Reference**, BIOS listing, POST reset
  path (TEST1; the 06/10/85 listing was read, **READ**). After a reset with
  the 8042 system flag set, POST reads CMOS 0Fh and clears it before it
  dispatches. Code 05h initializes both 8259s with every line masked, reads
  port 60h to flush the keyboard, sends an EOI and jumps through
  `0040:0067h`. Code 0Ah jumps through `0040:0067h` without initializing the
  interrupt controllers. Both enter with SS:SP = `0000:0400h` and
  DS = `0040h`; neither touches the 8042 output port. With the system flag
  clear, POST ignores the byte and runs the power-on path.
- **IBM Personal Computer AT Technical Reference**, BIOS listing, GATE_A20
  (BIOS1): command D1h, then data DFh to enable A20 or DDh to disable it,
  waiting for an empty input buffer between bytes. The program uses the same
  values.
- **IBM Personal Computer AT Technical Reference**, BIOS listing, INT 15h
  AH=88h (EXT_MEMORY, BIOS1; the 06/10/85 listing was read, **READ**):
  AX = CMOS 31h:30h, the extended memory POST found, in KiB. It returns
  through IRET, so CF and the other flags are the caller's; the listing
  never clears CF.
- **IBM Personal Computer AT Technical Reference**, BIOS listing, INT 15h
  AH=87h (BLOCKMOVE, BIOS1, **READ**). CX words go from the source to the
  target descriptor of a six-entry GDT at ES:SI: a dummy, the GDT itself,
  source, target, BIOS code and stack. The caller zeroes entries 0, 1, 4
  and 5 and the BIOS fills them; source and target carry a limit of at
  least 2·CX−1, a 24-bit base and access byte 93h. The BIOS saves the
  caller's registers and SS:SP (in `0040:0067h`), enables A20, sets CMOS
  0Fh = 09h, writes 00h to the manufacturing port 80h, loads its own IDT
  (all 32 exception vectors go to one handler) and the caller's GDT, enters
  protected mode and runs REP MOVSW. Then, or from the exception handler
  after writing 02h to port 80h, it resets the processor through the 8042.
  POST sees code 09h (TEST1), clears CMOS 0Fh without initializing the
  interrupt controllers and jumps to SHUT9. SHUT9 restores SS:SP, turns
  A20 off (DDh), selects CMOS register D with NMI enabled, reads the status
  back from port 80h into the saved AH, executes CMP AH,AL with AH = 0,
  restores the registers, then STI and RETF 2. So every register except AH
  is the caller's, CF=0 and ZF=1 only for status 00h, and IF is 1 whatever
  it was before the call: RETF 2 discards the caller's FLAGS. Status 01h is
  a parity error, 02h any exception during the move, 03h an A20 gate
  failure.
- **Intel UPI-41A/41AH/42/42AH User's Manual**, Data Bus Buffer and General
  Handshaking Protocol: the host writes only with IBF clear and reads only
  with OBF set.
- **Intel 80286 Programmer's Reference Manual**, System Control and
  Initialization: LMSW sets PE; an intersegment jump must follow. Interrupts
  and Exceptions: a fault during delivery of a fault raises the double fault
  (interrupt 8); a fault during delivery of the double fault shuts the
  processor down. The AT board turns the shutdown cycle into a processor
  reset (IBM Personal Computer AT Technical Reference, System Board, Logic
  Diagrams).

## Program and output

The boot sector loads the program to `0800:0000h`. The program initializes
COM1 (`3F8h`, divisor 1, 8N1), enables A20, runs the two counted INT 15h
checks once, runs 16 rounds and ends with the informational faulting block
move. State that
must survive a reset lives in the high memory area at physical `10FE00h`
(`FFFF:FE10h`): signature, round, pending stage, a check value, the
protected-mode results, the first unexpected vector, the 8259 masks and the
failure count. A BIOS that cold-boots instead of resuming (the Kiselev
Xi8088 build did) and tests only conventional memory leaves that state
intact, so the program can still report the reboot. Each line goes to the BIOS teletype (INT 10h AH=0Eh) and the same
bytes to COM1:

```text
at-pm-test rounds 16
int15 88h ok
int15 87h ok
r01 pm ok
r01 kbc ok
r01 tf ok
r01 a20 ok
...
r16 a20 ok
int15 87h fault ah 02 info 0000
failures 0
```

A failed stage prints `FAIL` and a four-digit hexadecimal mask of the failed
checks below; on a resume line, bit 20h adds `vector` and the unexpected
vector number in hex. `FAIL reboot` means the only failure was that the BIOS booted
the floppy again instead of jumping through `0040:0067h`. The program then
halts with interrupts disabled.

Two failures stop the run early, so that the final count is not lost. If
the program boots without its state while CMOS 0Fh still holds 05h or 0Ah,
the BIOS ignored the shutdown byte and also cleared memory above 1 MiB: it
clears the byte, prints `resume FAIL state lost` and `failures 1` (the
earlier count was in the lost state) and halts. The same happens if a
resume through `0040:0067h` finds no state. If A20 cannot be turned back on
in the `a20` stage, the state block is out of reach: the program prints
that line and the count it kept before the toggles, then halts. The same
holds after an INT 15h AH=87h call (see below).

NMI stays masked (port 70h bit 7) from writing the shutdown byte until the
resume; the program then selects register D with NMI masked (8Dh), and 0Dh
after the resume. Port 70h and 71h accesses are separated by a short jump
for I/O recovery.

| Stage | What it does | Mask bits on failure |
|---|---|---|
| `pm` | GDT and IDT in RAM, LGDT/LIDT, LMSW, far jump to the code selector, SS/DS/ES loads; writes 8 words through a data descriptor at `120000h` and reads them back; checks that the A20-masked alias at `020000h` still holds its real-mode pattern; INT 40h through an interrupt gate; a word read past a 256-byte limit raises #GP, whose own handler checks error code 0 and resumes at a recovery label. Results are stored in the state block and printed after the next resume. | 1 CS is not the code selector, 2 the high window does not read back, 4 the low alias changed (A20 not effective), 8 INT 40h did not run, 10h #GP not handled exactly once with error code 0 |
| `kbc` | Arms the resume (state, check value, `0040:0067h`, 8259 masks), writes CMOS 0Fh = **0Ah**, runs `pm`, then sends FEh to port 64h from protected mode. | see resume bits |
| `tf` | Arms the resume with CMOS 0Fh = **05h**, enters protected mode, loads an IDT limit of 0 and reads past the 256-byte limit. Neither the #GP nor the double fault can be delivered, so the processor shuts down. If the #GP handler runs anyway (the IDT limit was not applied), it marks the stage, which then fails. | see resume bits |
| `a20` | D1h/DDh, D0h readback (bit 1 clear), wrap test; D1h/DFh, D0h readback (bit 1 set), wrap test. The wrap test saves both words, writes different values to `0000:0000h` and `FFFF:0010h`, compares them and restores them, with interrupts disabled. | 1 readback after DDh, 2 no wrap with A20 off, 4 readback after DFh, 8 wrap with A20 on |

Resume bits, checked after either reset: 1 PE still set (SMSW), 2 the BIOS
left CMOS 0Fh set, 4 the 8042 A20 bit was lost across the reset, 8 the
conventional-memory copy of the round or check value differs, 10h the
state check value differs, 20h an unexpected protected-mode exception, 40h
the triple fault was not reached, did not shut down or reached the #GP
handler, 80h unknown pending stage, 100h the 8259 masks do not fit the
shutdown code, 8000h the BIOS rebooted. Bits 2, 4, 8 and 100h apply only to
a resume through `0040:0067h`.

The two codes match the two ways back: 0Ah is the fast return without
interrupt-controller initialization; 05h also resets the 8259s, flushes the
keyboard and sends an EOI. Before restoring anything, the resume reads the
mask registers at 21h and A1h. After 05h (the `tf` stage) they must be
FFh/FFh, the IBM behaviour (**READ**, BIOS listing, TEST1: every line masked
after initialization). After 0Ah (the `kbc` stage) they must equal the masks
saved before the reset. The program then restores the saved masks after
either code. The open AT BIOS's shutdown dispatch matches this IBM
behaviour (Results below).

Unexpected protected-mode vectors (0–40h, other than 13 and 40h) record
their number and pulse the 8042 reset, so a CPU fault still ends in a
reported resume instead of a hang.

### INT 15h AH=88h and AH=87h

Before the rounds, the program calls the two BIOS services in real mode;
the BIOS does all protected-mode work. Their lines have no round number and
count towards the failure total like the others. After the rounds, one more
AH=87h call, outside the BIOS's documented contract, prints an
informational line that is never counted.

The block-move buffers lie past the loaded program in its segment: a source
at `0800:2000h` (physical `0A000h`), a second buffer at `0800:2400h`, both
128 words. The target above 1 MiB is physical `10C000h`, inside the high
memory area, so with A20 on the program reads it back directly at
`FFFF:C010h`. Its A20-masked alias, `0800:4000h`, holds a guard pattern.
The source holds a numbered pattern (each word 0101h more than the one
before); the second buffer and the high block start zeroed.

Before each AH=87h call the program marks the call as pending in the state
block and keeps the failure count it finds there, writes 00h to CMOS 0Fh
(so a code left from an earlier shutdown cannot pass for the BIOS's), and
turns A20 off (D1h/DDh) and checks with the wrap test that it is off. The
BIOS must then turn A20 on itself for the move, so the guard is a real
check: a BIOS that moves without A20 writes the block to its alias. The
call is made with IF set and with CF and ZF opposite to what the expected
status returns, so a BIOS that leaves the flags alone fails. AL, BX, DX,
DI, BP and DS hold marker values and CX the word count; ES:SI points to the
GDT, rebuilt before each call with entries 0, 1, 4 and 5 zero. After the
call the program records every register through CS before it touches the
stack, clears DF, reads the 8042 output port (D0h), turns A20 back on
(D1h/DFh) and checks with the wrap test that it is on before it touches the
state block again, and reads CMOS 0Fh. If A20 stays off, the state block is
out of reach: the program prints the line, without the state block, and
the count it kept plus one, then halts, as the `a20` stage does. A BIOS
that boots the floppy instead of returning is caught by the pending
stage: during the round trip the program prints `int15 87h FAIL reboot`
and goes on with the rounds; during the faulting move it prints
`int15 87h fault info reboot` and the count.

The IBM BIOS turns A20 off on the way out; a BIOS that puts back the A20
state it found leaves it off as well, since the call started with A20 off.
Both pass. IF is checked because the IBM listing returns with STI and
RETF 2: a call made with IF set must come back with IF set. A BIOS that
patches CF in the caller's FLAGS and returns with IRET also passes that
check, but not ZF: the IBM header documents ZF=1 for success and ZF=0 for
an error "for PC-AT compatibility", so ZF counts.

| Line | What it does | Mask bits |
|---|---|---|
| `int15 88h` | Reads CMOS 30h/31h, calls AH=88h with CF clear (the IBM BIOS returns the caller's CF). | 1 CF set, 2 AX is not CMOS 31h:30h |
| `int15 87h` | Moves the 128 words from the source to `10C000h`, compares `FFFF:C010h` with the source, moves them back to the second buffer, compares it with the source and checks the guard. | per call (ORed): 1 AH not 00h, 2 CF set, 4 ZF clear, 8 IF clear, 10h AL/BX/CX/DX/SI/DI/BP changed, 20h DS or ES changed, 40h SS:SP changed, 80h A20 on after the call (output-port bit 1 set), 100h CMOS 0Fh not 00h, 1000h A20 would not turn off before the call, 2000h A20 would not turn on again (the run stops); then 200h the high block lacks the pattern, 400h the second buffer differs, 800h the guard changed (the BIOS moved with A20 off) |
| `int15 87h fault` (after the rounds, informational) | One word (CX=1) from a source descriptor with limit 0: the first word read raises #GP(0) inside an IBM-style protected-mode move. Prints `ah` and the status in hex, then `info` and the bits that differ from the IBM result. Never counted. | the per-call bits of `int15 87h`, against status 02h: 1 AH not 02h, 2 CF clear, 4 ZF set |

The faulting move is informational because it breaks the entry condition
the IBM listing documents: source and target limits of at least 2·CX−1. A
limit of 0 is outside that contract, so the result belongs to the
implementation. The 06/10/85 listing gives 02h, since every exception
vector of its block-move IDT goes to one handler that reports 02h
(**READ**, BLOCKMOVE). Another BIOS may legitimately reject or clamp the
request, build its own descriptors from the caller's bases and succeed, or
copy without protected mode. A BIOS that copies with LOADALL on a 286 even
takes the #GP in real mode, where it can hang. The line still pins the
IBM result to the byte in the user-ROM golden, and it comes after the
rounds, so a hang there loses only that line and the count.

Port 80h must read back the last byte written for this service: on the AT
it is one of the 74LS612 page-register latches, and SHUT9 reads the status
from it after the processor reset. The emulator's POST checkpoint port at
80h, which owns the port on the AT board in place of that page latch,
therefore latches its last write (`crates/devices/src/post_port.rs`). A
default-suite test writes 80h, resets the processor through the 8042 and
reads the byte back; port 80h reads FFh after power-on, the model's choice
(the 74LS612 power-on contents are unspecified). Before the latch, the IBM
block move read FFh and returned AH=FFh with CF set for every move.

## Results

**Open profile — MEASURED (since 2026-11-15, unemu-at-bios).** The golden
runs the at-games-open BIOS, unemu-at-bios `v1.0.2-unemu.2` (the unemu fork of
Kiselev 8088_bios, `MACHINE_UNEMU_AT`; `vendor/unemu-at-bios/README.md`), on
the same board, memory map (640 KiB + 1 MiB extended) and RTC seed as
`machines/at-games-open.toml`, without its game cards. Those are the journaled
IDE disk, the Sound Blaster, EMS and the mouse; this test does not exercise
them, and CI cannot provide the local disk. The display switch is `mono`: the
BIOS reads it with the IBM polarity and prints on the preset's MDA, where the
golden reads the screen. Every line passes, as with the IBM BIOS:
`int15 88h ok`, `int15 87h ok`, all 16 `pm`, `kbc`, `tf` and `a20` lines
`ok`, `int15 87h fault ah 02 info 0000` and `failures 0`. The run halts after
6,589,934 retired instructions: one POST (with the 8042 CPU reset of its
extended-memory sizing, shutdown code 09h) and one floppy boot, then every
reset resumes through `0040:0067h` without POST. **READ** (the fork's
`README.unemu.md`): its reset entry checks the 8042 system flag, reads and
clears CMOS 0Fh and dispatches 05h, 09h and 0Ah as the IBM listing does;
AH=88h returns CMOS 31h:30h, which its POST measures; AH=87h ends with the
status in AH, CMP AH,AL, STI and RETF 2. Its status survives the reset in the
saved AH on the caller's stack rather than in port 80h.

Before that, the profile ran the Kiselev 8088_bios 1.0.2 Xi8088 build with
the AT reset patch: all 16 `pm` and `a20` lines passed, every `kbc` and `tf`
line was `FAIL reboot` (it cold-booted on every reset, never reading CMOS
0Fh), the INT 15h lines were `int15 88h FAIL 0003`, `int15 87h FAIL 0607` and
`int15 87h fault ah 86 info 0005` (AH=86h with CF set for both functions),
`failures 34`, after 211,167,574 retired instructions (33 POSTs and floppy
boots).

**IBM 5170 BIOS (06/10/85) — MEASURED before the INT 15h lines.** With the
user-supplied firmware the user-ROM golden passed: every line `ok`,
including the `TF_HANDLED` and 8259-mask checks, `failures 0`, halted after
31,935,006 retired instructions, both reset paths resuming through
0040:0067h without POST. **MEASURED with the INT 15h stage:** `int15 88h
ok`, `int15 87h ok` (with the port 80h latch above), `int15 87h fault ah
02 info 0000` and `failures 0`, halted after 31,955,711 retired
instructions, pinned in the golden. Its
machine file declares both BIOS disk services and pins the
catalogue SHA-256s. It also needs an empty AT fixed-disk controller (`ide`,
`controller_model = "at-adapter"`, no drives). **MEASURED (an earlier run
without it):** POST counted memory, then showed 601 (diskette) and 162
(system options) and waited for F1, so the floppy never booted. **READ**
(BIOS listing, TEST2 and TEST4): on the 5170 the diskette controller sits on
the Fixed Disk and Diskette Drive Adapter. The diskette attachment test also
probes that adapter's fixed-disk controller: status at 1F7h must not stay
busy or show bits 2–3, and the cylinder register at 1F4h must read back
written values. Floating ports fail the probe with 601. The failure path
enters the drive-B configuration check without loading the drive state it
compares with, so that check (TEST4) can set the CMOS configuration-error
bit, which gives 162 (the stale register value is inferred, not read). The
derived diskette (10h) and equipment (14h) bytes are not the cause: they do
not depend on a catalogue hash match. A default-suite test checks both CMOS
bytes and the adapter probe with and without the card.

## Manual acceptance: Windows 3.x Standard mode (private)

For a local check against the real guest. Use only your own installation
media; nothing from this run is committed, except a verdict and hashes in
the private goldens.

1. Set up the at-games profile (IBM 5170 and VGA BIOS) with your own C:
   image, as in `machines/README.md`, Playing DOS games.
2. Install DOS on C: with its extended-memory manager loaded from
   CONFIG.SYS, then install Windows 3.x from your own media.
3. Boot C: and check that the extended-memory manager reports that it
   controls A20 and finds 1 MiB of extended memory.
4. Start Windows in Standard mode (the `/s` switch) and confirm that the
   About dialog of Program Manager reports Standard Mode.
5. Open a DOS prompt from Windows and exit it. This switches between
   protected and real mode many times.
6. Run Minesweeper and Solitaire for a few moves each.
7. Exit Windows to the DOS prompt, then restart with Ctrl+Alt+Del and check
   that POST and the boot to DOS complete.

Pass: no hang, no reboot loop and no unexpected exception in any step. A
failure in steps 4–5 with this program passing points away from the reset
and A20 paths covered here.
