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
crate: image loading belongs to `assembly`, and guest assets live under `vendor/`.

| Crate | Responsibility | Runtime dependencies | Test-only dependencies |
| --- | --- | --- | --- |
| `device` | Hardware-neutral contracts: `Device`, `InterruptController`, `DmaMemory`, `VClock`, `NextEvent`, access traces, DMA messages, and the observation capabilities (text scanout, byte output, `ByteCapture`) | `thiserror` | None |
| `cpu` | `Cpu`/`CpuStop` seam and `X86JitCpu`, which owns the core's `Vm` and `Vcpu` | `x86jit-core`, `thiserror` | None |
| `bus` | Owns boxed devices and executes a port/MMIO `DecodeMap`; open-bus policy and ticking | `device`, `thiserror` | None |
| `devices` | UART 16550, PIC 8259, PIT 8253, 8042, CMOS/RTC, DMA 8237, uPD765 floppy controller, VGA text, system control and POST port | `device`, `thiserror` | `testkit`, `video` |
| `chipset` | Shared/cascaded PIC composition, null interrupt controller, AT port constants; no `Chipset` trait | `device`, `devices` | `testkit` |
| `machine` | Generic `Machine<C: Cpu>` and execution/clock/IRQ/DMA loop | `cpu`, `bus`, `device`, `thiserror` | `testkit`, `devices` |
| `video` | `FrameSink`, text frames/grids, text-mode renderer and capture sink | `device` | None |
| `assembly` | `MachineConfig`, validation, presets, image/device construction, wiring, and `RunPolicy` driver | `machine`, `cpu`, `bus`, `device`, `devices`, `video`, `chipset`, `thiserror` | None |
| `testkit` | Single-device `MockBus`, scripted `MockCpu`, golden assertions | `device`, `cpu` | None |
| `cli` | Binary and runner library: guest programs, assembly/policy selection, output capture and stop diagnostics | `assembly`, `device`, `bus`, `cpu`, `chipset`, `machine`, `devices`, `video`, `anyhow` | `testkit` |

Concrete hardware selection lives in `assembly` and the frontend. `machine`
does not depend on `chipset`, `devices`, `video`, or x86jit directly. `Bus` is a
concrete router over `dyn Device`, not a trait; the config-derived decode map is
its routing authority. Concrete devices do not depend on the runtime bus.

## From configuration to a run

The normal real-mode path is:

```text
MachineConfig (preset or caller data)
  -> assemble(&config)
  -> AssembledMachine { Machine<X86JitCpu>, device_index }
  -> RunPolicy + assembly::run(&mut machine, &policy)
  -> Machine::run_bounded(...) -> output capture / stop classification
```

[`MachineConfig`](../../crates/assembly/src/machine.rs) describes CPU/reset state,
clock rate, RAM/MMIO layout, devices, drives, loadable images, and
output selection. [`assemble()`](../../crates/assembly/src/assemble.rs) validates
it before construction, maps CPU memory, loads images, creates devices in config
order, derives CMOS contents from memory/drives, and resolves device roles into
bus indices. It builds the decode map, IRQ lines and DMA couplings. One shared
master/slave PIC pair backs both bus faces and the interrupt-controller seam;
a configuration without a PIC receives a null controller.

[`RunPolicy`](../../crates/assembly/src/run.rs) describes how to drive that machine:
virtual-time cap, halted idle limit, natural stop or console-prompt polling, and
keyboard stimuli after the prompt. `run()` applies the idle limit and drives
`run_bounded`; prompt polling reads a configured RAM console through
`ConsoleProbe`. Policy cadence affects guest execution and golden identity.
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

[`Bus`](../../crates/bus/src/lib.rs) splits wider port accesses into consecutive,
independently decoded byte cycles, low byte first. Undecoded ports default to
all-ones reads/dropped writes; optional bounded tracing and strict faulting are
selectable.
Undecoded MMIO faults. Read results use `complete_port_in` or
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
  `tick(VClock)`, IRQ level/vector queries, reset, per-device save/restore, trace
  drains and optional DMA request/service/completion hooks. Host-side
  `as_any_mut` is an optional downcast for input/output glue, not execution-loop
  hardware selection. `NextEvent::{EveryPitClock, At(VClock), Dormant}` is an
  scheduling contract consulted by the production deadline loop; `next_event`
  defaults to `EveryPitClock` for unmigrated devices. Timestamped access wrappers
  synchronize the target device at instruction start; deadline dispatch settles
  due transitions without ticking dormant devices at each CPU boundary.
- **`InterruptController`**, defined in `device`, supplies `set_irq_line`,
  non-mutating `poll`, `acknowledge`, and `end_of_interrupt`. Machine wiring
  samples device levels and pulses a PIC input on a sampled low-to-high edge.
  Acknowledgement requires CPU IRQ readiness and no already queued vector,
  after the stopping instruction's bus/DMA effects have settled. Guest EOI port
  writes reach the same shared PIC state. See the adopted ordering in
  [scheduler design](design/scheduler.md#adoption).
- **[`FrameSink`](../../crates/video/src/lib.rs)** consumes `&Frame` via
  `present`; the current frame is `Frame::Text(TextGrid)`. Rendering turns VRAM
  and display-start/cursor state into cells with glyph bytes and attributes.
  `CaptureFrameSink` retains the last frame. Presentation is driven by callers,
  not by the machine loop.

The bus owns devices; `X86JitCpu` owns guest memory. Devices do not retain RAM
references. DMA requesters supply a channel and payload/length, the configured
controller supplies addresses/counts, and the machine lends a temporary
`DmaMemory` adapter over the CPU seam. `DmaOutcome` returns a transfer count or
the bytes read, truncated to the controller's actual transfer length.

### Observation and presentation

`assemble_with_observation` binds requested routes by `DeviceId` and validates
capabilities before execution. `AssembledMachine::capture_text` captures display
state at a stamped machine boundary; semantic byte sinks collect UART TX and
POST independently of access tracing. `ObservationPolicy.trace` selects Off
(default) or `Ring { capacity }` for every device and the open bus. All recorders
share overwrite-oldest behavior and cumulative drop counters; draining retains
the count. Diagnostics and their policy are excluded from hardware snapshots.
Capture preserves saved device state and
does not consume traces. BIOS runners return the colour grid, MDA RAM-probe
grid, COM1/POST streams, final floppy media and classified stop. Prompt detection
uses the same RAM text renderer as the MDA console. `run_vga_text` presents its
captured grid through a `FrameSink`; presentation remains outside the machine loop.

See [`design/observation.md`](design/observation.md) for the route contracts and
historical design inventory.

## Virtual clock

[Decision-5](../decisions/decision-5%20-%20Virtual%20clock%20in%20retired%20instructions.md)
sets retired instructions as the deterministic time source; block counts and
wall time do not define device time. `VClock(u64)` counts whole PIT input clocks.
[`ClockConfig`](../../crates/assembly/src/clock.rs) supplies the positive rational
CPU rate A/B (`retired_per_pit_clock` / `retired_per_pit_clock_denominator`), in
completed steps per PIT clock. Assembly passes
the rate into `Machine::with_cpu_rate`; the default is 1/1. Running time emits
`floor((remainder + completed_steps_delta * B) / A)` PIT clocks and carries the remainder
across CPU exits. This is an instruction-timed approximation, not cycle timing.

[Decision-14](../decisions/decision-14%20-%20Virtual%20time%20advances%20while%20the%20CPU%20is%20halted.md)
extends time through HLT: halted clocks tick devices without retiring CPU
instructions or consuming the running clock remainder. `Machine::virtual_time`
is completed steps plus `floor(total_halted_clocks * A / B)`, saturating
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

- **Scheduler:** [decision-15](../decisions/decision-15%20-%20Deterministic%20device%20scheduler%20with%20instruction-exact%20interrupt%20boundaries.md)
  amends [`design/scheduler.md`](design/scheduler.md). Instruction-boundary
  timing and deadline dispatch are adopted. Per-clock dispatch survives only as
  a feature-gated test oracle; the harness compares ordered accesses, INTA/EOI,
  RAM, registers, saved device state and stop endpoints without normalization
  between the two engines. Phase profiling after adoption is separate work.
- **Observation:** [`design/observation.md`](design/observation.md) describes
  typed device capabilities and assembly-owned observation routes. Access traces
  are now opt-in, bounded and independent of semantic output; the document retains
  the historical inventory and describes the remaining observation work.

[`design/machine-config.md`](design/machine-config.md) explains composition;
[`design/spec.md`](design/spec.md) specifies hardware coverage and seams;
[`design/machine-time-profile.md`](design/machine-time-profile.md) records timing
measurements. Historical decisions are under [`../decisions/`](../decisions/);
accepted designs may precede implementation. Deferred hardware and fidelity
work is listed in [`deferred.md`](deferred.md).
