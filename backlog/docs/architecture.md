# Architecture

This describes the implementation on `main`: a deterministic, headless IBM
PC/AT-class emulator with a real-mode CPU, device models, declarative assembly,
and BIOS/FreeDOS integration tests. Testability, swappability, and documented
hardware coverage guide the crate boundaries. The scheduler and observation
migrations below are in flight; their design documents are not implementation
inventories. The hardware specification is [`design/spec.md`](design/spec.md).

## Crate map and dependencies

These are the direct dependencies from each crate's `Cargo.toml`; test-only
dependencies are separate. Workspace membership and the exact `x86jit-core` git
revision are in the root [`Cargo.toml`](../../Cargo.toml). There is no firmware
crate: image loading belongs to `assembly`, and guest assets live under `vendor/`.

| Crate | Responsibility | Runtime dependencies | Test-only dependencies |
| --- | --- | --- | --- |
| `device` | Hardware-neutral contracts: `Device`, `InterruptController`, `DmaMemory`, `VClock`, `NextEvent`, access traces and DMA messages | `thiserror` | None |
| `cpu` | `Cpu`/`CpuStop` seam and `X86JitCpu`, which owns the core's `Vm` and `Vcpu` | `x86jit-core`, `thiserror` | None |
| `bus` | Owns boxed devices and executes a port/MMIO `DecodeMap`; open-bus policy and ticking | `device`, `thiserror` | None |
| `devices` | UART 16550, PIC 8259, PIT 8253, 8042, CMOS/RTC, DMA 8237, uPD765 floppy controller, VGA text, system control and POST port | `device`, `thiserror` | `testkit`, `video` |
| `chipset` | Shared/cascaded PIC composition, null interrupt controller, AT port constants; no `Chipset` trait | `device`, `devices` | `testkit` |
| `machine` | Generic `Machine<C: Cpu>` and execution/clock/IRQ/DMA loop | `cpu`, `bus`, `device`, `thiserror` | `testkit`, `devices` |
| `video` | `FrameSink`, text frames/grids, text-mode renderer and capture sink | None | None |
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
clock rate and quantum, RAM/MMIO layout, devices, drives, loadable images, and
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
`Machine::run()` directly. The Long64 serial demo is assembled inline in `cli`;
the declarative assembler currently constructs Real16 machines.

### Current execution order

The authoritative loop is
[`Machine::run_bounded`](../../crates/machine/src/lib.rs). For a running CPU:

1. Check the absolute virtual-time cap, then call `Cpu::run(quantum)`.
2. Read the retired-instruction delta and advance device time. For each PIT
   input clock, increment `VClock`, call `Bus::tick_all`, and sample wired IRQ
   levels for rising edges.
3. If no injected vector is pending, acknowledge a serviceable PIC request and
   queue it with `Cpu::inject_irq`.
4. Dispatch the CPU stop: port/MMIO access through the bus, read completion or
   MMIO-write acknowledgement; budget exhaustion continues, exceptions return
   an error, and HLT suspends execution or returns a halted outcome.
5. Service wired DMA requests before the next CPU run.

Thus **clock advancement and IRQ acknowledgement currently precede the stopping
instruction's bus dispatch**. The intended bus-effects-before-INTA ordering in
decision-15 is not implemented by this loop. A CPU quantum can overshoot a run
cap, and the quantum is still measured in core blocks, not instructions.

[`Bus`](../../crates/bus/src/lib.rs) splits wider port accesses into consecutive,
independently decoded byte cycles, low byte first. Undecoded ports default to
all-ones reads/dropped writes with tracing; strict faulting is selectable.
Undecoded MMIO faults. Read results use `complete_port_in` or
`complete_mmio_read`; trapped writes require `complete_mmio_write` so retrying
the instruction does not repeat the device side effect.

While halted, the loop stops immediately with IF clear (NMI is not modelled).
With IF set it first tries IRQ service; otherwise, subject to idle/cap bounds,
it advances one PIT clock, ticks devices and samples edges, services IRQs, then
DMA. A queued IRQ permits CPU re-entry. Halt state and accumulated idle time
survive bounded runner calls.

## Contracts and ownership

- **[`Cpu`](../../crates/cpu/src/lib.rs)** exposes `run`, general-purpose
  registers/RIP, port/MMIO completions, IF status, IRQ injection/pending status,
  DMA byte access and the retired counter. It has no segment-register API,
  bus-binding API or `irq_ack()` method. Segment seeds and `code_segment()` are
  concrete `X86JitCpu` setup/inspection APIs.
- **[`Device`](../../crates/device/src/lib.rs)** supplies port/MMIO access,
  `tick(VClock)`, IRQ level/vector queries, reset, per-device save/restore, trace
  drains and optional DMA request/service/completion hooks. Host-side
  `as_any_mut` is an optional downcast for input/output glue, not execution-loop
  hardware selection. `NextEvent::{EveryPitClock, At(VClock), Dormant}` is an
  additive scheduling contract; `next_event` defaults to `EveryPitClock` and
  the current loop does not consult it. Timestamped access wrappers and lazy
  device synchronization remain migration work.
- **`InterruptController`**, defined in `device`, supplies `set_irq_line`,
  non-mutating `poll`, `acknowledge`, and `end_of_interrupt`. Machine wiring
  samples device levels and pulses a PIC input on a sampled low-to-high edge.
  The current acknowledgement checks for an already queued vector, not CPU
  interrupt readiness. The CPU holds a queued vector until delivery is allowed;
  guest EOI port writes reach the same shared PIC state. This is the existing
  [IRQ contract](../decisions/decision-4%20-%20Hardware%20IRQ%20injection%20contract.md),
  pending the ordering/readiness replacement in decision-15.
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

### Observation and traces today

`TraceEvent::{Port, Mmio}` records timestamp, direction, address/port, width and
value. Devices drain access history with `take_trace`; the bus separately drains
undecoded-port history. Current concrete devices and the bus retain unbounded
trace vectors. These access records are also used by existing output capture:

- CLI VGA capture replays drained writes into fresh VRAM/CRTC state, then calls
  `video::render_text_mode`. The card already has live-state getters, used by its
  device-level golden, but CLI capture does not use them.
- POST bytes are filtered from port-write traces. UART transmission instead
  comes from `Uart16550::take_output` through a frontend downcast, preserving
  the distinction between transmitted bytes and divisor-latch writes.
- The BIOS MDA console is a RAM probe with fixed start and hidden cursor,
  separate from the VGA colour window. Prompt detection has its own RAM probe.
- `run_vga_text` manually presents its final grid to a capture sink. BIOS runners
  return grids and byte streams. `OutputConfig::frame_sink` is declared but
  `assemble()` does not wire a sink from it.

See [`cli/src/lib.rs`](../../crates/cli/src/lib.rs) and
[`design/observation.md`](design/observation.md). A unified observation facade,
state-based display capture, semantic stream routes and independently bounded
tracing are proposed work, not current public contracts. Draining a trace loses
the history on which replay-based capture depends.

## Virtual clock

[Decision-5](../decisions/decision-5%20-%20Virtual%20clock%20in%20retired%20instructions.md)
sets retired instructions as the deterministic time source; block counts and
wall time do not define device time. `VClock(u64)` counts whole PIT input clocks.
[`ClockConfig`](../../crates/assembly/src/clock.rs) supplies the positive rational
CPU rate A/B (`retired_per_pit_clock` / `retired_per_pit_clock_denominator`), in
retired steps per PIT clock, plus the cooperative block quantum. Assembly passes
the rate into `Machine::with_cpu_rate`; the default is 1/1. Running time emits
`floor((remainder + retired_delta * B) / A)` PIT clocks and carries the remainder
across CPU exits. This is an instruction-timed approximation, not cycle timing.

[Decision-14](../decisions/decision-14%20-%20Virtual%20time%20advances%20while%20the%20CPU%20is%20halted.md)
extends time through HLT: halted clocks tick devices without retiring CPU
instructions or consuming the running clock remainder. `Machine::virtual_time`
is retired instructions plus `floor(total_halted_clocks * A / B)`, saturating
at `u64::MAX`. Caps and policy targets use these instruction equivalents;
device clocks use `VClock`. Golden runs depend on the CPU revision, config and
policy cadence, including the current block quantum.

## x86jit integration

[`X86JitCpu`](../../crates/cpu/src/x86jit_backend.rs) adapts a pinned external
`x86jit-core`; it uses the interpreter for Real16 and also supports the Long64
serial demo. CPU changes are developed upstream and consumed by re-pinning,
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
  completions and the STI shadow, then enters the guest IVT handler. `retired()`
  forwards `Vcpu::retired_instructions()` to the machine clock.
- DMA byte access uses `Vm::read_bytes` / `write_bytes`; writes run `note_write`
  for self-modifying-code invalidation
  ([decision-6](../decisions/decision-6%20-%20Guest%20memory%20writes%20via%20note_write%20for%20SMC%20safety.md)).
  **Known limit:** these are host backing-memory APIs, not guest bus accesses:
  they bypass ROM guest-write protection and MMIO trap dispatch. DMA is safe
  for the intended RAM buffers, not a faithful DMA-to-ROM/MMIO model. Read
  errors return `0xFF`; write errors are discarded by the current byte seam.

## In flight and further reading

- **Scheduler:** [decision-15](../decisions/decision-15%20-%20Deterministic%20device%20scheduler%20with%20instruction-exact%20interrupt%20boundaries.md)
  amends [`design/scheduler.md`](design/scheduler.md). The rational CPU rate,
  `NextEvent` contract and test-only A/B observation/comparison harness are on
  `main`. Event skipping, instruction-exact CPU budgets, SS-load interrupt
  inhibition and readiness-based INTA after bus/DMA effects remain in flight;
  the production loop is still the per-clock implementation described above.
- **Observation:** [`design/observation.md`](design/observation.md) proposes
  replacing replay and output downcasts with typed device capabilities and
  assembly-owned observation routes. The migration has not landed.

[`design/machine-config.md`](design/machine-config.md) explains composition;
[`design/spec.md`](design/spec.md) specifies hardware coverage and seams;
[`design/machine-time-profile.md`](design/machine-time-profile.md) records timing
measurements. Historical decisions are under [`../decisions/`](../decisions/);
accepted designs may precede implementation. Deferred hardware and fidelity
work is listed in [`deferred.md`](deferred.md).
