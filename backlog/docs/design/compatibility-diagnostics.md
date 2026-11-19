# Per-board compatibility diagnostics

Investigation `upc-fro.2`, extended with the user-supplied Landmark AT catalogue
entry and real-mode-prefix golden on 2026-11-11. No ROM fixture is distributed.
Sources were accessed in the investigation on 2026-11-11; runtime observations
below are supervisor measurements, not a full-suite qualification.

Evidence labels:

- **READ:** seen in a named source, including emulator source; not a runtime
  compatibility result.
- **MEASURED:** hashed or probed in the investigation or by the supervisor, as
  identified below; not remeasured by the sandboxed worker.
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
dumps skip even when `UNEMUPC_REQUIRE_FIXTURES=1`; only the explicit
`UNEMUPC_REQUIRE_USER_ROMS=1` lane requires them. This is not an open-fixture
download or a default firmware change. A catalogue identity alone does not
qualify execution.

**Verdict:** Supersoft/Landmark AT 1.2.0 is the only ROM identified by this
spike as usable now for an `ibm-5170` compatibility test. Its real-mode prefix
has a pinned user-supplied golden.
A full run needs protected-mode work beyond the initial CPU seam. Ruud's ROM is an XT candidate for a future `ibm-5160`, not an AT
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

MEASURED by the supervisor: both part SHA-1 values match MAME. Catalogue entry
`landmark-rom-post-at-1.2.0` is user-supplied and Listed on `ibm-5170`:

| Artifact | Size | SHA-256 |
|---|---|---|
| U27 even | 32768 | `3677eb9f1b8c39daef82a99337649f43633a46db8726bc6a3805076ba3b25916` |
| U47 odd | 32768 | `6a74a12bc1cda8c67c0526e061c62b8d7e1b622afdf3e9442950355571580313` |
| Interleaved at F0000h | 65536 | `a8310f83de0f3e6b71e4a3ff82a01ffe3c3b5a957677aed2fad17b96bce2e577` |

It occupies the system BIOS socket (`SystemBios`, `TopOfFirstMib`), requires
an 80286, 8042, RTC/CMOS and cascaded PICs, and provides no boot services or
option-ROM scan window. Its catalogue capabilities declare direct hardware
tests for the floppy and fixed-disk controllers. Validation therefore accepts
an FDC with a drive and an IDE card without claiming INT 13h services. This
permission requires a matching system-ROM identity; ordinary BIOS entries and
unlisted images still need the corresponding service provider.
The EPROM type `27256` labels both parts; original MAME filenames are preserved
in the user-ROM README. The commented-out `atdiag` set has unknown origin and
is excluded.

READ: MAME `src/mame/pc/ibmpc.cpp` separately lists the XT `pclandmark`:
`5150_or_5160_2764_8kb.bin`, 8 KiB, CRC `4e89a4d8`, SHA-1
`39a28fb2fe9f1aeea24ed2c0255cebca76e37ed7`. It is not the AT pair.

### Chip layout and display risk

READ: two 27256 chips, U27 even and U47 odd, interleave into 64 KiB at
F0000h; this fits `firmware.bios.parts`. The 5162 uses U34/U35 instead.
MEASURED by the supervisor: the assembled image has two identical 32 KiB halves.

READ: the manual accepts MDA, CGA, EGA or self-initializing VGA and recommends
monochrome. The minuszerodegrees 5170 report instead found two genuine IBM
MDA cards produced neither beeps nor display, a clone MDA and CGA cards
worked, and none of the tested VGA cards worked. MAME recommends Hercules
or MDA. MEASURED by the supervisor: the diagnostic initializes our MDA and
displays the result table; this display risk did not occur in the real-mode prefix.

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
**Every entry in “Expected today” is ASSUMED, not a measured pass.** The table
was formulated for Application Real16 at `44403dd`; the current `ibm-5170`
selects the I286 machine profile at `7a9ea390b8f588ce4af9845980bbf8534f793f61`.
Rows after test 13 are individual expectations if reached, not a claim that
the current run reaches them. No new user-ROM result is assumed.

| # | Test (descriptive identifier) | Hardware | PM / >1 MiB | Expected today (ASSUMED) |
|---|---|---|---|---|
| 1 | U74 CPU registers and logic | Registers, ALU, MUL/DIV, strings | No; possible SMSW CPU identification is ASSUMED, unknown | PASS, or stop if SMSW is used |
| 2–3 | U27/U47 ROM checksum | Even/odd lanes | No | PASS |
| 4–6 | U103 8254 channels 0/1/2 | Counter bits, count rate | No | PASS; rate comparison against CPU timing is a risk |
| 7–8 | U111/U122 8237A pair | 00h–0Fh, C0h–DFh | No | PASS |
| 9 | U124 74LS612 page registers | Patterns; 80h–8Fh range ASSUMED | No | Risk: port 80h is our POST capture port and read FFh; since 2026-11-15 it reads back its last write, as the 74LS612 does (READ: `crates/devices/src/post_port.rs`) |
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

Historical MEASURED by the supervisor on the Real16 application profile: screen lines 1–12 report
Passed, in manual order: `cpu`, `rom_a`, `rom_b`, `pit_ch0`, `pit_ch1`,
`pit_ch2`, `dma1`, `dma2`, `dma_page`, `kbc_parity`, `mem_16k`, `mem_refresh`.
The page-register read-back risk did not occur; no device workaround is needed.
Line 13 has an empty status when execution stops on unsupported SIDT at
F000:A3CE (opcode prefix `0F 01`). The protected-mode test is not a pass.
A budget of 30,000,000 scheduling steps reaches this stop without a disk.
These observations describe the former Real16 profile; the rows below it
were unexecuted expectations at that revision. The board now runs the I286
machine profile at the current pin:
SIDT, LGDT/LIDT, LMSW and same-CPL 16-bit protected execution are available.
CPL-0 IDT interrupt/trap gates,
exception escalation through #DF/shutdown, IRET, LDT selectors and descriptor
queries are now available; exceptions default to in-guest delivery. Shutdown
requests a CPU-only board reset. Explicit outward faults retain vector,
optional error code and CS:IP. Task gates and privilege transitions remain
unsupported.

**MEASURED by the supervisor at main `9bd422d`, CPU `fe240a5`:** Landmark AT
completes full diagnostic passes on `ibm-5170`. Every test passes except the
floppy read test on the measurement rig, which has a controller but no drive
or medium. This evidence supersedes the earlier unsupported-SIDT stop. The
regression configuration below adds a read-only 1.2 MB medium; its subsequent
measured endpoint and hash are pinned in §3.

**MEASURED by the supervisor after the floppy-controller fix:** the floppy
read test now passes with the medium attached. One complete diagnostic pass
finishes between approximately 137 million and 150 million steps. The ROM's
protected-mode tests return to real mode through an 8042 CPU reset, and the
ROM counts each reset as an error: its error counter rises by two per pass.
The counter and failure title therefore reflect the diagnostic's own reset
handling, not an emulator fault. The minuszerodegrees Supersoft/Landmark IBM
5170 notes describe the same behaviour on real IBM 5170 hardware. The golden
now pins this state without reproducing screen text; its measured stop and
hash values are recorded in §3.

Historical READ: x86jit `8979fc9`, `x86jit-core/src/lift/mod.rs`, has no lifter arms for
SIDT or the protected-mode control instructions. Test 13 needs `xj-srh.15 (x86jit)` and more: the manual
also requires exceptions and task switching, outside that bead's scope.

## 3. Golden capture rule

Run headlessly until exit or a deterministic virtual-time budget. The
full-pass capture records only stop kind/location, retired count and the MDA
text-plane hash. The historical prefix capture additionally records our own
verdict classifications. Historical capture rules were:

- exit reason and opcode, and retired-instruction count;
- SHA-256 of the MDA text plane;
- our own verdict table, using our identifiers and only the status cell at
  each row's fixed position to classify pass, fail or absent;
- a reason for every expected non-PASS row.

The ignored CLI test `user_rom_landmark_at_full_pass` in
`crates/pc-harness/tests/landmark.rs` pins the complete-pass golden in the user-ROM
lane. It extends `preset:ibm-5170` with MDA as the sole display,
`display_switch = "auto"`, a unit-0 1.2 MB floppy drive using
`vendor/freedos/freedos-1200.img` read-only, and an IDE card with
`controller_model = "at-adapter"`. The adapter remains present because the
diagnostic probes the fixed-disk adapter even without a hard disk. Missing
media reports `vendor/freedos/fetch.sh` as the fixture command. The system ROM
keeps `provides = []`: its catalogue permissions cover direct controller tests
without inventing boot services. RTC start is `2000-01-01T00:00:00`, NVRAM is
volatile and derived, and CPU revision is pinned to
`7a9ea390b8f588ce4af9845980bbf8534f793f61`.

This test runs with the named `FULL_PASS_RETIRED_CAP` set to 150,000,000,
using the existing deterministic `RunPolicy::natural` session policy. The cap
is the machine's total virtual-time safety net, measured in retired-instruction
units plus halted PIT progress; the actual retired count is recorded separately.
The cap includes one complete pass with the reset-counted state described above.
The test records only the typed stop summary and CS:IP, actual retired count
and SHA-256 of the visible 80×25 MDA glyph/attribute plane. It neither classifies
nor prints screen cells. **MEASURED by the supervisor on this branch** with
that cap, the full-pass golden pins:

| Field | Value |
|---|---|
| Stop kind | `Ok(RetiredCapReached)` |
| CS:IP | `0004:0015` |
| Retired instructions | `138,208,231` |
| Total virtual time (steps) | `150,000,000` |
| MDA text-plane SHA-256 | `fe45632b8a9c1b762af117b162f56adc523b8aa2e5bda990bad5adb7079e807a` |

Synthetic writer tests cover the display, drive geometry and read-only policy, adapter model,
escaped absolute chip paths, RAM backing (including B8000h–BFFFFh reclaimed
when VGA is removed), clock and RTC settings. Validation tests cover a
Landmark-like synthetic identity with no services and both controllers, and
preserve ordinary BIOS and unlisted-image rejection.

The existing `user_rom_landmark_at_real_mode_prefix` measurement and historical
Real16 constants remain unchanged, with their original diskless configuration.
They are historical prefix evidence, separate from the full-pass golden. It loads
a temporary machine file extending `preset:ibm-5170`, using absolute part paths
(or an assembled image), `provides = []`, explicit RTC start and volatile derived
NVRAM. It follows `run --machine`: `machine_file::load_for_assembly`,
`LoadedMachine::assemble`, then `run_session` with `RunPolicy::natural`, the
30,000,000-step cap and `pc_harness::BIOS_IDLE_LIMIT`. Catalogue membership is required;
no unlisted escape is used for the user ROM. This selects the preset hardware
(640 KiB base RAM, MDA and VGA text, UART, wired FDC without a drive),
volatile derived NVRAM, RTC start `2000-01-01T00:00:00`, and CPU revision
`7a9ea390b8f588ce4af9845980bbf8534f793f61`, now selecting the I286 machine profile.
The former Real16 golden checked the unsupported-instruction exit, address,
opcode, twelve Passed verdicts and the empty thirteenth status, re-confirmed
by the supervisor at `44403dd` as recorded below.
The I286 measurement prints a safe stop summary, CS:IP, retired/completed
counts, virtual time, MDA hash and verdict classifications. It does not read
or print stop opcodes or screen text. Stop/count/hash constants remain
TODO-MEASURE until the supervisor supplies a local capture; the former
twelve-test prefix remains historical evidence.
Synthetic tests cover absolute paths (including escaped filenames), lane order,
preset devices, RAM backing, clock and RTC/NVRAM settings through this loader.
The preset retains RAM at A0000h–AFFFFh alongside the two text-card apertures;
CMOS reports 640 KiB base RAM, no extended RAM and equipment byte 30h:
Auto selects the installed MDA (video bits 11b), with no floppy drive or
coprocessor bits set. This diagnostic BIOS does not use the catalogued Kiselev
switch override.
The preset clock rate is 8/13 retired steps per PIT clock, with the refresh
detect bit driven by PIT channel 1 OUT rising edges.

**Adapter-gap defect and fix:** MEASURED by the supervisor before the fix:
the Listed ROM stopped on `CpuError::Unmapped` at physical C0001h, with CS F000h,
RIP 9CFEh and 3,801,531 retired instructions. Both frontend and direct assembly
use the same assembler. Its old gap mapping depended on the selected firmware's
scan window: Landmark's None omitted the gaps, whereas an unlisted image fell
back to Kiselev's window and received them. `--allow-unlisted-firmware` never
bypassed a matching listed identity.

Assembly now gives boards with a firmware catalogue read-only, zero-filled
gaps from C0000h to the system BIOS base, around ROM images and MMIO windows.
This board property is independent of firmware scan capability. A synthetic
Listed 64 KiB BIOS without a scan window checks the complete C0000h–EFFFFh gap,
a guest read at C0001h and an ignored guest write. The test failed before this
change and passes afterwards. Successful CPU map calls captured before the fix
are compared after it for all four repository machine files and the IBM 5170
preset with both Kiselev BIOS variants; their complete region lists are equal.
The gap value remains 00h. Whether empty sockets should read FFh needs electrical
evidence and is a separate fidelity question. The measured prefix below confirms
the intended SIDT stop after this fix.

`CpuError::UnsupportedInstruction.addr` is the real-mode RIP offset A3CEh,
not linear FA3CEh. A synthetic SIDT test pins that contract. Unexpected stops
report the error variant and typed addresses, current CS/RIP, retired count,
completed scheduling steps and virtual time, including successful run outcomes
such as `RetiredCapReached`. Free-form CPU diagnostic payloads are omitted to
avoid copying guest bytes; no screen text or instruction dump is printed.
The text-plane hash covers the visible 80×25 glyph/attribute byte pairs in
row-major order, excluding cursor and decoded style. Only the status cell on
each line is classified; no row labels or screen dumps are retained.
Historical supervisor measurements at x86jit `8979fc9`, `a92b676` and
`44403dd`, with the former 1/1 rate and per-read refresh bit, retired
15,822,937 instructions and produced the text-plane hash below.

**MEASURED by the supervisor on 2026-11-11:** the user-ROM lane on the
calibrated `preset:ibm-5170` at 8/13, with PIT-driven refresh and CPU
`44403dd85077c0db29ad09780003ebd3b193ecdd`, produces:

- rows 1–12: Passed; row 13 status: empty (unchanged);
- unsupported SIDT at F000:A3CE (unchanged);
- retired instructions: 15,785,137;
- MDA text-plane SHA-256 (unchanged):
  `373ae9a68d11ea20d8da2232d39a90cdbb1b6f573b9fb28ea04e3ab79d3b31d1`.

PIT-driven timed waits and the calibrated throughput reduce the retired count;
the diagnostic reaches the same logical stop and screen. Firmware remains
user-supplied, and the test keeps its clean skip when the ROM is absent.

READ (measured by the supervisor): the left box border is at zero-based column
0, the test name starts at column 2, and the right border is at column 47.
The status field is columns 35–46 on frame rows 1–13. Trim that field and
classify `PASSED`, `FAILED` or `NOT PRESENT`; spaces denote an untested row.
The former Real16 golden asserted the retired count and text hash
unconditionally. The catalogue entry remains Listed; the full-pass regression
now pins the supervisor's media-equipped I286 measurement above. Both tests
retain their `CPU_REV` pin check. The separate historical prefix configuration
and its constants remain unchanged.

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
| `upc-fro.3` | User-supplied Landmark AT catalogue entry implemented with supervisor-measured hashes |
| `upc-fro.4` | Real-mode-prefix golden pinned per §3 from two identical supervisor runs |
| `upc-fro.5` | Full-pass golden; PM beyond `xj-srh.15 (x86jit)`, RAM above 1 MiB, empty-socket FPU (`xj-562.11 (x86jit)`, `upc-18x.9`) |
| `upc-fro.6` | Ruud XT v5.7 fixture for `ibm-5160`, P4; licence decision before fetching |

Conditional only: file a 74LS612 page-register read-back bead for port 80h
while retaining POST capture **if the golden demonstrates that failure**.
The measured prefix passed this row, so no page-register implementation or
follow-up bead is needed from this run. Since 2026-11-15 the POST port at
80h keeps POST capture and also reads back its last write, because the IBM
INT 15h AH=87h block move reads its status there after a processor reset
([`at-pm-test.md`](at-pm-test.md#int-15h-ah88h-and-ah87h)).

## Sources

Source access date in the investigation: 2026-11-11. Hashes labelled MEASURED
above come from that investigation or the supervisor measurements identified
above, not from the websites' descriptions.

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
- x86jit [`x86jit-core/src/lift/mod.rs` at 8979fc9](https://github.com/unemu-org/x86jit/blob/8979fc95cd81d73763d50d1d627b40ded3f8f284/x86jit-core/src/lift/mod.rs).

## IBM 5170 POST measurement

`user_rom_ibm_5170_1985_post` uses hash-discovered 1985-06-10 BIOS parts or
an assembled image, replacing Kiselev on `preset:ibm-5170`. It selects MDA,
`display_switch = "auto"`, an IDE card with `controller_model = "at-adapter"`,
and a read-only 1.2M A: drive with `vendor/freedos/freedos-1200.img`. NVRAM is
volatile and derived; RTC start is `2000-01-01T00:00:00`. Execution waits for
FreeCOM's date question, injects Enter twice to accept date and time, then polls
for `A:\>` or `A:>` at the existing one-million-step cadence. A
200,000,000-step safety cap bounds the run. The ignored lane skips absent user
dumps; missing floppy media fails with its relative path and fetch command.

**MEASURED at CPU `fd67927` with locally supplied firmware:** POST passes cleanly
and the DOS prompt is reached. The stop is `Ok(RetiredCapReached)` at
`0070:02AD`, with 24,872,018 retired instructions, 27,000,000 completed steps
and virtual time 27,000,000. The DOS-prompt verdict is true. The regression
pins the complete port-80h checkpoint sequence, stop location, retired count,
prompt verdict and final MDA text-plane SHA-256
`c02b5aeabf65118af62136535b8637d42a4792378d0e407463c3865debc484e7`.
Reports retain checkpoint codes, counts, verdicts and the hash; no screen text
is emitted. This qualifies this BIOS revision and floppy boot configuration.
