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
    cpu:     CpuConfig,
    clock:   ClockConfig,
    memory:  MemoryConfig,
    devices: Vec<DeviceConfig>,   // a flat list, produced by preset functions
    drives:  Vec<DriveConfig>,    // the ONE home for attached media
    images:  Vec<LoadImage>,      // the ONE home for pre-loaded bytes
    output:  OutputConfig,        // initial presentation subscribers
}
```

`MachineConfig` and every field type derive `Eq` and `Debug`. The config is
**identifiable**: two configs are equal iff they describe the same machine. That
is what lets a golden test pin the machine it ran against, and it is the
precondition for a future snapshot/save-restore feature (deferred — see
[`deferred.md`](../deferred.md)).

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
- **The option-ROM scan area is writable RAM, not ROM.** The region the BIOS
  scans for option ROMs is *zeroed, writable RAM* in this model, not a
  write-protected ROM region. A semantic `MemoryConfig` distinguishes
  `Ram` / `Rom` / MMIO-trap regions so this is stated in data; a byte-count
  model would silently get the region's write-protection wrong.

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

### Single homes (no triplication)

Two data-model decisions exist specifically to prevent the same information
living in three places:

- **`images`: one `LoadImage` list, not three.** Firmware ROM, a `memory.rom`
  blob, and a CPU code blob were three separate concepts. They collapse to one
  list of `LoadImage { base, bytes, kind: Rom | RamLoad }`. `kind` says whether
  the bytes land in a ROM region or are loaded into RAM (SMC-visible per
  decision-6). One list, one meaning.
- **`drives`: one top-level list, routed by slot.** Attached media were split
  between an ad-hoc floppy argument and a controller's own drive list. They
  collapse to one top-level `Vec<DriveConfig>`, each routed to its controller by
  a `controller_slot` field. The floppy controller does not own a private drive
  list; drives are attached to the machine and routed to controllers.

`DriveConfig` and floppy geometry are fleshed out in task-34; the top-level
placement and slot routing are fixed here.

### The fixed-disk (IDE/HDD) attachment seam

`DriveKind` is `Floppy { geometry } | HardDisk { geometry }`. The floppy side is
wired end-to-end: the assembler routes each `Floppy` drive into the µPD765's
drive slots by `controller_slot`, and the CMOS `floppy_types` nibbles and
equipment byte are *derived* from the attached floppies, so attaching a second
drive of a different geometry needs no manual CMOS edit — the whole point of the
"attach different drive types" story.

The `HardDisk` variant is deliberately the **seam a future XT-IDE/ATA device
plugs into with no new assembler architecture**. A `HardDisk` drive is already
first-class config: it is validated, it binds to a controller by
`controller_slot`, and the CMOS `hard_disk_types` nibble is already derived from
it. What is missing is only the *device*: the assembler skips `HardDisk` during
device construction because no fixed-disk controller is modelled yet. When one is
added, it reuses the exact floppy pattern — a `build_ide_drives(config)` that
filters `drives` for `HardDisk` and routes them into the new device by
`controller_slot`, called from the `DeviceKind` match arm just as the µPD765's
`build_fdc_drives` is called today. The drive list, the slot routing, and the
CMOS derivation are all in place; adding fixed-disk support is a new `Device`, not
a new config shape. The IDE/ATA device itself is deferred (see
[`deferred.md`](../deferred.md)).

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

**CMOS is derived, never free-configured.** The BIOS reads floppy types, the
equipment byte, and base/extended memory size from CMOS NVRAM and expects them to
be *consistent with the actual machine* (§7.6). If CMOS is configured
independently of the memory map and drives, the BIOS sees a self-inconsistent
machine and drops to F1 setup. So the assembler **derives** the CMOS seed
(`floppy_types`, `equipment`, `base_memory_kb`, `extended_memory_kb`) from
`MemoryConfig` and the attached `drives`. CMOS is an output of assembly, not an
input. (The device already supports this seed via `CmosConfig`; what changes is
*who computes it* — the assembler, from the machine description, not the caller
by hand.) Wired up in task-37.

**Explicit decode-conflict policy.** Port/MMIO decode is **first-match order**,
and that order is *semantics*, not luck. The canonical example: `0x80` is the
POST diagnostic latch while `0x81` is a DMA page register — adjacent ports owned
by different devices, where a sloppy range would swallow the wrong one. The
assembler therefore:

1. Sorts the decode entries **deterministically** (so build order cannot change
   which device wins a lookup), and
2. **Errors on any overlap** between two devices' ranges (a genuine conflict is a
   config bug, not something to resolve silently by first-match).

Deterministic sort + hard error on overlap means the decode map is a pure
function of the config, and an accidental collision fails loudly at assembly time
instead of misrouting a port at run time.

**Validation before assembly.** A `ConfigError` pass runs *before* any device is
built: unknown `controller_slot`, an IRQ line with no controller, a `LoadImage`
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
