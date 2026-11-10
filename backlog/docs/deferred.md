# Deferred

Things **deliberately not implemented**. If something seems missing and is listed here, don't add it unprompted — there's a reason. Each entry: what, why deferred, when to revisit.

## Format

```markdown
### <Feature / behavior>

- **Why deferred:** <one paragraph>
- **Revisit when:** <trigger condition>
- **Tracked in:** <task-N, if any>
```

## Entries

### Protected mode, paging (386+), APIC, PCI

- **Why deferred:** Target is IBM PC/AT-class (80286). The 286 has no paging;
  protected mode is not needed for real-mode boot. APIC/PCI are 386+/later-era.
- **Revisit when:** Phase B (protected mode, paging) and phase C (PCI) of
  decision-16, which chose the later machine generations. APIC stays out: the
  target span runs on the 8259 pair.
- **Tracked in:** decision-1, decision-16; epics `protected-mode` and `win9x-base`.

### Full A20 gate (via 8042)

- **Why deferred:** A20 modelled as always-on for M1–M3 to unblock early boot.
- **Revisit when:** A boot flow requires A20 toggling — protected-mode code
  does (decision-16, phase B).
- **Tracked in:** decision-8; the A20 bead in epic `protected-mode`.

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
  rollback are available with shared/exclusive stable locking.
- **Still deferred:** Immutable raw-image export (`media commit`/`compact`)
  and atomic retention pruning with anchors. The positive retention count is
  validated but does not prune yet.
  Serialized journal configuration and frontend selection flags are also later
  work; current selection uses Rust `MachineConfig` data.
- **Revisit when:** A guest session needs image export or automatic retention rather
  than append/replay/checkpoint/finish and offline inspection/rollback.
- **Tracked in:** [`design/persistent-media.md`](design/persistent-media.md),
  §Offline commands and §Retention anchor.
