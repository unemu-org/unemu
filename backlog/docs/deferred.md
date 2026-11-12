# Deferred

Things **deliberately not implemented**. If something seems missing and is listed here, don't add it unprompted — there's a reason. Each entry: what, why deferred, when to revisit.

## Format

```markdown
### <Feature / behavior>

- **Why deferred:** <one paragraph>
- **Revisit when:** <trigger condition>
- **Tracked in:** <task-N, if any>
```

A20 gating is implemented: the board's 8042/port 92h line drives CPU physical
address bit 20 and survives CPU-only reset. See
[the board contract](design/machine-config.md#a20-line-and-system-control-port-a).

## Entries

### Open AT-class (80286) system BIOS

- **Why deferred:** No usable open 286 AT BIOS was identified in the candidate
  survey. Legacy Bochs `rombios` was rejected; a proper port is **ASSUMED** to
  take about 2–4 weeks for CPU/runtime, video, A20/protected-mode services and
  boot qualification. Keep Kiselev with an explicit display switch for open
  goldens and measure AT fidelity through the user-firmware lane; see
  [open AT BIOS candidates](design/firmware-and-machine-classes.md#24-open-at-bios-candidates).
- **Revisit when:** Goldens need AT-faithful firmware behaviour that Kiselev
  cannot provide, or a maintainer asks.
- **Tracked in:** upc-6le.37.

### Protected mode, paging (386+), APIC, PCI

- **Why deferred:** Target is IBM PC/AT-class (80286). The 286 has no paging;
  protected mode is not needed for real-mode boot. APIC/PCI are 386+/later-era.
- **Revisit when:** Phase B (protected mode, paging) and phase C (PCI) of
  decision-16, which chose the later machine generations. APIC stays out: the
  target span runs on the 8259 pair.
- **Tracked in:** decision-1, decision-16; epics `protected-mode` and `win9x-base`.

### `RegionKind::Rom` in x86jit

- **Why deferred:** BIOS stub can load into `Ram` by convention initially.
- **Revisit when:** Faithful ROM write-protection matters (real BIOS blob).
- **Tracked in:** decision-9.

### Graphics beyond serial (VGA text, framebuffer)

- **Why deferred:** First E2E target is headless serial (deterministic goldens).
- **Revisit when:** VGA text landed (task-5). A window is phase A of
  decision-16; a pixel framebuffer is phase C.
- **Tracked in:** task-5; epics `skeleton` and `win9x-base`.

### Real-mode segment-register loads (`mov Sreg, r/m`) and far transfers

- **Why deferred:** x86jit's real mode does not yet lift `mov Sreg, r16`/`pop Sreg`
  or far `jmp`/`call`/`retf` (CS reloads other than `INT`/`IRET`). The emulator's
  boot programs work around this: the machine seeds CS/DS/ES/SS before running.
- **Revisit when:** running a real BIOS/DOS/bootloader, which loads segment
  registers itself — needs these lifted in x86jit (a further real-mode extension).
- **Tracked in:** decision-3 (real-mode scope), task-7 (test386.asm / real BIOS).

### Snapshot / save-restore of a running machine

- **Why deferred:** The machine-config layer makes `MachineConfig` identifiable
  (`Eq`/`Debug`) precisely so a future snapshot feature has a stable machine
  identity to hang off, and the `Device` contract carries `save`/`restore` from
  day one (decision-7). But wiring up whole-machine snapshot/replay is a
  standalone feature, not part of building the config layer — designing it now
  would pre-empt work with no current consumer.
- **Revisit when:** A replay/debug or golden-snapshot workflow needs to freeze
  and restore a running machine.
- **Tracked in:** decision-7, [`design/machine-config.md`](design/machine-config.md).

### Retired Long64 machine skeleton

- **Why removed:** Adoption of instruction-boundary timing intentionally deletes
  `run_serial`, `HELLO_PROGRAM`, the Long64 CLI blob mode/default demo, and
  `golden_serial.rs`. The machine runs step-bounded Real16 CPUs only; the CPU
  adapter's own Long64 unit tests remain. This is not a regression to restore
  and does not justify a CPU-mode axis in `MachineConfig`.
- **Revisit when:** An independently justified machine target requires another
  CPU mode and provides an execution/timing oracle. Long64 is outside the
  current PC/AT-to-Windows-98 goal line.
- **Design:** [`design/machine-config.md`](design/machine-config.md), Non-goals.

### Fixed-disk (XT-IDE / ATA) controller device

- **Why deferred:** The config side of fixed disks is done — `DriveKind::HardDisk`
  is first-class `MachineConfig` data with a named `controller: DeviceId`, a
  controller-local `unit`, and `write: WriteMode`. Validation rejects a hard
  disk until a compatible controller is implemented; an FDC cannot serve it.
  CMOS `hard_disk_types` derivation retains the fixed-disk attachment seam.
  What is missing is the *device* that serves it. The
  first end-to-end target is headless serial and floppy boot; a fixed-disk
  controller has no current consumer, and modelling ATA register semantics is a
  standalone device task, not part of the config layer. The seam is built so
  adding one is a new `Device` plus a `build_ide_drives`-style router, with no new
  assembler architecture (see
  [`design/machine-config.md`](design/machine-config.md), the fixed-disk seam).
- **Revisit when:** A boot flow or guest needs to read or write a hard disk (e.g.
  installing DOS to an HDD image rather than booting a floppy).
- **Tracked in:** [`design/machine-config.md`](design/machine-config.md).

### Real (unmodified) BIOS

- **Why deferred:** Start from a minimal BIOS stub; x86jit models a modern CPU, so
  286-detection quirks would break a period BIOS.
- **Revisit when:** A real BIOS blob is a goal.
- **Tracked in:** decision-10.

### Offline floppy journal tools and retention

- **Implemented:** Explicit journal mode persists floppy writes as command-complete
  sector batches beside an immutable file-backed base; new sessions replay the
  journal. Volatile remains the default, and readonly rejects guest writes.
  Offline `media generations`, `verify`, explicit backed-up tail repair and
  rollback and immutable raw-image export (`media commit`/`compact`) are available
  with shared/exclusive stable locking. Retention atomically replaces a verified
  anchor plus retained history at closed boundaries; the positive count includes
  the anchor. Failure isolation, complete volatile/journal result comparisons,
  rollback through guest DMA and volatile fixture hash/namespace protection have
  executable acceptance evidence in the persistence design appendix. TOML/YAML
  machine-file floppy drive tables accept `write = "journal"` with an explicit
  journal path, alongside Rust `MachineConfig` selection.
- **Still deferred:** Frontend journal-selection flags. The CLI and window
  frontends do not load machine files yet; that integration is in progress.
  The runnable walkthrough uses Rust configuration. Physical power-cut validation,
  Linux storage measurement in this session and support beyond the local Unix
  storage contract remain unmeasured; see the design's Supported host guarantees
  section.
- **Revisit when:** Frontend selection or another host platform is needed.
- **Tracked in:** [`design/persistent-media.md`](design/persistent-media.md),
  §Offline commands and §Retention anchor.

### Layered disk images (base image plus content-addressed layers)

Block-level copy-on-write layers would store only changed sectors and identify
their parent by content hash. A base image, such as a user-supplied Windows 98 SE
installation, plus layers of installed programs would form a linear chain.
Layers cannot be freely combined: two layers built separately on one base both
change the FAT and the Windows registry, so combining them means building one
on top of the other. A discardable per-session top layer would make every run
start from the same disk state; committing it would create a new layer.

Optional recipe layers would replay a deterministic input script (keys, mouse
and media changes at virtual times) on the parent to reproduce the layer byte
for byte. Results could be cached by `(parent hash, recipe hash, CPU core revision)`.
Proprietary bases and layers would be user-supplied and identified by hash only,
never committed, like user firmware. A saved machine state could also be a layer,
but would be tied to the emulator version; disk layers would remain plain data.

- **Why deferred:** No workload needs layered images before Windows 98 SE runs
  in phase C. The floppy journal and the coming hard-disk write-back cover
  persistence needs in the meantime.
- **Revisit when:** The hard-disk write-back is designed, and again when phase C
  needs a reproducible Windows 98 SE disk state for golden tests.
- **Requirement for now:** The write-back format must not preclude layers: keep
  the base read-only and identified by hash, and keep written sectors separate
  from the base. The floppy design already preserves this separation; see
  [`design/persistent-media.md`](design/persistent-media.md), §Context and
  §Header: exactly 128 bytes, and
  [`design/machine-files.md`](design/machine-files.md), §Drives.
- **Tracked in:** upc-18x.5.
