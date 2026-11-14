# Architecture

This describes the implementation on `main`: a deterministic, headless IBM
PC/AT-class emulator with a real-mode CPU, device models, declarative assembly,
and BIOS/FreeDOS integration tests. Testability, swappability, and documented
hardware coverage guide the crate boundaries. Corrected timing and deadline
dispatch are adopted; design documents distinguish implementation from proposals.
The hardware specification is [`design/spec.md`](design/spec.md).

## Crate map and dependencies

These are the direct dependencies from each crate's `Cargo.toml`; test-only
dependencies are separate. Workspace membership and the exact `x86jit-core` git
revision are in the root [`Cargo.toml`](../../Cargo.toml). There is no firmware
crate: `machine-file` reads and hashes declared images into inline buffers,
`assembly` also supports direct Rust image sources, and guest assets live under `vendor/`.

| Crate | Responsibility | Runtime dependencies | Test-only dependencies |
| --- | --- | --- | --- |
| `image-store` | Platform-neutral OCI Image Layout, streaming SHA-256 blobs, deterministic machine manifests, provider-validated closure and atomic compare-and-swap local tags | `serde`, `serde_json`, `toml`, `sha2`, `thiserror`, `libc` (Unix) | None |
| `media` | Floppy journal encoding, streaming recovery/replay, retention-anchor validation, durable host sink and stable single-writer locks ([format](design/persistent-media.md#chosen-format-byte-layout)) | `sha2`, `thiserror`, `libc` (Unix) | None |
| `device` | Hardware-neutral contracts: `Device`, `InterruptController`, `DmaMemory`, `VClock`, `NextEvent`, access traces, DMA messages, and the observation capabilities (text scanout, byte output, `ByteCapture`) | `thiserror` | None |
| `cpu` | `Cpu`/`CpuStop` seam and `X86JitCpu`, which owns the core's `Vm` and `Vcpu` | `x86jit-core`, `thiserror` | None |
| `bus` | Owns boxed devices and executes a port/MMIO `DecodeMap`; open-bus policy and ticking | `device`, `thiserror` | None |
| `devices` | UART 16550, PIC 8259, PIT 8253, 8042, CMOS/RTC, DMA 8237, uPD765 floppy controller, VGA and MDA text cards, system control, POST port and Lo-tech EMS board | `device`, `thiserror` | `testkit`, `video` |
| `chipset` | Shared/cascaded PIC composition, null interrupt controller, AT port constants; no `Chipset` trait | `device`, `devices` | `testkit` |
| `machine` | Generic `Machine<C: Cpu>` and execution/clock/IRQ/DMA loop | `cpu`, `bus`, `device`, `thiserror` | `testkit`, `devices` |
| `video` | `FrameSink`, text frames/grids, text-mode renderer and capture sink | `device` | None |
| `assembly` | `MachineConfig`, validation, presets, image/device construction, wiring, and `RunPolicy` driver | `machine`, `cpu`, `bus`, `device`, `devices`, `video`, `chipset`, `media`, `sha2`, `thiserror` | `media/test-support` |
| `machine-file` | Schema 1 TOML/YAML parsing, source origins, image hashing and lowering through assembly validation; merging remains deferred ([schema](design/machine-files.md#schema-field-by-field)) | `assembly`, `serde`, `sha2`, `toml`, `serde-saphyr`, `thiserror` | None |
| `window` | CP437 rasteriser, shared `FrameSink`, and egui desktop shell | `video`, `device`, `assembly`, `machine`, `winit`, `egui`, `eframe` | `cpu`, `assembly/test-support` |
| `testkit` | Single-device `MockBus`, scripted `MockCpu`, golden assertions | `device`, `cpu` | None |
| `cli` | Binary and runner library: guest programs, assembly/policy selection, output capture and stop diagnostics | `assembly`, `device`, `bus`, `cpu`, `chipset`, `machine`, `devices`, `video`, `anyhow` | `testkit`, `machine/reference-scheduler`, `assembly/reference-scheduler`, `assembly/test-support` |

Concrete hardware selection lives in `assembly` and the frontend. `machine`
does not depend on `chipset`, `devices`, `video`, or x86jit directly. `Bus` is a
concrete router over `dyn Device`, not a trait; the config-derived decode map is
its routing authority. Concrete devices do not depend on the runtime bus.

`image-store` takes a caller-selected local root and owns `store.toml`, the
OCI layout under `images/`, immutable blobs, staging files and stable advisory
locks. It has no emulator dependencies. `ConfigValidator` providers validate
platform-owned config schemas and enumerate their complete ordered payload
closure, including transitive disk references when supported. The store checks
routing-platform agreement, metadata/artwork references, descriptor limits,
exact closure order and every blob hash before publishing a tag. Providers
declare extra payload media types and size limits; shared code has no platform
payload allowlist. Resolution reports missing artwork without blocking required
payloads, while publication still requires the full artwork closure. Foreign OCI
index entries and optional fields survive local tag updates. Canonical
manifest bytes identify the image; config bytes independently identify the
machine. Reads never fall back to source paths. See
[machine images](design/machine-images.md), §§1–3 and §5.

Blob ingestion streams through a fixed 64 KiB buffer. Linux/macOS no-clobber
rename publishes synced blobs without replacing existing inodes; tag updates
reread the index under a short lock and atomically rewrite it after a digest
compare-and-swap. Stable kernel locks release on process exit, so leftover lock
files are reused. Locality checks and a lock probe reject unsupported roots;
Linux currently accepts ext-family, tmpfs, Btrfs, XFS, ZFS, F2FS and overlayfs;
opening also probes no-clobber publication. macOS uses
the filesystem's local flag, and other hosts are refused. This crate does not
implement machine-file import, providers, disk reconstruction, workspaces,
registry transfer, quarantine/damage tracking or GC. Future GC must take the
exclusive store lease before deleting any objects.

## From configuration to a run

`machine_file::load` reads a complete TOML or YAML machine description, expands the
assembly-owned board data and card defaults, resolves images against their
defining files, checks optional SHA-256 pins, and retains the bytes inline.
It derives the RAM mosaic and calls `MachineConfig::validate()` before returning.
The reference file is `machines/at-freedos.toml`; frontends still use Rust presets.

`assembly::descriptors` owns the implemented card names, video decodes, RAM
and resource domains, effective defaults, named floppy geometries and firmware
slots. EMS domains reference the device model's public jumper/population
constants, so the runtime constructor and configuration cannot drift.
Machine-file parsing/lowering consumes these definitions. The
transport-neutral `assembly::capabilities` API describes fields, pages large
integer domains, offers choices and validates a lowered `Draft` with atomic
pending edits. Paths use card identities (`card[sound].irq`) and drive units,
with optional source-level card names and image-field paths supplied by the
caller. Image paths exclude the system BIOS when numbering option ROMs.
Port descriptors expose IRQ dependencies; edits follow a changed port's derived
IRQ default when the old default is still selected, preserving explicit IRQ edits.
Choices retain unrelated existing errors as diagnostics without disabling other
fields, so staged resource swaps remain editable. Responses echo
the caller's revision so a frontend can discard stale choices.

`validate_draft` shares the ordered checks behind `MachineConfig::validate`,
collecting one error per independent check. Diagnostics include stable codes,
field paths, conflict partners, decoded ports/routed IRQs/DMA ownership and
severity. Candidate edits replace existing resources and rebuild derived
port/MMIO/RAM maps; several fields are applied before checking conflicts.
Selected firmware uses catalogue requirements and sizes for placement checks,
while absent bytes remain pending. The known June 1985 IBM BIOS drive-type
restriction is exposed to drafts; legacy lowering's first-error behavior and
source-origin mapping remain unchanged. No host image reads, UI types or
execution clocks enter the API. Launch must still load/verify inputs and run
assembly validation, including persistent-path checks. `Validation::ready`
means no reported error or pending check, not permission to bypass assembly.

The normal real-mode path is:

```text
MachineConfig (preset or caller data)
  -> assemble(&config)
  -> AssembledMachine { Machine<X86JitCpu>, device_index, observation }
  -> RunPolicy + assembly::run(&mut machine, &observation, &policy)
  -> Machine::run_bounded(...) -> output capture / stop classification
```

[`MachineConfig`](../../crates/assembly/src/machine.rs) describes CPU/reset state,
clock rate, RAM/MMIO layout, devices, drives and loadable images.
[`assemble()`](../../crates/assembly/src/assemble.rs) validates
it before construction, maps CPU memory, loads images, creates devices in config
order, derives CMOS contents from memory/drives, and resolves device roles into
bus indices. It builds the decode map, IRQ lines and DMA couplings. One shared
master/slave PIC pair backs both bus faces and the interrupt-controller seam;
a configuration without a PIC receives a null controller.

[`RunPolicy`](../../crates/assembly/src/run.rs) describes how to drive that machine:
virtual-time cap, halted idle limit, natural stop or console-prompt polling, and
keyboard stimuli after the prompt. `run()` applies the idle limit and drives
`run_bounded`; prompt polling captures a bound `DeviceId` display route through
`ConsoleProbe` and the observation facade. Policy cadence affects guest execution and golden identity.
The assembled machine starts without an idle wait. Some simple runners call
`Machine::run()` directly. The assembler constructs Real16 machines and rejects a CPU without step-bounded
execution. The Long64 machine demo has been intentionally removed.

### Current execution order

The authoritative loop is
[`Machine::run_bounded`](../../crates/machine/src/lib.rs). For a running CPU:

1. Settle instruction effects, check the absolute virtual-time cap, then
   acknowledge a serviceable PIC request only if `Cpu::irq_ready()` permits it.
2. Bound `Cpu::run_steps` by the next device deadline, remaining cap and host
   cooperation limit. A pending INTR requests a stop at the first eligible
   boundary, respecting IF and STI/SS-load inhibition.
3. Charge completed prefix steps, dispatch the stopping port/MMIO access at
   instruction-start time, and finish pending read/write handshakes. Each REP
   iteration is a step, so long string instructions advance device time.
4. Charge the completed stopping step once, settle bus/DMA effects and sample
   wired IRQ transitions before the next acceptance check. Errors retain the
   completed prefix's device time. Exact active caps win over a new INTA.

Production uses deadline dispatch. Devices without a deadline implementation
retain per-clock visits, while scheduled devices synchronize lazily on access.
The non-default `reference-scheduler` feature exposes per-clock dispatch to
integration tests as an oracle with the same CPU/IRQ rules. No production
`MachineConfig` selects it, and no legacy outer loop remains.

[`Bus`](../../crates/bus/src/lib.rs) dispatches port cycles according to each
window's `PortWidth`: `Byte` by default, or `Word` for IOCS16# devices. An even
port in a word window receives one size-2 call for a word access. Width is
selected by the cycle's starting port, so a single-port word window consumes
both data bytes even if the next port decodes to a byte device. Odd word accesses
use two byte cycles. An aligned 32-bit access to word windows uses two size-2 cycles at
port and port+2; each new cycle independently selects its window and width.
Mixed windows therefore use byte or word cycles as selected, low bytes first,
with port arithmetic wrapping at 16 bits. A fault preserves completed cycles.
Device traces record those actual cycle sizes. This models the IBM Personal
Computer AT Technical Reference, Section 1, System Board, I/O Channel,
I/O Channel Signal Description, -I/O CS16 (IOCS16#); wait states and physical
byte-lane signalling remain outside the model. Existing presets retain byte
windows. Assembly checks word windows against a kind-based register support
table, currently empty; ATA card wiring will declare its word data register.
Port dispatch resolves the device and cycle width in one first-match lookup.
See [Port window width](design/machine-config.md#port-window-width).
Undecoded ports default to
all-ones reads/dropped writes; optional bounded tracing and strict faulting are
selectable. Assembly coalesces touching MMIO windows into one CPU trap region
while retaining per-device bus decoding. MMIO accesses contained in one decode
window reach the device once with their original size. Accesses crossing windows
are split into contiguous sub-accesses in ascending address order, with
little-endian read composition and write slicing. An undecoded MMIO address
faults immediately: earlier sub-accesses remain completed and later ones are not
attempted. Read results use `complete_port_in` or
`complete_mmio_read`; trapped writes require `complete_mmio_write` so retrying
the instruction does not repeat the device side effect.

While halted, the loop stops immediately with IF clear (NMI is not modelled).
With IF set it first tries IRQ service; otherwise, subject to idle/cap bounds,
it jumps to the next device event, idle expiry or cap, settles DMA and samples
edges before retrying IRQ acceptance. A queued IRQ permits CPU re-entry. Halt state and accumulated idle time
survive bounded runner calls.

## Contracts and ownership

- **[`Cpu`](../../crates/cpu/src/lib.rs)** exposes `run_steps`, completed-step
  and whole-instruction counters, general-purpose registers/RIP, port/MMIO
  completions, IF and IRQ readiness, IRQ injection/pending status and DMA byte
  access. It has no segment-register API,
  bus-binding API or `irq_ack()` method. Segment seeds and `code_segment()` are
  concrete `X86JitCpu` setup/inspection APIs.
- **[`Device`](../../crates/device/src/lib.rs)** supplies port/MMIO access,
  `tick(VClock)`, IRQ level queries, reset, per-device save/restore, trace
  drains and optional DMA request/service/completion hooks. Host-side
  `as_any_mut` is an optional downcast for input/output glue, not execution-loop
  hardware selection. `NextEvent::{EveryPitClock, At(VClock), Dormant}` is an
  scheduling contract consulted by the production deadline loop; `next_event`
  defaults to `EveryPitClock` for unmigrated devices. Timestamped access wrappers
  synchronize the target device at instruction start; deadline dispatch settles
  due transitions without ticking dormant devices at each CPU boundary.
- **[`SectorMedia`](../../crates/device/src/media.rs)** is owned by each uPD765
  drive and covers its configured geometry. It serves 512-byte sectors, accepts
  one ordered batch of replacements per nonempty completed WRITE DATA, and
  reports write protection that remains constant while mounted. Partial sectors
  are zero-filled by the controller; abnormal completions include sectors already
  committed. The contract exposes no clock or host error result. Default drives
  use `InMemoryMedia`; reset retains the medium and device snapshots exclude it.
  The host-side `media::JournalMedia` owns reconstructed bytes, synchronous
  per-command output, recovery and checkpoint/finish under a stable sidecar lock.
  Its shared `SinkStatus` latches the first failure outside guest-visible state;
  reads and later memory writes continue after output stops. Assembly adapts
  journals to `SectorMedia` and exposes per-drive persistence handles. CLI and
  window frontends poll status at run boundaries and explicitly finish normal
  sessions; execution errors leave the tip unsealed.
  See [persistent media](design/persistent-media.md#integration-layers-flush-points-determinism-argument).
- **`InterruptController`**, defined in `device`, supplies `set_irq_line`,
  non-mutating `poll`, `acknowledge`, and `end_of_interrupt`. Machine wiring
  forwards rising and falling device pin levels to PIC inputs; the PIC detects
  edges and withdraws unacknowledged requests when their inputs fall.
  Acknowledgement requires CPU IRQ readiness and no already queued vector,
  after the stopping instruction's bus/DMA effects have settled. Guest EOI port
  writes reach the same shared PIC state. See the adopted ordering in
  [scheduler design](design/scheduler.md#adoption).
  Known limitation: cascaded-board validation does not reserve master IRQ2 for
  slave INT, so a device route can still conflict with that internal connection.
- **[`FrameSink`](../../crates/video/src/lib.rs)** consumes `&Frame` via
  `present`; the current frame is `Frame::Text(TextGrid)`. Rendering turns VRAM
  and display-start/cursor state into cells with glyph bytes and attributes.
  `CaptureFrameSink` retains the last frame. Presentation is driven by the assembly observation facade at capture
  boundaries, outside the machine loop.

The bus owns devices; `X86JitCpu` owns guest memory. Devices do not retain RAM
references. DMA requesters supply a channel and payload/length, the configured
controller supplies addresses/counts, and the machine lends a temporary
`DmaMemory` adapter over the CPU seam. `DmaOutcome` returns a transfer count or
the bytes read, truncated to the controller's actual transfer length.

### Snapshot clock and interrupt state

Device restore keeps the current trace history, drop counts and recording policy.
A restored bus clock must be at least as late as each device snapshot's absolute
clock; devices ignore earlier sync/tick times to avoid rewinding or replaying an
interval. Restoring an earlier epoch requires restoring its global clock too.

A future machine-level snapshot must also save `Machine::last_levels`, the IRQ
IRQ sampler's last observed levels. No device snapshot contains these samples;
losing them can suppress or invent an interrupt edge after restore. CPU state,
the global clock and its fractional remainder, halted state and idle counters
also belong to that machine-level snapshot.

### Observation and presentation

The optional `window` crate remains a leaf frontend. Its `unemupc98-window`
binary uses egui/eframe 0.37 with the glow renderer and the existing winit 0.30
event loop. One client area switches between the machine library and a single
nearest-filtered guest texture fitted at 4:3. Classic grey controls and a blue
gradient title strip are the default; macOS uses its native title bar without
an additional custom strip. Toolbar icons are painted geometry, independent of
font coverage. View selects modern controls and the system, light or dark theme.
Menus stay inside the window on every platform.

`src/shell_model.rs` owns pure discovery, command enablement, hotkey mapping and
pixel conversion. Discovery includes TOML/YAML files directly in `machines/`
and `machines/local/`, with stable ordering and blank last-run metadata. The
existing machine-file loader and read-only checker supply hardware details and
readiness; missing firmware or invalid media disables Start and exposes the
validator diagnostic. Open accepts a machine-file path. New/settings/media
insertion/import/layer/catalogue dialogs remain placeholders; Locate firmware
and Preferences edit a shared ROM-folder list. Save state and CD-ROM are disabled. The shell does not yet use the image-store library.

`assembly::firmware_discovery` is a host-only API taking explicit search roots.
It locates single ROMs or interleaved chips by full-file SHA-256 and optional
catalogue length, preferring valid declared paths. Its process cache includes
canonical path, size, modification time, Unix change time and file identity;
fresh metadata is checked on hits, and Windows always rehashes. Catalogue pins
supply exact image/chip lengths. Unknown pins scan only 512-byte ROM blocks from
512 bytes through 1 MiB (or an exact declared length up to 1 MiB). Frontend scans
stay inside each canonical root, skip unusable roots/entries with origin-labelled
warnings and keep searching; the required-ROM test lane retains strict failures. The existing user-ROM test entry points are thin wrappers using
the vendor directory and `UNEMUPC98_USER_ROMS_DIR`. No discovery runs while the
guest executes.

`machine_file::load_for_assembly_with_rom_dirs` resolves missing or mismatched
firmware paths in the in-memory resolved document before ordinary lowering.
Part pins and the assembled-image pin are validated with the existing checked
interleaver; option-ROM container prefixes retain their full-file pins. Machine
files and ROM files are never rewritten or copied. The launch loader still
retains verified disk-base descriptors through assembly. CLI run/check accept
repeatable `--rom-dir`; without explicit directories they perform no discovery,
preserving ordinary path/hash diagnostics regardless of the environment.

The shell stores host ROM folders in the platform config directory's
`unemupc98/preferences.toml`. On macOS this is under Library/Application Support;
on Linux it follows XDG_CONFIG_HOME, falling back to ~/.config. Preferences and
Locate firmware share native folder browsing (rfd) and text entry. Saves write and
sync a temporary file in the same directory, rename it, then sync the directory
on Unix. Unknown TOML fields round-trip. Save invalidates welcome readiness;
only the selected machine is checked on a single host worker, with a checking
message meanwhile. Start requests another worker check, then consumes the prepared
in-memory document without scanning on the UI thread. Stale results cannot replace
newer preferences or selections. Launch uses the same roots, also searching the environment
when set. The deterministic configuration and guest core do not store preferences.

`src/logo.rs` embeds the CC0 brand assets, decodes the icon and GIF once at
startup, and uploads reusable welcome/About textures. Its pure animation clock
uses GIF delays and wraps by elapsed UI time; only a visible welcome pane
requests the next frame repaint. Running sessions and hidden/minimized windows
reset the animation epoch. The file-only `reduced_motion` preference defaults
to false and pins the first frame without animation repaints when enabled.

`src/shell.rs` draws the shell and wraps eframe's public winit application.
The wrapper retains physical keyboard positions and ordered raw mouse motion
instead of reconstructing scan codes from text events. The existing
`Presenter`, `HostKeyboard`, `HostMouse`, `HostAudio` and pacing policy own the
machine session. Menus, dialogs and focus loss release held guest keys and
pointer capture; a screen click restores guest focus. The wrapper withholds
keyboard/IME events from egui during guest focus and pointer events during
capture, including when the host can only confine the pointer. The screen is
not an egui keyboard-focus target, and keyboard UI zoom is disabled. Modifier
changes always reach egui so a held Ctrl cannot survive into a dialog.
Queued keys retain their arrival focus, session, repeat and synthetic flags.
Start/Stop and keyboard focus transitions reset shell key state; only keys
whose makes reached the guest can produce forwarded breaks. Modifier makes
wait for a guest chord, keeping host shortcut prefixes out of the guest.
Held keys from before Start/focus gain cannot enter through OS repeats or
synthetic focus events. Only Ctrl+Alt chords remain host shortcuts during guest
focus; F1 and Ctrl+N/O/comma reach the guest. Pointer release and Pause preserve
keyboard focus. Reset queues explicit guest Ctrl/Alt/Delete makes rather than
resetting the board directly. Stop and normal window close finish only a
successful, still-running session. Execution errors remain unsealed; a halt
has already finished. A failed finish drops the session, shows its diagnostic
and retains a nonzero exit status while allowing the window to close. A later
successful Start or an actual clean Stop clears the stale error.

The frontend advances with `Machine::run_bounded` toward bounded virtual-clock
targets, captures the selected display through the observation facade, and
uploads `WindowSink`'s decoded pixels. No sink ticks devices. Only the binary
reads `Instant`: the existing rational pacing policy limits catch-up to 50 ms,
paints on a 60 Hz grid, and discards hidden, suspended, paused and modal time.
One pause predicate combines user pause, modal/menu state, occlusion and
minimization. Hidden logic passes retain focus-loss and key-release bookkeeping;
restoring a minimized window needs no separate occlusion event.
Audio retains its bounded cpal ring, mute and recoverable-error policy.
Neither playback nor UI repaint frequency drives virtual guest time.

```text
MDA/VGA scanout -> observation capture -> WindowSink -> CP437/graphics rasteriser
  -> 0x00RRGGBB pixels -> egui texture (nearest) -> eframe/glow window
```

The original VileR fonts and attribution stay embedded. About and informational
CLI output also carry egui's embedded font credits and original licence texts;
AccessKit is enabled for the shell. Integer scaling first removes the sink's
pixel-doubling factor. Rasterisation and all
input/pacing/session tests remain headless. The status bar shows power,
configured instruction/PIT ratio, EMS and host audio. Drive activity lamps stay
off because the existing observation boundary exposes no activity signal; the
UI labels that limitation. Headless CLI runs do not depend on the shell and
retain their existing behavior. See `design/machine-images.md` §7.

`assemble_with_observation` binds requested routes by `DeviceId` and validates
capabilities before execution. `AssembledMachine::capture_text` captures display
state at a stamped machine boundary; semantic byte sinks collect UART TX and
POST independently of access tracing. `ObservationPolicy.trace` selects Off
(default) or `Ring { capacity }` for every device and the open bus. All recorders
share overwrite-oldest behavior and cumulative drop counters; draining retains
the count. Diagnostics and their policy are excluded from hardware snapshots.
Capture preserves saved device state and
does not consume traces. BIOS runners return the colour and MDA card grids,
COM1/POST streams, final floppy media and classified stop. The BIOS preset routes
`0xB0000..=0xB7FFF` and display ports `0x3B0..=0x3BB` / `0x3BF` to MDA;
printer ports `0x3BC..=0x3BE` remain undecoded. VGA retains only its colour ports,
so the cards have independent CRTC state. Prompt detection selects the MDA route
by `DeviceId` and uses the same facade capture as final capture. The facade
renders once and fans out that frame to subscribers.
`ObservationPolicy.output.frame_sink` selects Headless (the default, no subscriber)
or Capture (one last-frame sink per requested display). Presentation selection
belongs to observation setup, outside the hardware-only `MachineConfig`.
`Observation::subscribe` accepts a boxed `FrameSink` for a bound route, providing
the window presenter seam without a window dependency. `run_vga_text` selects
Capture; presentation stays outside the machine loop and adds no execution steps.

See [`design/observation.md`](design/observation.md) for the route contracts and
historical design inventory.

## Virtual clock

[Decision-5](../decisions/decision-5%20-%20Virtual%20clock%20in%20retired%20instructions.md)
sets retired instructions as the deterministic time source; block counts and
wall time do not define device time. `VClock(u64)` counts virtual ticks;
`device::clock::TICKS_PER_PIT_CLOCK` remains **1**. `VClock::from_pit` converts
PIT input clocks at unit boundaries. The `device` crate owns clock domains
because it already owns the common device-time seam; no additional crate or
runtime dependency is needed.

`device::clock::OSC` is exactly **315/22 MHz**. Named `ClockDomain` constants
cover PIT (OSC/12), CGA horizontal dots (OSC), VGA dots (25.175/28.322 MHz),
UART (1.8432 MHz), RTC (32.768 kHz), OPL3 samples (OSC/288), 48 kHz output,
and AT 12/16 MHz CPU crystals (6/8 MHz 286 after division by two). The CPU
crystals describe hardware frequencies, not the calibrated instruction rate.
A domain stores an exact ratio of virtual ticks per edge. `edges_between(a,b)`
counts edges in `(a,b]`; `edge_at(n)` rounds up once; `next_edge_after(t)` uses
the absolute epoch to retain phase, including when several fast edges share a
tick. Checked edge methods report exhaustion; duration conversion saturates.
Devices keep local reset/programming origins where the existing model anchors them.
Audio integration remains in virtual-tick areas. PIT counters advance by input
edges, while deadlines, UART/keyboard durations and the VGA clock conversion use
domains. The public `VClock` field and existing stateless VGA raster remain
unchanged by this seam (decision-22 §1a–b).
[`ClockConfig`](../../crates/assembly/src/clock.rs) supplies the positive rational
CPU rate A/B (`retired_per_pit_clock` / `retired_per_pit_clock_denominator`), in
completed steps per PIT clock. Assembly passes
the rate into `Machine::with_cpu_rate`; the default is 1/1. Running time emits
`floor((remainder + completed_steps_delta * B * ticks_per_PIT) / A)` virtual ticks and carries the remainder
across CPU exits. This is an instruction-timed approximation, not cycle timing.

[Decision-14](../decisions/decision-14%20-%20Virtual%20time%20advances%20while%20the%20CPU%20is%20halted.md)
extends time through HLT: halted clocks tick devices without retiring CPU
instructions or consuming the running clock remainder. `Machine::virtual_time`
is completed steps plus `floor(total_halted_ticks * A / (B * ticks_per_PIT))`, saturating
at `u64::MAX`. Caps and policy targets use these instruction equivalents;
device clocks use `VClock`. Golden runs depend on the CPU revision, config and
policy cadence. Host cooperation size cannot change timing by contract;
device-free CPU regressions and the per-clock-versus-deadline comparisons hold
the CPU core to it (see [scheduler adoption](design/scheduler.md#adoption)).

## x86jit integration

[`X86JitCpu`](../../crates/cpu/src/x86jit_backend.rs) adapts a pinned external
`x86jit-core`; machines use its Real16 interpreter. Long64 remains an
adapter unit-test concern only. CPU changes are developed upstream and consumed by re-pinning,
not copied into this workspace
([decision-13](../decisions/decision-13%20-%20Consume%20x86jit%20main%20at%20an%20exact%20revision.md)).

- `Exit::PortIo`, `MmioRead`, `MmioWrite`, `Hlt`, `Exception` and
  `BudgetExhausted` map to `CpuStop`. `UnmappedMemory` and `UnknownInstruction`
  become typed CPU errors; `Syscall` is unsupported. Host interruption, code
  space exhaustion and unrecognized exits become `UnexpectedExit` errors.
- The memory mosaic uses `RegionKind::Ram` (RWX), `Trap` (MMIO), and `Rom`
  (RX, executable/readable with guest stores dropped). Host image loading uses
  `Vm::write_bytes`, which can populate ROM. Real-mode `INT n` already vectors
  through the guest IVT; it is not an outstanding embedder implementation.
- The Lo-tech EMS board owns its SRAM behind `Device` and a 64 KiB MMIO
  frame. Port writes update its four latches synchronously before the next CPU
  instruction; normal RAM still uses the existing `Vm` RAM regions. Every
  400h I/O alias is represented in the decode map and conflict validation.
  The IBM 5170's E0000h–FFFFFh system-board ROM decode rejects an E000h
  frame; the shipped profiles use D000h. Only Rust bare test rigs accept E000h.
  The normal high ROM alias is unchanged. CPU reset preserves latches and SRAM.
  The pinned CPU cannot fetch instructions from trap regions. A code fetch
  from the frame raises a CPU fetch fault and stops the run with a CPU error;
  this limitation is tracked as open work (upc-6le.53).
  See the coverage matrix in `crates/devices/src/lotech_ems.rs` and
  [machine fields](../../machines/README.md#lo-tech-ems-card-reference).
- `inject_irq` forwards a queued vector; the core gates delivery on IF, pending
  completions and the STI/SS-load shadows, then enters the guest IVT handler.
  `completed_steps()` drives machine time; `retired()` remains available for
  whole-instruction diagnostics.
- DMA byte access uses `Vm::read_bytes` / `write_bytes`; writes run `note_write`
  for self-modifying-code invalidation
  ([decision-6](../decisions/decision-6%20-%20Guest%20memory%20writes%20via%20note_write%20for%20SMC%20safety.md)).
  The backend remembers the regions it mapped: a DMA write into ROM is
  dropped and a DMA read from ROM returns ROM bytes, while a byte in a trapped
  MMIO window is handed back to the machine (`DmaAccess::Mmio`), which runs it
  as a bus MMIO cycle on the owning device. Read errors return `0xFF`; write
  errors are discarded by the current byte seam.

## In flight and further reading

- **Board validation:** [`design/compatibility-diagnostics.md`](design/compatibility-diagnostics.md)
  records the diagnostic-ROM spike, user-supplied firmware policy, expected
  Landmark AT coverage and the golden capture rule; execution is not qualified.
- **Scheduler:** [decision-15](../decisions/decision-15%20-%20Deterministic%20device%20scheduler%20with%20instruction-exact%20interrupt%20boundaries.md)
  amends [`design/scheduler.md`](design/scheduler.md). Instruction-boundary
  timing and deadline dispatch are adopted. Per-clock dispatch survives only as
  a feature-gated test oracle; the harness compares ordered accesses, INTA/EOI,
  RAM, registers, saved device state and stop endpoints without normalization
  between the two engines. The post-adoption profile is in
  [`design/machine-time-profile.md`](design/machine-time-profile.md).
- **Observation:** [`design/observation.md`](design/observation.md) describes
  typed device capabilities and assembly-owned observation routes. Access traces
  are opt-in, bounded and independent of semantic output; the document retains
  the historical inventory. Prompt polling, final capture and presentation now share one route-based facade.

[`design/machine-config.md`](design/machine-config.md) explains composition;
[`design/spec.md`](design/spec.md) specifies hardware coverage and seams;
[`design/machine-time-profile.md`](design/machine-time-profile.md) records timing
measurements. Historical decisions are under [`../decisions/`](../decisions/);
accepted designs may precede implementation. Deferred hardware and fidelity
work is listed in [`deferred.md`](deferred.md).
