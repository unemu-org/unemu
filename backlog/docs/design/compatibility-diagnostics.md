# Per-board compatibility diagnostics

Spike `upc-fro.2`, 2026-11-11. This records the diagnostic-ROM investigation;
it does not qualify a ROM execution or add a fixture. Sources and measurements
below are those of the spike, accessed 2026-11-11.

Evidence labels:

- **READ:** seen in a named source, including emulator source; not a runtime
  compatibility result.
- **MEASURED:** hashed or probed in the spike; not remeasured for this document.
- **ASSUMED:** inferred or expected behavior requiring a later golden run.

## 1. Purpose and firmware policy

Measure compatibility per board with period BIOS-replacement diagnostic ROMs,
then diagnostic disks once the board's BIOS can boot them. These complement
device isolation tests and open-BIOS boot goldens: a diagnostic checks the
board's observable hardware combination, with its own coverage limits and
quirks. A pass is not proof of complete hardware fidelity.

Proprietary firmware is **user-supplied by hash only**. Commit identities,
chip metadata and our verdicts, never ROM bytes or captured screen text.
Tests run only when the matching user-supplied parts are present. Follow the
separate lane established by `upc-x83.11` and
[`vendor/user-roms/README.md`](../../../vendor/user-roms/README.md): absent
dumps skip even when `UNEMUPC98_REQUIRE_FIXTURES=1`; only the explicit
`UNEMUPC98_REQUIRE_USER_ROMS=1` lane requires them. This is not an open-fixture
download or a default firmware change. A catalogue identity alone does not
qualify execution.

**Verdict:** Supersoft/Landmark AT 1.2.0 is the only ROM identified by this
spike as usable now for an `ibm-5170` compatibility test. Its real-mode prefix
is the first target; a full run needs protected-mode work beyond the initial
CPU seam. Ruud's ROM is an XT candidate for a future `ibm-5160`, not an AT
substitute.

## 2. Supersoft/Landmark AT 1.2.0

### Versions and identities

READ: the AT variant is version 1.2.0 for the IBM 5170, 5162 and AT clones.
Its short banner identifiers are `ROM POST DIAGNOSTICS VERSION 1.2.0` and
`COPYRIGHT 1990 SUPERSOFT INC.` The reference is Landmark Research
International's *ROM POST User's Manual* (1991), MAN-RP-110191-JDR v1.2.
It covers PC/XT and AT even/odd ROM pairs, plus a larger AT part for
386/486 systems.

READ: MAME `src/mame/pc/at.cpp`, lines 851–857 at last file commit
`cabb909b38` (2026-11-03), identifies BIOS selector `landmark` (index 2),
labelled `Landmark/Supersoft diagnostic ROMs`, with the note to use Hercules
or MDA. Each AT chip image is `8000h` bytes:

| Lane / socket | MAME filename | CRC | SHA-1 |
|---|---|---|---|
| Even / U27 | `5170_even_u27_ 27256.bin` | `6790392d` | `c4a5310341f346dd072d096152060ef5e4430a7f` |
| Odd / U47 | `5170_odd_u47_ 27256.bin` | `4c0f3db4` | `97a0cf589b93551ed1d03bd622cbc8fd5634512f` |

MAME supplies no SHA-256 here. Measure part and assembled SHA-256 identities
from a user dump whose SHA-1 matches before pinning a catalogue entry. The
commented-out `atdiag` set has unknown origin and is excluded.

READ: MAME `src/mame/pc/ibmpc.cpp` separately lists the XT `pclandmark`:
`5150_or_5160_2764_8kb.bin`, 8 KiB, CRC `4e89a4d8`, SHA-1
`39a28fb2fe9f1aeea24ed2c0255cebca76e37ed7`. It is not the AT pair.

### Chip layout and display risk

READ: two 27256 chips, U27 even and U47 odd, interleave into 64 KiB at
F0000h; this fits `firmware.bios.parts`. The 5162 uses U34/U35 instead.
ASSUMED: probably 32 KiB of code mirrored; do not treat that inference as a
measured layout.

READ: the manual accepts MDA, CGA, EGA or self-initializing VGA and recommends
monochrome. The minuszerodegrees 5170 report instead found two genuine IBM
MDA cards produced neither beeps nor display, a clone MDA and CGA cards
worked, and none of the tested VGA cards worked. MAME recommends Hercules
or MDA. **ASSUMED risk:** our MDA behavior is unknown; the first golden must
establish whether the diagnostic initializes it.

### Reporting, quirks and prerequisites

READ: manual pp. 13–14 and 40–41 describe a full-screen table, reverse-video
active test, looping tests and failure beep codes. The manual describes no
POST-port output; port 80h alone is not a verdict channel.

READ quirks and needs from the manual and minuszerodegrees notes:

- Power-on can spend over 20 seconds signalling monitor initialization
  failure before a display appears.
- The protected-mode test takes about 25 seconds on a 6 MHz 5170 and can
  temporarily report a failure before its final pass indication.
- Floppy read can take about 1.5 minutes to fail without a disk.
- The A20 gate's operation is not tested.
- No battery is needed. With no keyboard, the keyboard tests pass.
- No loopback plugs are needed: the AT suite has no serial/parallel tests.
- Floppy read needs a formatted disk in A:. A speaker is recommended.
- Fit 640 KiB conventional RAM for the baseline. With only 512 KiB, errors
  after 80000h are expected; the memory tests ignore CMOS memory sizes.

These are reported hardware timings, not measured emulator budgets. Golden
limits must use virtual time, never a host-clock timeout as the verdict.

### Test coverage and expected behavior today

READ: manual p. 16 gives the order; pp. 17–36 describe the tests. Hardware
and mode requirements below come from that reading unless marked ASSUMED.
**Every entry in “Expected today” is ASSUMED, not a measured pass.** “Today”
means the real-mode `ibm-5170` at x86jit pin
`8979fc95cd81d73763d50d1d627b40ded3f8f284`. Rows after test 13 are individual
expectations if reached, not a claim that the current run reaches them.

| # | Test (descriptive identifier) | Hardware | PM / >1 MiB | Expected today (ASSUMED) |
|---|---|---|---|---|
| 1 | U74 CPU registers and logic | Registers, ALU, MUL/DIV, strings | No; possible SMSW CPU identification is ASSUMED, unknown | PASS, or stop if SMSW is used |
| 2–3 | U27/U47 ROM checksum | Even/odd lanes | No | PASS |
| 4–6 | U103 8254 channels 0/1/2 | Counter bits, count rate | No | PASS; rate comparison against CPU timing is a risk |
| 7–8 | U111/U122 8237A pair | 00h–0Fh, C0h–DFh | No | PASS |
| 9 | U124 74LS612 page registers | Patterns; 80h–8Fh range ASSUMED | No | Risk: port 80h is our POST capture port and reads FFh (READ: `crates/devices/src/post_port.rs`) |
| 10 | U126 parity detection | Port 61h bits 7/6 ASSUMED | No | PASS |
| 11 | Critical memory | RAM 0–16 KiB | No | PASS |
| 12 | Memory refresh | About 10-second wait | No | PASS |
| 13 | Protected-mode CPU | Descriptor caches, GDTR/IDTR/LDTR/TR, MSW, PM entry, exceptions, task switching | PM | Blocked |
| 14–15 | 8259 controllers 1/2 | 20h/21h, A0h/A1h | No | PASS |
| 16 | Hot interrupt | All interrupts masked for 1 second | No | PASS |
| 17 | Interrupt level 0 | IRQ0 rate | No | PASS; rate unverified |
| 18 | RTC interrupt | IRQ8 periodic versus PIT channel 0 | No | Likely PASS |
| 19 | NMI | None expected | No | PASS |
| 20 | U76 80287 | Detection and registers | No | Wrong presence result: core executes x87 ungated (READ); needs empty-socket FPU model, `xj-562.11 (x86jit)` and `upc-18x.9` |
| 21–22 | 8042 and keyboard scan lines | 8042 reset, IRQ1 | No | PASS |
| 23 | CMOS RAM | Pattern write/restore | No | PASS with volatile NVRAM |
| 24–25 | Floppy adapter and read | µPD765, DMA 2, IRQ6, A: C0 H0 S1 | No | PASS with an image in A: |
| 26–28 | MDA / CGA / EGA-VGA memory | B0000h, B8000h, A0000h | No | MDA PASS; CGA PASS if VGA-text card fitted; EGA/VGA absent |
| 29–30 | System memory and slow refresh | 0–640 KiB, then from 1 MiB | >1 MiB needs PM | Conventional part only after PM blocker is resolved |
| 31 | System BIOS | U17/U37 checksum | No | FAIL expected; manual says to ignore it |
| 32 | CMOS configuration | Configuration display | No | Not applicable |

ASSUMED: the current machine should report at most tests 1–12, then stop at
the first LMSW/SMSW/LGDT, possibly earlier if CPU identification uses SMSW.
READ: x86jit `8979fc9`, `x86jit-core/src/lift/mod.rs`, has no lifter arms for
these instructions. Test 13 needs `xj-srh.15 (x86jit)` and more: the manual
also requires exceptions and task switching, outside that bead's scope.

## 3. Golden capture rule

Run headlessly until exit or a deterministic virtual-time budget. Record:

- exit reason and opcode, and retired-instruction count;
- SHA-256 of the MDA text plane;
- our own verdict table, using our identifiers and only the status cell at
  each row's fixed position to classify pass, fail or absent;
- a reason for every expected non-PASS row.

Pin ROM hashes, CMOS start, volatile NVRAM and the exact x86jit revision.
The ROM's status tokens may be read to classify a result, but **never copy
its screen text into a golden, log or document**. Keep hashes and our own
verdict table, not screen dumps or transcriptions. The short identity names
above identify the firmware; they do not authorize reproducing its display.

## 4. Ruud's Diagnostic ROM

### Availability and measured XT v5.7 identities

READ: the minuszerodegrees AT page (Last-Modified 2026-11-02) says there is
presently no AT version and reports an AT version in development in October
2026. Do not qualify AT support from that announcement. The XT build is a
future `ibm-5160` candidate.

READ: version 5.7 is dated 2026-10-24 and declares an 8086 CPU in source.
MEASURED: the distribution contains an 8 KiB ROM at FE000h packaged as a
32 KiB binary of four identical copies. The v5.7 ZIP is openly fetchable
from minuszerodegrees; the author's old link is a stub pointing there.
Versions 5.3–5.6 ZIPs were still served; no VCS was identified in the spike.

| Artifact | SHA-256 (MEASURED in spike) |
|---|---|
| v5.7 ZIP | `476efc3281314b654862090fe7e224c917a23000853a7f9c32433673ca469ce7` |
| 32 KiB binary | `110e8e197f77246e0f45963a746a9e2cebf50c53089b2988e82caeccbea63360` |
| One 8 KiB copy | `7a25ef79dfd0a71e65e7e78964032324cbb040fab2c21a84e027bad0af893ac4` |
| Assembly source | `f3f9ffafd37a9d85494403f657eb8113718c95fe0533dbc58a763f72ab1049ce` |

### Licence and reproducibility

READ: no licence file was found. The ROM banner's licence statement is:

> Ruud's Diagnostic ROM for PC/XT - No copyright, all open source - Author: Ruud Baltissen

READ: disclaimers accompany it. In VCFed thread 68525 the author states
“My work will be open source anyway.” (2018-12-21) and “My source will be
freeware so you can edit it in the way it suits you best.” (2018-12-25).
A three-line macro is marked “Copied from the source code of XTRAMTEST”
(GPL-2.0). These are informal statements: a maintainer decision is needed
before adding fixture fetching; this document grants no licence clearance.

READ: Ruud described disassembling the Landmark ROMs and writing his own;
minuszerodegrees describes it as based on Landmark. The source does not
mention Landmark. The spike found no evidence of copied code; describing it
as a functional rewrite remains ASSUMED.

READ: building requires NASM ≥ 2.11. The source embeds `__DATE__`; the NASM
source inspected in the spike uses `time()` without `SOURCE_DATE_EPOCH`
support for it. A reproducible rebuild must pin the date as well as the
source and toolchain; the distributed binary hash is not a reproducible-build
claim.

### Coverage and XT requirements

READ from source strings: CPU, ROM checksum, three 8253 channels, 8237A,
hot timer channel 1, RAM parity latches, first 2 KiB, top of RAM, RAM data,
refresh, address and slow-refresh tests, 8259, hot IRQ, IRQ0, hot NMI,
keyboard reset/stuck key, floppy reset/read, ROM checksums F4000h–FC000h,
and SW1/SW2 configuration display. Tests remain in real mode and at or
below 640 KiB.

READ: it needs the XT 8255 at 60h–63h, including PB6/PB7 and parity latches.
Checkpoints go to 378h/278h/3BCh, COM1 at 3F8h, and port 80h. That is not
the current AT board's peripheral combination.

## 5. Other options

READ survey; applicability is a proposed validation role, not a measured run:

| Option | Nature | Validation role |
|---|---|---|
| IBM PC Diagnostics AT 2.00–2.07 / AT Advanced Diagnostics 2.06/2.07 | Proprietary boot floppies; require an IBM-compatible BIOS; serial/parallel wrap plugs | After the IBM BIOS boots (`upc-6le.29`); numbered error codes |
| CheckIt, QAPlus, AMIDiag, PC-Doctor, Landmark Service Diagnostics | Proprietary DOS programs | Smoke tests rather than board tests |
| XTRAMTEST (GPL-2.0) | Open XT ROM | Future `ibm-5160` |
| `test386.asm` (`upc-fro.1`) | Open, 386+ | `generic-386-isa` |

## 6. Follow-up beads

READ: the supervisor filed these follow-ups; this list records the spike's
handoff, not a replacement task board:

| Bead | Scope and dependencies |
|---|---|
| `upc-fro.3` | Landmark AT catalogue entry, user-supplied; establish SHA-256 from matching dumps |
| `upc-fro.4` | Real-mode-prefix golden with the capture rule in §3 |
| `upc-fro.5` | Full-pass golden; PM beyond `xj-srh.15 (x86jit)`, RAM above 1 MiB, empty-socket FPU (`xj-562.11 (x86jit)`, `upc-18x.9`) |
| `upc-fro.6` | Ruud XT v5.7 fixture for `ibm-5160`, P4; licence decision before fetching |

Conditional only: file a 74LS612 page-register read-back bead for port 80h
while retaining POST capture **if the golden demonstrates that failure**.
No unconditional page-register implementation or filed bead is claimed.

## Sources

Source access date in the spike: 2026-11-11. Hashes labelled MEASURED above
come from that spike, not from the websites' descriptions.

- MAME [AT ROM declarations](https://github.com/mamedev/mame/blob/cabb909b38/src/mame/pc/at.cpp#L851-L857)
  (last file commit 2026-11-03) and [PC/XT declarations](https://github.com/mamedev/mame/blob/master/src/mame/pc/ibmpc.cpp).
- Landmark Research International, [*ROM POST User's Manual*, MAN-RP-110191-JDR v1.2](https://retrocmp.de/ibm/5170/SUPERSOFT-LANDMARK-ROM_USERS%20MANUAL_V1-2_OCR.pdf),
  especially pp. 13–14, 16–36 and 40–41.
- minuszerodegrees, [Landmark on IBM 5170](https://www.minuszerodegrees.net/supersoft_landmark/5170/Supersoft%20Landmark%20ROM%20-%20IBM%205170.htm)
  (Last-Modified 2025-02-01), [IBM 5162 layout](https://www.minuszerodegrees.net/supersoft_landmark/5162/Supersoft%20Landmark%20ROM%20-%20IBM%205162.htm),
  and [AT clones](https://www.minuszerodegrees.net/supersoft_landmark/Clones/Supersoft%20Landmark%20ROM%20-%20AT%20clones.htm)
  (Last-Modified 2025-09-12).
- minuszerodegrees, [Ruud AT availability](https://www.minuszerodegrees.net/ruuds_diagnostic_rom/5170/Ruuds%20Diagnostic%20ROM%20-%20AT.htm)
  (Last-Modified 2026-11-02), [IBM 5160](https://www.minuszerodegrees.net/ruuds_diagnostic_rom/5160/64-256KB.htm)
  (Last-Modified 2026-11-10), and [v5.7 distribution and source](https://minuszerodegrees.net/ruuds_diagnostic_rom/bin/ruuds_diagnostic_rom_v5.7_32kb.zip).
- VCFed [thread 68525](https://forum.vcfed.org/index.php?threads/68525/)
  (including author statements on 2018-12-21 and 2018-12-25) and
  [thread 69279](https://forum.vcfed.org/index.php?threads/69279/).
- [XTRAMTEST source](https://github.com/ki3v/xtramtest).
- NASM [`asm/nasm.c`](https://github.com/netwide-assembler/nasm/blob/master/asm/nasm.c)
  (`__DATE__` handling inspected in the spike).
- WinWorld [IBM PC Diagnostics AT 2.x catalogue](https://winworldpc.com/product/ibm-pc-diagnostics/at-2x).
- x86jit [`x86jit-core/src/lift/mod.rs` at 8979fc9](https://github.com/unemu-org/x86jit/blob/8979fc95cd81d73763d50d1d627b40ded3f8f284/x86jit-core/src/lift/mod.rs).
