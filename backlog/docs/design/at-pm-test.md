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
- switching the A20 gate through the 8042 output port.

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
COM1 (`3F8h`, divisor 1, 8N1), enables A20 and starts 16 rounds. State that
must survive a reset lives in the high memory area at physical `10FE00h`
(`FFFF:FE10h`): signature, round, pending stage, a check value, the
protected-mode results, the first unexpected vector, the 8259 masks and the
failure count. The open BIOS cold-boots instead of resuming and tests only
conventional memory (**READ**, its source), so the state survives that
reboot. Each line goes to the BIOS teletype (INT 10h AH=0Eh) and the same
bytes to COM1:

```text
at-pm-test rounds 16
r01 pm ok
r01 kbc ok
r01 tf ok
r01 a20 ok
...
r16 a20 ok
failures 0
```

A failed stage prints `FAIL` and a four-digit hexadecimal mask of the failed
checks below; with resume bit 20h it adds `vector` and the unexpected vector
number in hex. `FAIL reboot` means the only failure was that the BIOS booted
the floppy again instead of jumping through `0040:0067h`. The program then
halts with interrupts disabled.

Two failures stop the run early, so that the final count is not lost. If
the program boots without its state while CMOS 0Fh still holds 05h or 0Ah,
the BIOS ignored the shutdown byte and also cleared memory above 1 MiB: it
clears the byte, prints `resume FAIL state lost` and `failures 1` (the
earlier count was in the lost state) and halts. The same happens if a
resume through `0040:0067h` finds no state. If A20 cannot be turned back on
in the `a20` stage, the state block is out of reach: the program prints
that line and the count it kept before the toggles, then halts.

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
either code. The open BIOS patch that adds the shutdown dispatch
(`upc-fro.11`) must match this IBM behaviour.

Unexpected protected-mode vectors (0–40h, other than 13 and 40h) record
their number and pulse the 8042 reset, so a CPU fault still ends in a
reported resume instead of a hang.

## Results

**Open profile — MEASURED.** The golden runs the at-games-open BIOS (Kiselev
8088_bios 1.0.2, Xi8088 configuration with the AT reset patch) on the same
board, memory map (640 KiB + 1 MiB extended) and RTC seed as
`machines/at-games-open.toml`, without its game cards. Those are the journaled
IDE disk, the Sound Blaster, EMS and the mouse; this test does not exercise
them, and CI cannot provide the local disk. All 16 `pm` and `a20` lines pass.
Every `kbc` and `tf` line is `FAIL reboot`, and the count is `failures 32`.
The run halts after 211,149,814 retired instructions (33 POSTs and floppy
boots). **READ** (the pinned Kiselev source): this BIOS was written for the
8088. Its reset vector always enters POST, POST never reads CMOS 0Fh, and it
has no A20 or extended-memory code. So no reset resumes through
`0040:0067h`. The resets themselves work: each
line proves that the processor left protected mode, that the triple fault
shut it down, and that memory above 1 MiB survived. The golden pins this
result as expected to fail. To pass, the open BIOS needs the AT shutdown
dispatch at its reset entry: check the 8042 system flag, read and clear
CMOS 0Fh, and handle 05h and 0Ah as the IBM listing does. That would be a
further patch to the Kiselev BIOS, with re-measured open goldens. It is
outside the machine model.

**IBM 5170 BIOS (06/10/85) — MEASURED.** With the user-supplied firmware the
user-ROM golden passes: every line `ok`, including the `TF_HANDLED` and
8259-mask checks, `failures 0`, halted after 31,935,006 retired instructions,
both reset paths resuming through 0040:0067h without POST. Its machine file declares both BIOS disk services and pins the
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
