# Machine-configuration layer

Design for the **declarative machine-composition layer**: a plain-data
`MachineConfig` and a single data-driven assembler that turns it into a running
`Machine`. This is the missing home between the swap traits
([spec.md §3](spec.md), decision-7) and the ~13 hand-written `run_*` functions in
`crates/cli`.

Status: **design accepted, unimplemented.** This document is the contract that
tasks task-28 through task-37 build against. Section-number cross-references
(`§N`) point at [`spec.md`](spec.md).

Related: [`spec.md §3`](spec.md) (machine assembly), decision-7 (swap seams and
its task-28 annex), decision-4 (IRQ-injection / interrupt latency), decision-5
(retired-instruction clock), decision-8 (A20 always-on).

## The problem

The swap traits exist (`Cpu`, `Chipset`, `Bus`, `Device`, `FrameSink`), but
there is **no declarative layer above them** that says *what machine to build*.
Machine composition today is hand-written assembly code, and it has drifted into
three separate, partially-overlapping copies:

- **`crates/cli` `run_*` functions** — around thirteen of them
  (`run_bios`, `run_bios_boot`, `run_freedos`, `run_freedos_interactive`,
  `run_test386`, `run_serial`, `run_boot`, `run_keyboard`, `run_rtc`,
  `run_vga_text`, `run_floppy_boot`, …). Each hand-assembles a device list, a
  port/MMIO decode map, IRQ and DMA wiring, seeds the CPU, loads images, runs a
  bespoke loop, and drains outputs. They share structure by copy-paste, not by
  construction.
- **`AtChipset::build` / `SerialChipset::build`** in `crates/chipset` — the
  trait-based composition path. It declares a decode map and a device set, but
  it **cannot build the FreeDOS machine**: the FreeDOS path needs a floppy
  controller, a DMA controller, a POST-code latch, and a system-control port
  that `AtChipset` does not assemble.
- **`assemble_bios_machine` / `assemble_bios_machine_quantum`** in `crates/cli`
  — a *third* copy that does what `AtChipset` cannot, and therefore duplicates
  most of what `AtChipset` does.

On top of the duplication, device identity is **hand-counted integer indices**.
`crates/chipset` exports `AT_UART_INDEX = 0`, `AT_PIC_INDEX = 1`, …; `crates/cli`
re-declares its own parallel set `BIOS_UART_INDEX = 0`,
`BIOS_MASTER_PIC_INDEX = 1`, … up to `BIOS_DMA_INDEX = 10`. Every decode entry,
every IRQ line, every drain call addresses a device by a literal index that must
be kept in lock-step with the order devices were pushed onto a `Vec`. Insert a
device in the middle and every later index silently shifts.

The result is combinatorial debt with no home: N machines × M devices, each
combination transcribed by hand, with three drifting copies of the composition
logic and no single source of truth for "device X lives at index Y with these
ports and this IRQ line." The traits are the right seams; what is missing is a
**declarative description of a machine** and **one** function that realises it.

## Data model

A machine is described by a single value, `MachineConfig`, that names *what the
machine is* — not how it is run (that is `RunPolicy`, below) and not any
concrete Rust device type (that stays behind the assembler, decision-7).

```
MachineConfig {
    board:   BoardModel,         // motherboard constraints; class is derived
    cpu:     CpuConfig,
    clock:   ClockConfig,
    memory:  MemoryConfig,
    devices: Vec<DeviceConfig>,   // a flat list, produced by preset functions
    drives:  Vec<DriveConfig>,    // the ONE home for attached media
    images:  Vec<LoadImage>,      // the ONE home for pre-loaded bytes
    system_bios_image: Option<usize>, // declared BIOS index, if known
    output:  OutputConfig,        // initial presentation subscribers
}
```

`MachineConfig` and every field type derive `Eq` and `Debug`. The config is
**identifiable**: two configs are equal iff they describe the same machine. That
is what lets a golden test pin the machine it ran against, and it is the
precondition for a future snapshot/save-restore feature (deferred — see
[`deferred.md`](../deferred.md)).

### Board model

`MachineConfig.board` is `BoardModel::Ibm5170` or `BoardModel::Bare`. The BIOS
preset selects `Ibm5170` when floppy media is attached. Its no-media variant
is a partial motherboard rig on `Bare`; all demos and hand-wired Rust test
presets also select `Bare`. `BoardModel::class()` derives `Some(MachineClass::At)` for the AT and
`None` for a bare rig. Board identity is part of config equality, never an
execution option. The assembler still constructs only the flat device list;
selecting a board validates its wiring rather than inserting hidden devices.

`Ibm5170.motherboard_devices()` supplies the default motherboard data in bus
order. Cards follow in their declared order. The wiring reference is the **IBM
Personal Computer AT Technical Reference**, Section 1 **System Board**, under
**System Interrupts**, **System Timers**, **Direct Memory Access**, **I/O Address
Map**, **Keyboard Controller**, and **Real-Time Clock/CMOS RAM Information**.

| Order / part | Modelled wiring and coverage | Missing / deviation |
|---|---|---|
| 1. `MasterPic` | 8259 at 20h–21h, cascaded master | — |
| 2. `SlavePic` | 8259 at A0h–A1h, slave on master IRQ2 | — |
| 3. `Pit` | 40h–43h, IRQ0 | Existing 8253 model stands in for the AT's 8254 |
| 4. `Dma` | 8237 at 00h–0Fh, channels 0–3, page latches 81h–88h | No refresh DMA requester |
| 5. `DmaWord` | Second 8237 at C0h–DFh (odd ports alias the preceding even register), pages 89h–8Fh, channels 4–7; channel 4 is cascade | No cycle-level arbitration between banks |
| 6. `Keyboard` | 8042 at 60h/64h, IRQ1; D1h output bit 0 and F0h–FFh pulses request CPU reset | A20 remains always on; pulse duration and temporary output levels are not timed |
| 7. `Rtc` | MC146818 at 70h–71h, IRQ8, NMI-mask state | Seed and virtual timing remain configurable |
| 8. `Coprocessor` | Empty 80287 socket: F0h, F1h, F8h–FFh; IRQ13 wired inactive | Empty bus socket only; CPU x87 is currently ungated |
| 9. `SysControl` | 61h latch | Existing refresh approximation and speaker state |
| 10. `Post` | Diagnostic latch at 80h by default | Optional, relocatable diagnostic decode |

The **IBM Personal Computer AT Technical Reference** (September 1985),
System Board, **Direct Memory Access**, pp. 1-9–1-11, supplies the two-bank
address/page wiring. The word bank uses local channel numbers 0–3 for system
channels 4–7. Channel 4 never transfers payload. Channels 5–7 step one word
per count, with physical address `(page & FEh) << 16 | address << 1`; wrapping
stays within the same 128 KiB page. Pages 8Bh/89h/8Ah serve channels 5/6/7;
8Fh is the refresh latch, not a channel-4 address extension. Register access
is still byte-wide. The System Board **I/O Address Map** (p. 1-37) and
DMA wiring decode all of C0h–DFh: SA1–SA4 select controller registers, so
`(port - C0h) >> 1` maps odd ports to the preceding even register. A CPU word
OUT is split into two byte cycles that access the same flip-flop register.
The Intel 8237A datasheet's **First/Last Flip-Flop**,
**Master Clear**, **Current Address/Word Count**, **Autoinitialization**, and
**Cascade Mode** semantics. Both DMA directions support complete words;
an unmatched trailing byte is left untouched. No assembled card requests
channels 5–7 yet. The two banks retain the existing explicit-service model:
no HRQ/HLDA arbitration latency or gating of DMA1 by DMA2 is simulated.

The same IBM reference, **I/O Address Map** (p. 1-37), **NMI and Coprocessor
Controls** (p. 1-38), **System Interrupts**, and Section 2 **Coprocessor**, assigns
F0h to clearing latched BUSY, F1h to coprocessor reset (both write-only;
software should write zero), and F8h–FFh to the 80287. F2h–F7h remain
unmapped. An empty socket does not drive read data or IRQ13: reads on every
assigned port return the emulator's open-bus byte FFh, and writes of any
value are discarded. FFh is the emulator's bus convention, not an IBM-defined
absent-coprocessor status value. This describes only the I/O bus socket.
The pinned CPU core currently executes x87 in Real16 without an FPU-presence
gate: FNINIT/FNSTCW writes 037Fh to a flat scratch word, despite the empty
board socket. An absent-FPU model is requested from the CPU core project;
this repository does not intercept or block x87 instructions.

**Keyboard Controller**, **Commands** and **Output Port** (pp. 1-53–1-55)
connect output bit 0 to active-low System Reset. D1h lowering that bit latches
a request; F0h–FFh pulse commands latch one whenever command bit 0 is zero,
including FEh. Odd pulse commands do not request reset. The approximately
6 microsecond pulse is represented as an event rather than a timed pin level;
D0h reads the retained output latch. The pending request survives device
snapshot/restore and is cleared by device reset. Assembly shares a signal
with the machine without teaching the machine core about an 8042. At the
completed instruction boundary the machine returns `RunOutcome::ResetRequested`,
even at a coincident cap, and consumes the pending event. `Machine::run()`
also returns that outcome explicitly. Another call can continue executing;
the CPU has not been reset. CLI natural, prompt and post-prompt runs keep the
same absolute cap while continuing past reported requests; the window loop
counts every request and continues. It logs only the first request in a
session, then logs the total when the event loop exits, including error exits.
Sessions with no requests produce neither message. The counter saturates at
`u64::MAX`; counting and log selection use no host clock. This interim
behaviour preserves guest reboot code that falls back to a far jump through
FFFF:0000. CPU restart is
not available in the current backend and belongs to separate work. RAM,
peripheral state and virtual time are retained. See `status.md`, **Broken /
known defects**.

Every `Ibm5170` requires the 8237 in its canonical position, even if an FDC
entry is undecoded or unwired. The no-media BIOS preset keeps that historical
partial machine on `Bare`: an undecoded, unwired FDC derives CMOS drive A: while
no 8237 is assembled. Changing that preset's board identity affects validation
only, preserving its device list and execution.

Omitting or moving `Post` away from 80h changes the expected page-latch range
to 80h–88h. POST is validated before deriving this range, so a wrong kind or
decode is diagnosed against `Post` itself. Removing the optional POST role
closes up the order.

Board validation follows unique-ID validation and precedes general wiring
checks. Each rule returns a stable, exact-message-tested `ConfigError`:

| Rule | Error |
|---|---|
| Required motherboard role is present | `BoardDeviceMissing` |
| Motherboard devices precede cards, in the table's order | `BoardDeviceOrder` |
| Entire `DeviceConfig` matches board data, except the contents of `CmosCfg` | `BoardDeviceWiring` |
| Card IRQ is 3–7, 9–12, 14 or 15; 0/1/2/8/13 stay reserved | `BoardCardIrq` |
| Card DMA channel is 0–3, the modelled 8237's range | `BoardCardDma` |
| Card DMA controller is the board's `DeviceId::Dma` | `BoardCardDmaController` |

`BoardDeviceWiring` names the device, differing field, expected value and actual
value. The full comparison includes port-overlap allowances and scheduling
`access_dependents`; neither can silently override board wiring. RTC
initialization and virtual timing remain configurable. A literal-table unit test pins the board
ports, IRQs and DMA requests independently of both preset and validator.

There are no resource override rules in this implementation. Port-overlap
allowances do not authorize IRQ or DMA overrides. Existing IRQ conflicts and
DMA-controller-presence checks still apply. `Bare` skips board rules and firmware service requirements;
general decode, resource and drive-binding validation still runs.

### Drive binding and write mode

```rust
DriveConfig {
    kind: DriveKind,             // Floppy or HardDisk, with geometry
    image: ImageSource,
    controller: DeviceId,        // named device, never a global slot number
    unit: u8,                   // unit local to that controller
    write: WriteMode,           // Volatile (default), ReadOnly, or Journal
}
```

A binding must name an existing device (`DriveControllerMissing`) of the right
kind (`DriveControllerKind`): `Floppy` on `DeviceKind::Fdc`, or `HardDisk` on
`DeviceKind::Ide`. The unit must be below the controller's declared slot count
and hardware limit (four for FDC, two for IDE; `DriveUnitUnavailable`). Two drives
cannot bind to the same `(controller, unit)` (`DuplicateDriveUnit`). Each error
has an exact-message unit test. These rules apply on both boards.

AT CMOS describes only diskette drives bound to the primary `DeviceId::Fdc`.
Floppies on another named controller do not contribute type nibbles or the
equipment-byte drive count, even when their local unit is 0 or 1. This rule also
applies to `Bare` rigs using the same CMOS derivation. A primary 1.2 MB unit 0
and a secondary 720 KB unit 0 therefore derive `floppy_types = 0x20` and
`equipment = 0x01`.

`WriteMode::Volatile` replaces `write_protect = false`: guest writes modify only
the mounted in-memory image. `WriteMode::ReadOnly` replaces `write_protect = true`
and uses the same `Drive::new_read_only` constructor: WRITE DATA reports the
existing not-writable status before requesting DMA or modifying media. Neither
mode writes the host image or creates host output.

`WriteMode::Journal { path, keep_generations }` requires an explicit journal
path and positive retention count. `WriteMode::journal(path)` supplies the
default count of ten. The enum makes journal-only fields on other modes
unrepresentable. Journal mode requires `ImageSource::File`, a floppy and
512-byte sectors. Assembly rejects normalized-path and existing-file-identity
aliases (including hard links) between journal/lock outputs, any mounted drive
or load image, and other journal/lock outputs before creating files. It reads
the base read-only once, then opens, recovers and replays the journal before the
first guest instruction. A mount refusal names the drive and its cause.

The assembled session owns per-drive persistence handles. Embedders use
`run_session` (or check handles at their own completed service boundaries),
`checkpoint` for explicit recovery points, and `finish_persistence` only after
successful execution or a normal policy-limited end. Dropping a failed run
leaves its commands unsealed for recovery; host failures never enter controller
status or virtual time. The retention count is retained for the separate
retention service; automatic pruning and offline tools are not wired yet.
See [Persistent media](persistent-media.md#assembly-session-api).

### `CpuConfig` and `ClockConfig` — minimal but the right home

These two structs start **deliberately minimal** but exist so that future
bus-speed and CPU-type work is a *field addition*, never a re-architecture.

```
CpuConfig {
    chip:         ModelledChip,   // marker for the modelled part (today: one variant)
    reset_vector: SegOff,         // CS:IP the CPU starts at
    segments:     SeedSegments,   // DS/ES/SS + SP seeded at reset (see below)
}
```

The CPU core models segment *bases*, not selector loads (a real-mode guest that
does not run its own segment setup relies on the machine seeding CS/DS/ES/SS/SP;
see [`deferred.md`](../deferred.md) and the `new_real16*` constructors in
`crates/cpu`). `CpuConfig` is where that seeding is declared, so the assembler
picks the right constructor from data instead of the caller picking it by hand.

```
ClockConfig {
    retired_per_pit_clock:             u64, // positive numerator A
    retired_per_pit_clock_denominator: u64, // positive denominator B
}
```

The CPU rate A/B counts completed steps per PIT input clock. An instruction is
one step and each completed REP iteration is one step. The carried remainder
preserves the ratio across CPU calls. This is a modelling ratio, not a nominal
frequency. Block quantum and interrupt-timing selection have been removed:
interrupts are accepted only at completed instruction boundaries after bus and
DMA effects, with IF and STI/SS-load shadows respected. Cooperation size is not
machine identity. Production always uses the deadline engine; the non-default
`reference-scheduler` feature exposes a per-clock oracle only to tests, outside
`MachineConfig`. Assembly probes zero-step execution and returns a clear
`StepBoundedCpuRequired` config error for an unsupported CPU.

Neither struct carries a frequency axis or a behavioural CPU-type toggle today —
see [Non-goals](#non-goals) for why, and why keeping the structs is nonetheless
the point: when a real consumer and a real oracle appear, they arrive as new
fields here rather than a redesign of everything above.

### `MemoryConfig` — a semantic memory map

`MemoryConfig` describes the address space as regions with *kinds*, not a flat
byte count. Two things it must express that a naive "N kB of RAM" cannot:

- **A20 as a model constant.** A20 is always-on (decision-8, §4.4). The memory
  map is a flat 1 MB+ linear space with no wrap; A20 is not a configurable knob.
  The config records this as a fixed property of the modelled machine, so nobody
  reintroduces it as a per-machine option.
- **Firmware scan space is not system RAM.** IBM PC AT Technical Reference,
  Section 1 "System Board", **System Memory Map** assigns C0000h–DFFFFh to
  I/O expansion ROM, E0000h–EFFFFh to reserved system-board space, and
  F0000h–FFFFFh to system-board ROM. The model protects the wider C0000h–F7FFFh
  interval from RAM because Kiselev scans through F7FFFh. Card MMIO windows
  remain valid; assembly fills only gaps between ROMs and MMIO with read-only
  zeroes. Real empty sockets float, often to FFh; 00h is a deliberate model
  choice to preserve the goldens. Guest writes and `RamLoad` cannot create
  hidden firmware headers. The historical no-floppy `Bare` rig retains its map.

The MMIO list retains per-device windows. For CPU mapping, assembly sorts them
by address and coalesces each run of touching windows into one `RegionKind::Trap`
region. Non-adjacent windows stay separate; gaps and RAM are never included in
the merged trap regions. The bus retains the per-device decode map and splits
cross-window accesses into little-endian sub-accesses. This lets accesses across
adjacent devices reach the bus despite the pinned CPU's single-region containment
requirement. RAM/MMIO crossings remain a [known CPU limitation](../status.md#broken--known-defects).

### Port window width

`PortWindow.width` and the corresponding `bus::PortRange.width` in `DecodeMap`
use `PortWidth::{Byte, Word}`. The enum excludes unsupported widths.
`PortWidth::default()`, `PortWindow::new`, `single`, and `overlapping` all select
`Byte`; `.with_width(PortWidth::Word)` opts in. Every existing preset retains
byte cycles. Assembly carries each window's width unchanged through decode-map
sorting. Validation rejects a word window starting at an odd port with
`UnalignedWordPortWindow`. A kind-based table declares the permitted word
register addresses; every even cycle address covered by a word window must be
listed for that kind, otherwise validation returns `UnsupportedWordPort`.
The production table is empty because no assembled device kind currently
implements word registers. Synthetic declarations exercise the accepting path
only in validation tests; POST remains byte-only. The ATA card will declare its
data register when it is wired. Board validation also checks width as part of
the fixed motherboard wiring and reports a changed word width explicitly.

The width describes a cycle starting at a decoded port, rather than ownership
of individual data lanes. An aligned word in a word window reaches the device
once with size 2, even when the window covers only that even port and the next
port belongs to a byte window or is undecoded. This permits a word data register
beside a separately decoded byte register without reading both registers.
A word beginning at an odd port uses two independently decoded byte cycles.
An aligned 32-bit access to word windows uses word cycles at port and port+2. Each cycle
selects width from the first matching window at its own starting port, so mixed
windows can produce word/byte/byte or byte/byte/word sequences. Values are
composed/sliced little-endian; port numbers wrap at 16 bits. A fault stops after
completed cycles. Traces describe actual device/open-bus cycles, including size.

The hardware reference is the **IBM Personal Computer AT Technical Reference**,
Section 1, **System Board**, **I/O Channel**, **I/O Channel Signal Description**,
**-I/O CS16** (IOCS16#): the card selects a 16-bit I/O transfer. The odd-alignment
and mixed-window rules above define the emulator's dispatch contract; physical
byte-lane signalling and wait states are not modelled. No ATA card or device
configuration is added here; its data window can opt in when the card is wired.

### `DeviceConfig` — one device, fully described

The device list is **flat**. Each entry fully describes one hardware unit and
carries a stable identity:

```
DeviceConfig {
    id:    DeviceId,              // stable name; replaces the hand-counted indices
    kind:  DeviceKind,           // which modelled part (UART, PIC, PIT, …)
    ports: Vec<PortRange>,       // port range(s) it decodes
    mmio:  Vec<MmioRange>,       // MMIO range(s) it decodes (e.g. text buffer)
    irq:   Option<IrqLine>,      // the PIC input it drives, if any
    dma:   Option<DmaWiring>,    // requester/controller wiring, if any
    access_dependents: Vec<DeviceId>, // timers an access can affect; empty by default
}
```

`DeviceId` is the single fix for hand-counted indices. Devices are named, not
numbered; the assembler (below) returns a `DeviceId -> bus-index` map so every
drain/inject addresses a device by name. Inserting a device no longer shifts a
column of literals.

`IrqLine` and `DmaWiring` are the home for wiring the CLI frontend used to
declare by hand. This is the concrete fix for the partial decode-ownership
violation noted under decision-7: the IRQ/DMA relationships that were scattered
across `run_*` bodies now live in the device data the preset produces.

`access_dependents` declares a directed scheduling dependency: an access to
this device can change another device's next event. Validation requires every
dependent to be present and distinct from the source. Assembly resolves each
role to a valid bus index and registers the edge with `Bus::add_access_coupling`,
which also rejects invalid indices and self-coupling. Repeated edges are
idempotent; directed cycles are allowed because synchronization does not recurse.
Every indirectly affected timer must be listed explicitly.

Before a port cycle, MMIO fragment, or mutable borrow, the bus synchronizes the
target first, then its dependents in configuration order, regardless of edge
declaration order. Unrelated devices receive no access synchronization, and
legacy every-clock devices receive no extra sync. This makes dependent deadlines
start at the current access clock rather than their earlier lazy-sync time. The
field does not implement hardware signal wiring. No preset needs it today:
the shared PIC faces are dormant and have no autonomous timer, and system
control does not yet drive the PIT's speaker gate. See
[scheduler.md §Adoption](scheduler.md#adoption) for cache and settle ordering.

### Single homes (no triplication)

Two data-model decisions exist specifically to prevent the same information
living in three places:

- **`images`: one `LoadImage` list, not three.** Firmware ROM, a `memory.rom`
  blob, and a CPU code blob were three separate concepts. They collapse to one
  list of `LoadImage { base, bytes, kind: Rom | RamLoad }`. `kind` says whether
  the bytes land in a ROM region or are loaded into RAM (SMC-visible per
  decision-6). One list, one meaning.
- **`drives`: one top-level list, routed by controller and unit.** Attached media were split
  between an ad-hoc floppy argument and a controller's own drive list. They
  collapse to one top-level `Vec<DriveConfig>`, each routed to its controller by
  `controller: DeviceId` and `unit: u8` fields. The floppy controller does not own a private drive
  list; drives are attached to the machine and routed to controllers.

`DriveConfig` and floppy geometry are fleshed out in task-34; the top-level
placement and controller-local unit routing are fixed here.

### The fixed-disk (IDE/HDD) attachment seam

`DriveKind` is `Floppy { geometry } | HardDisk { geometry }`. The floppy side is
wired end-to-end: the assembler routes each `Floppy` drive into the µPD765's
drive slots by `controller` and `unit`, and the CMOS `floppy_types` nibbles and
equipment byte are *derived* from the attached floppies, so attaching a second
drive of a different geometry needs no manual CMOS edit — the whole point of the
"attach different drive types" story.

`HardDisk` binds to `DeviceKind::Ide(IdeCfg { base, control })` by controller
and unit: master 0, slave 1 (a slave requires a master). One channel is supported,
with base `0x1F0` or `0x170`, control at base + `0x206`, IRQ14 or IRQ15,
and no DMA. Only the data register at base supports word cycles; all other
task-file and control registers use byte cycles. Lowering splits primary FDC
windows with `FdcCfg::port_windows(true)`; Rust callers supply the same split.
The secondary channel leaves the FDC's legacy decode intact.

Assembly mounts `InMemoryMedia` through `SectorMedia`, selecting volatile or
readonly writes. Geometry must have 1–16383 cylinders, 1–16 heads, 1–63 sectors
per track and 512-byte sectors; the image must exactly match it. Journal mode
remains floppy-only. ATA submits one batch per completed write command, including
complete sectors before an address failure, command replacement, diagnostics,
reset or successful snapshot restore. A partial sector is discarded. Protected
writes finish with ERR/ABRT and submit no batch. Mounted media and the completed
prefix of an active command remain outside snapshots; restore does not roll back
committed sectors. The host-only `Ata::flush_pending()` submits any complete
prefix without changing guest-visible state. A future persistent IDE backend
must call it only at session end, when the guest will no longer finish or abort
the pending command; calling it mid-command splits one command into multiple
batches. The host reaches ATA through the device's `as_any_mut` seam.
`SectorMedia::sector_count()` bounds the advertised ATA capacity at construction. ATA-2 §§6.2.8 and 8.4 define abort reporting and
PIO data-out.

CMOS fixed-disk types follow the firmware provider. Registers 12h, 19h and
1Ah stay zero when an option ROM provides `fixed-disk`, including XUB. They
are derived from the primary IDE `HardDisk` units only when the hash-matched
system BIOS catalogue entry both provides `fixed-disk` and declares the
`cmos-drive-types` capability. Kiselev does not declare that capability.
The IBM AT September 1985 fixed-disk tables supply standard types 1–14 and
extended types 16–22 (nibble F plus 19h/1Ah). CHS duplicates choose the lowest
number. Other geometries return `UnsupportedCmosDiskGeometry`: IBM reserves
type 47, so no vendor-specific user-defined layout is invented. See
[firmware and machine classes](firmware-and-machine-classes.md), Derived
configuration, for the reference and firmware-source evidence. The XTIDE
boot profile remains separate work.

A byte cycle on the ATA data register (`in al, 1F0h`) stops the run with
`UnmappedRegister`: this AT model accepts only a word cycle at the data
register, matching its IOCS16# word transfer contract. XTIDE AT builds use
word port I/O. The other task-file registers remain byte-wide.

## Firmware services

`LoadImage.provides: Vec<FirmwareService>` declares guest services supplied by
a ROM. The initial vocabulary is deliberately small:

| Variant | File vocabulary | Guest interface |
|---|---|---|
| `Floppy` | `floppy` | INT 13h diskette services |
| `FixedDisk` | `fixed-disk` | INT 13h fixed-disk services |

System BIOS and option ROM images use the same field. The BIOS preset declares
`Floppy`, matching the machine-file examples; this validation metadata does not
change guest execution. Firmware identification uses the separate catalogue
below. A recognized BIOS or option ROM supplies its catalogue services when the
declaration is omitted or empty; a nonempty restatement must match the catalogue
set. A conflict returns `FirmwareProvidesMismatch` with both sets. Unlisted
firmware under the escape option uses its declared services.

On firmware boards (every board other than `Bare`), requirements follow from a
table of device kinds. An FDC with at least one drive bound to its device id
needs `Floppy`. The table records the matching kind, required service and whether
a bound drive is necessary. IDE requires `FixedDisk` even without a bound drive.

Validation checks required services against the union of ROM declarations and
returns `NoProvider { device, service }` in device/table order. `Bare` rigs skip
these requirements, and an FDC without bound drives needs no provider. A ROM
service does not require a consumer. RAM loads with nonempty `provides` fail with
`ServicesOnRamLoad`; a RAM program cannot satisfy a firmware requirement.

## Firmware catalogue

`BoardModel::firmware_catalogue()` owns the reviewed `FirmwareEntry` values.
The IBM 5170 catalogue contains `xub-r638-at` (8 KiB, fixed-disk service,
primary ATA requirement) and `kiselev-xi8088-1.0.2`, the unmodified 32 KiB
Xi 8088 build pinned by `vendor/kiselev-bios/fetch.sh`. Its GPL-3.0-or-later
licence, upstream revision, SHA-256, load rule, service metadata, required
8042/RTC/cascaded PIC parts, 80286 minimum and tested status are data in
`crates/assembly/src/firmware.rs`. `Bare` owns no catalogue and assembly skips
both catalogue and BIOS-placement checks for these hand-wired rigs.

Assembly resolves file sources once, then hashes the actual bytes before any
ROM mapping; inline and generated system BIOS images follow the same check.
Option ROMs are also hashed and allowlisted; `OptionRomNotListed` lists the
known adapter builds. The ROM ending at `0x100000` is the system BIOS,
regardless of image-list order. Its size must be positive and divisible by 8 KiB,
and its end must be `0x100000`. This check precedes the existing general region bounds
check, so an overlong system BIOS reports `BiosPlacement`, including its base
and an exact size (bytes when not divisible by 1024). Catalogue boards reject a
missing BIOS (`MissingSystemBios`), a reset other than power-on
`ResetVector F000:FFF0` (`FirmwareReset`), RAM shadowing the BIOS or its reset
byte (`SystemBiosOverlapsRam`), and other overlapping images (`ImageOverlap`).
Since every valid system BIOS ends at 1 MiB, at most one can satisfy placement
without an overlap. If none ends at 1 MiB, `system_bios_image` identifies the
explicitly declared BIOS; its invalid size or end reports `BiosPlacement`.
The BIOS preset (including the CLI `--bios` path) and machine-file lowering set
this index. When the index is unset, a sole ROM is also a BIOS candidate and
reports `BiosPlacement`. Zero ROMs, or multiple undeclared ROMs with no top-aligned
candidate, report `MissingSystemBios`; address order never resolves ambiguity.
Callers reordering images must update the declared index. File-backed ranges
and identity are checked after resolution; machine files validate their explicit
BIOS placement at its defining image origin.
Other image/region errors retain their existing variants. The catalogue's
`base` field is informational for system BIOS entries: all use the common top
alignment rule, which must also be enforced before looking up unknown bytes.
Option ROMs require explicit 2 KiB-aligned bases and must fit the BIOS entry's
`scan_window` (Kiselev: [C8000h, F8000h)). The 55AAh signature, nonzero length
byte × 512 equal to the image size, zero byte-sum checksum, and absence of image
overlap are checked before mapping. Firmware without a scan window refuses
option ROMs with `FirmwareDoesNotScanOptionRoms`. `AssembledMachine.option_roms` contains the
resolved images with effective services in ascending address order. Suggested
catalogue bases remain informational; machine files specify the base.
Unknown bytes report `FirmwareNotListed` with the full SHA-256 and
every known entry's distribution and fetch script. Listed entries must also
match the board, CPU minimum and required parts (rules 7–9 in the firmware design).

`AssemblyOptions { allow_unlisted_firmware: true }` is a run-only escape hatch,
passed to `assemble_with_options` or `assemble_with_observation_and_options`.
It never enters `MachineConfig`, machine files or goldens. Accepted unknown
hashes are returned in `AssembledMachine.unlisted_firmware`; frontends print
one warning per hash. The option only relaxes catalogue membership: listed
entries still satisfy compatibility checks, and every checked BIOS still
satisfies placement. Required BIOS presence, memory isolation and power-on
reset are also mandatory with the escape option. Synthetic IBM 5170 assembly
tests may opt in for unknown hashes; rigs needing custom entry points select
`Bare` explicitly without changing their device set.

The historical no-floppy POST preset remains a `Bare` rig to preserve its
hardware and golden. The CLI POST runner and scheduler comparison explicitly
check its BIOS against the IBM 5170 catalogue through the shared assembly
helper `check_board_system_bios` before assembling that partial rig. Complete
BIOS/floppy and FreeDOS machines use normal strict assembly; all existing
golden expectations remain unchanged.

## RTC start and NVRAM source

`CmosCfg { start: RtcStart, nvram: NvramSource, timing }` separates the fixed
calendar seed from the initial store. `RtcStart` holds binary `year`, `month`,
`day`, `hour`, `minute`, and `second`; the weekday is derived, with Sunday = 1.
Its default is exactly **2000-01-01 00:00:00, Saturday (7)**. This implementation
accepts valid dates in 1980–2099, matching the existing two-digit year and
four-year leap cycle; invalid dates, hours, minutes and seconds fail with
`InvalidRtcStart` before assembly, naming the first invalid field and showing
the supplied time as `YYYY-MM-DDTHH:MM:SS`. Host time has no config variant or
runtime clock read. The default RTC second is exactly **3,579,545 / 3 PIT
input clocks**, derived from the nominal **14,318,180 / 12 Hz** timer input
(IBM PC AT Technical Reference, System Board, **System Timers** and
**Oscillator**). The calendar phase counts thirds of a PIT clock. For the
32,768 Hz periodic divider, `oscillator_phase` carries a remainder in units of
1/3,579,545 of an oscillator clock: each PIT clock adds `3 * 32768`, and division
by 3,579,545 yields whole oscillator clocks for `divider_phase`. Both counters
retain their remainders; deadlines round up to whole PIT clocks, so batching
cannot accumulate drift. The UIP window keeps the existing approximate
1 ms policy: floor(3,579,545 / 1000) thirds = **1193 PIT clocks** before each
update. Motorola MC146818 datasheet, **Update Cycle**, specifies the once-per-second
update and 244 µs from UIP assertion to update start; the model retains its
longer polling window. Explicit `CmosCfg.timing: Some((second_ticks, uip_ticks))`
and `with_timing(second_ticks, uip_ticks)` retain integer PIT-clock units,
including the CLI RTC demonstration's small timing ratio. Snapshots include the
phase-unit scale as well as the calendar and periodic remainders.

`NvramSource` has three variants:

| Variant | Initialization |
|---|---|
| `Derived` (default) | Existing machine-derived AT seed and checksum |
| `Blank` | Existing unseeded chip, retaining its control defaults |
| `Image([u8; 64])` | Writable bytes copied without checksum repair; read-only status initialized to power-on values |

The fixed array enforces image length at the Rust boundary. A future machine-file
loader must reject any file whose length is not 64 before creating this value;
assembly does no NVRAM file I/O. All guest writes are volatile; file writeback
and journal policy are deferred.

The image's time-register backing bytes are preserved, but reads use `start`,
encoded according to the image's register B. Loading clears A's read-only UIP
bit and all of C, recomputes IRQF, and sets D to the device's valid-RAM value
(80h, reserved bits clear). This prevents stale image status from raising IRQ8
at power-on or producing an unrestorable snapshot. Subsequent control reads
retain normal semantics: A overlays live UIP, C clears flags after reading,
and D reports valid RAM/time. These register meanings follow the Motorola
MC146818 datasheet, **RAM**, **Time, Calendar, and Alarm Locations (Table 3)**,
and **Registers A–D**. General RAM and alarm bytes are not normalized, and no
AT checksum is repaired for an image. `Derived` with the default start retains
every register and backing byte of the prior construction; tests compare the
entire device snapshot and all 64 port-visible registers, alongside unchanged
machine goldens.

The century of `RtcStart` does **not** populate the BIOS century byte at 32h.
`Derived` leaves that byte at 00h, as before; `Image` retains the supplied byte.
Use an NVRAM image to set 32h when firmware needs an explicit century. The live
year register remains the two-digit year, including the 99-to-00 rollover.

## The assembler

`fn assemble(config: &MachineConfig) -> Result<Assembled, ConfigError>` is the
one function that turns data into a `Machine`. It replaces `AtChipset::build`,
`SerialChipset::build`, and `assemble_bios_machine` with a single realiser.

**Placement (hard constraint).** The assembler lives in a **new crate above the
`machine` crate**. The machine core must never name a concrete device or CPU
type (decision-7, §3.2); it depends only on the traits. The assembler is
precisely the layer that *does* know the concrete `DeviceKind -> Box<dyn Device>`
mapping, so it sits above `machine`, alongside where the concrete devices are
known — not inside the core. Built in task-30.

**Return value.** The assembler returns the built `Machine` **and a
`DeviceId -> bus-index` map**. Drain and inject helpers address devices through
this map, by id, so the hand-counted `*_INDEX` constants disappear entirely.

**CMOS defaults to a derived configuration.** The assembler derives floppy
types, equipment and memory sizes from the attached drives and memory map.
Fixed-disk types additionally require the capable system BIOS provider described
above. `derive_cmos_config` returns a fallible, complete 64-byte seed, including
12h/19h/1Ah and the AT checksum at 2Eh/2Fh (sum of 10h–2Dh). An explicit NVRAM
image bypasses derivation; the caller is then responsible for its contents. Calendar initialization is separate
from the NVRAM source, as described below.

**Explicit decode-conflict policy.** Port/MMIO decode is **first-match order**,
and that order is *semantics*, not luck. The canonical example: `0x80` is the
POST diagnostic latch while `0x81` is a DMA page register — adjacent ports owned
by different devices, where a sloppy range would swallow the wrong one. The
assembler therefore:

1. Sorts port entries by `(start, end)` and MMIO entries by `(base, end)`
   **deterministically**.
2. Rejects every unflagged port overlap with `PortOverlap`, and every MMIO overlap.
   A port overlap flagged by `PortWindow.allow_overlap` or
   `DeviceConfig.allow_port_overlap` is accepted only when one window is strictly
   contained in the other and both start at the same port. Sorting then places
   the narrower window first, giving it every shared port. Identical windows,
   partial overlaps and containment with different starts fail with
   `PortOverlapDecodeOrder`, which names both devices, both windows and the first
   shared port and asks the caller to split the wider window or move one.

Deterministic sorting and validation make the decode map a pure function of the
config, and an accidental collision fails loudly at assembly time instead of
misrouting a port at run time.

`FdcCfg::port_windows(primary_ata_control)` supplies the FDC's AT decode. Existing
presets pass `false`, preserving `0x3F0–0x3F7`. With a primary ATA control register
at `0x3F6`, callers pass `true`: the FDC gets `0x3F0–0x3F5` and `0x3F7`, with no
overlap flags. The IBM Personal Computer AT Technical Reference, System Board,
**I/O Address Map** lists `0x3F0–0x3F7` for the diskette controller and
`0x1F0–0x1F8` for fixed disk. Its **Fixed Disk and Diskette Drive Adapter** section
identifies the fixed-disk register at `0x3F6` and the diskette registers at
`0x3F2`, `0x3F4`, `0x3F5` and `0x3F7`. The split decode is this model's choice:
it keeps unused `0x3F0`, `0x3F1` and `0x3F3` with the FDC, as the unsplit window
does. This model keeps `0x3F7` entirely with the FDC
(digital input register on read, configuration control register on write).
The real adapter shares read bits there with the fixed disk; that bit-level
sharing is not modelled.

**Validation before assembly.** A `ConfigError` pass runs *before* any device is
built: an absent or incompatible drive controller, an unavailable unit, an IRQ line with no controller, a `LoadImage`
outside the memory map, a decode overlap. Assembly of a validated config does not
fail for configuration reasons.

## Presets

Presets are the ergonomic layer: **plain functions** that return a
`MachineConfig`.

```
fn at_class(params: AtParams) -> MachineConfig { … }
fn serial() -> MachineConfig { … }
```

They are ordinary functions, not a trait and not a builder DSL — a preset just
fills in a `MachineConfig`. Today's `AtChipset` and `SerialChipset` become
presets: the knowledge they encode (which devices, at which ports, with which
wiring) moves into the `DeviceConfig` list a preset produces. A caller wanting a
one-off variation clones a preset's config and edits fields. Added in task-31.

### Canonical BIOS device order

Device order is part of the machine: the assembler preserves `devices` order,
and the bus serves due devices in bus order at each whole PIT boundary. Devices
due at the same boundary therefore run in that order. Decode-map sorting does
not change this scheduling order.

The BIOS preset follows the board model's fixed motherboard order first:
`MasterPic`, `SlavePic`, `Pit`, `Dma`, `DmaWord`, `Keyboard`, `Rtc`,
`Coprocessor`, `SysControl`, `Post`.
Cards follow in file order; the BIOS preset's card order is `Mda`, `Vga`, `Uart`,
`Fdc`, matching [machine-files.md §Canonical order](machine-files.md#canonical-order). The no-floppy
BIOS rig omits both DMA banks and the empty coprocessor socket, and keeps its undecoded, unwired FDC. Other Rust test-rig
presets retain their existing order. The BIOS MMIO windows are sorted by base:
`Mda` at `0xB0000`, then `Vga` at `0xB8000`.

### BIOS monochrome display routing

The `bios` preset, shared by BIOS and FreeDOS runners, includes independent VGA
and MDA text cards. `DeviceKind::Mda(MdaTextCfg {})` constructs `MdaText` at role
`DeviceId::Mda`. Its decoded MMIO aperture is `0xB0000..=0xB7FFF`, removed from
RAM; the card mirrors 4 KiB of physical text RAM across that aperture. Its ports
are `0x3B0..=0x3BB` and `0x3BF`, excluding the unmodelled printer function at
`0x3BC..=0x3BE`. VGA retains its colour ports and `0xB8000..=0xBFFFF` aperture;
monochrome CRTC writes no longer update VGA registers. Other presets retain
their existing device and memory maps.

These ranges follow the IBM PC Technical Reference, Options and Adapters,
Monochrome Display and Printer Adapter, Programming Considerations. The device
module documents the status/open-bus assumptions. Validation applies the same
missing-device, port/MMIO conflict and RAM-exclusion checks as for VGA.

Prompt polling's `ConsoleProbe.device` names a bound display by `DeviceId`,
without a guest-memory address or geometry override. Polling and final capture
both call `Observation::capture_text`, which renders `Device::text_scanout` once
through `video::render_text_mode`, including the CRTC start address and cursor,
then delivers that frame to the route's subscribers. Poll cadence and keyboard
injection stay fixed.

### Output configuration

`OutputConfig { frame_sink: FrameSinkKind }` selects initial presentation wiring
for every text route requested by `ObservationPolicy`. The default `Headless`
installs no subscriber and keeps capture and prompt detection available.
`Capture` installs one `video::CaptureFrameSink` per route; the facade retains
only its last frame, available through `Observation::captured_frame(DeviceId)`.
Selecting Capture without any requested display does not add observation routes.
Inline hosts call public `Observation::configure_output(OutputConfig)` after
`Observation::bind`; reconfiguration clears configured last frames and preserves
external subscribers.

A host presenter attaches with `Observation::subscribe(DeviceId, Box<dyn FrameSink>)`.
The route must already be bound. Subscribers receive the exact owned frame
returned by capture, at existing prompt polls and final captures. They receive
no machine reference and must not block, introduce host timing or call back into
the machine. Assembly depends on the video trait, with no window dependency.
Observation policy owns route selection and byte limits; RunPolicy owns poll
cadence. Presentation never changes execution step bounds. See
[observation.md §Sinks, bounds and lifecycle](observation.md#sinks-bounds-and-lifecycle).

## RunPolicy — "how we run" is not "what the machine is"

`MachineConfig` describes the machine. It does **not** describe the run. Those
are split (implemented in task-32, in the `assembly` crate next to the assembler):

```
RunPolicy {
    retired_cap: Option<u64>,          // safety bound on virtual time (retired + halted clocks)
    idle_limit:  Option<u64>,          // halted PIT clocks with no wake-up before stopping
    stop:        StopCondition,        // Natural | PromptThen { console, needle, poll_step, then_budget }
    stimuli:     Vec<KeyboardStimulus>,// scancodes + WHEN to inject (InjectionSchedule)
}
```

The stop *shape* is an enum, not a `bool`: `StopCondition::Natural` runs to a
halt/quiescence (the POST boots and demos); `StopCondition::PromptThen` folds the
old `boot_to_date_prompt` loop — poll the `console` in `poll_step` retired-instruction
chunks until `needle` shows, then, if stimuli are scheduled, inject them and run
`then_budget` more. Scancode injection is a `KeyboardStimulus` (bytes + an
`InjectionSchedule`, today `AfterPrompt`); it is the **mechanism** the interactive
FreeDOS port (task-36) drives — task-32 provides it, task-36 is the caller.

`idle_limit` is a run-policy concern: assembly starts without an idle wait and
`run(machine, observation, policy)` applies it before executing. The active cap counts steps,
including REP iterations, plus accumulated halted instruction equivalents. The
cap is exact during execution; halted caps round up to a whole PIT clock.

**The injection cadence is part of the golden identity.** *When* scancodes are
injected and how often the loop polls for a pending IRQ is not a free tuning
knob: it changes which interrupts are taken and therefore the observable output.
`RunPolicy` derives `Eq`/`Debug` for exactly this reason. A golden trace is valid
only against a fixed `RunPolicy` cadence, exactly as it is valid only against a
pinned CPU revision. Introduced in task-32.

## Migration strategy

The old hand-written assembly paths and the new config path **coexist** until the
final deletion. Nothing is deleted until the config path is proven to reproduce
the old path byte-for-byte.

**A/B equality per ported runner.** Every runner ported to the config path is
guarded by an **A/B test that asserts full-result equality** — the *entire*
return value, compared by the runner's own return type (`BiosRun`,
`Test386Run`, `Vec<u8>`). `BiosRun` has no `PartialEq` today, so the comparison
goes through a **shared A/B harness** (task-35) that compares every observable
field (POST bytes, VGA render, serial stream, stop reason). "Equal" means the
new path is indistinguishable from the old, not merely "looks similar."

**Porting order — safest first, scariest last.**

1. `run_test386` and `run_floppy_boot` first — narrowest surface.
2. The PC-family boot runners next.
3. `run_freedos_interactive` **last** — it has the finest interrupt cadence and
   the most fragile keyboard-injection timing, so it is ported only once the
   harness and the easier runners have shaken the config path out (task-32 /
   task-36).

**The deletion gate (task-33).** Removing the `Chipset` trait and the ad-hoc
assembly is a **deletion-only commit**. It is gated on **all A/B tests being
green on the pre-deletion commit** — i.e. the commit *before* the deletion must
have every runner passing A/B equality. The A/B tests are then deleted *together
with* the old paths they compared against (they have no meaning once one side is
gone). task-33 lands the decision-7 annex verification and removes the trait; it
does not itself port anything.

## The demos and `run_serial`

Resolved here so the trait removal (task-33) is unblocked:

- **The four Real16 demos** — `run_boot`, `run_keyboard`, `run_rtc`,
  `run_vga_text` — are **ported to the config path in task-31**. They are small
  and the AT-class config already covers everything they need; there is no reason
  to keep them as hand-written assembly.
- **`run_serial` and its Long64 golden are intentionally removed.** The Long64
  walking skeleton has served its purpose. `HELLO_PROGRAM`, flat Long64 blob
  loading and the default CLI demo are retired with the legacy machine loop.
  `--boot` remains the Real16 timer demo; no arguments print usage and point at
  DOS goldens. The CPU adapter retains its own Long64 unit tests. Machine config
  has no CPU-mode axis and this removal is not a regression to restore.

## Non-goals

Deliberately out of scope. Do not implement these unprompted.

- **No MHz / frequency axis.** Virtual time is retired instructions
  (decision-5); a nominal "MHz" measures nothing in this model. `ClockConfig`
  carries a *ratio*, not a frequency. A frequency axis arrives only if a consumer
  needs it *and* there is an oracle to validate it against.
- **No behavioural CPU-type toggle.** There is no oracle for an 8086/8088
  behavioural mode, and the CPU core models one part. `CpuConfig.chip` is a
  marker with one variant today; a second *behavioural* variant waits for a
  consumer and a differential oracle.
- **No CPU-mode axis in the config.** `run_serial`'s Long64 case was intentionally
  deleted, without adding a mode dimension to `MachineConfig` (see
  above).
- **Any deleted demo.** `run_serial` (and its golden) was deleted;
  that removal is intentional and recorded — not a regression to
  restore.
- **No config file format / serialization.** `MachineConfig` is Rust data. A
  textual/serialized machine description is a later concern; the `Eq`/`Debug`
  derives are for goldens and future snapshots, not a file format.
- **No protected mode / 386, no cycle-accurate timing.** Unchanged from the
  project's standing scope (§1.3, [`deferred.md`](../deferred.md)).
- **Snapshot / save-restore is deferred, not designed here.** The config is made
  identifiable (`Eq`/`Debug`) so a future snapshot feature has a stable machine
  identity to hang off — but building it is deferred
  ([`deferred.md`](../deferred.md)).

## Milestone map

The layer is built across task-28 … task-37. Dependencies mirror the backlog.

| Task | What | Depends on |
|------|------|------------|
| **task-28** | This design doc + decision-7 annex (the Chipset-trait replacement) + deferred.md snapshot entry. Docs only. | — |
| **task-29** | Config data model + `ConfigError` validation, in a new assembly crate. | task-28 |
| **task-30** | The assembler: `MachineConfig -> Machine` (+ `DeviceId -> index` map), in the assembly crate. | task-29 |
| **task-31** | Presets + port the PC-family `run_*` (and the four Real16 demos) to config, under A/B equality. | task-30 |
| **task-32** | `RunPolicy` split + port `run_freedos_interactive` (highest risk). | task-31 |
| **task-33** | Remove the `Chipset` trait, delete the ad-hoc assembly, verify the decision-7 annex against final code. Deletion-only, gated on all A/B green. | task-36 |
| **task-34** | `DriveConfig` + floppy geometries — the seam under future IDE/HDD. | task-30 |
| **task-35** | Shared A/B result-equality test harness (gives `BiosRun` a comparison). | task-30 |
| **task-36** | Port `run_freedos_interactive` to config + `RunPolicy` (the scary one). | task-32 |
| **task-37** | `DriveConfig` wiring + CMOS derived from attached drives. | task-33 |
