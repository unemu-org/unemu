# Observing machine output

Design and migration record, 2026-11-09. Typed scanout and semantic byte sinks
are implemented. The inventory and prototype evidence below describe the earlier
implementation; the trace policy and its migration gate are now implemented.
BIOS/FreeDOS monochrome output now comes from an independent MDA card, as
specified in §The MDA card console below.

## Implemented access trace policy

`device::TracePolicy` has two modes: `Off` (the default) and
`Ring { capacity }`. Every concrete device and the open bus use the same
`TraceBuffer`. Off retains no events, counts no drops and allocates no history.
A ring retains the newest events in access order, overwriting the oldest and
incrementing a saturating `u64` drop counter. Capacity zero retains nothing and
counts every event as dropped. `Device::take_trace` and
`Bus::take_open_bus_trace` drain retained events without clearing the counters;
`Device::trace_dropped` and `Bus::open_bus_trace_dropped` report lost events.
A drain is bounded by the configured capacity.

`ObservationPolicy.trace` explicitly selects the policy for all devices and the
open bus during assembly. `assemble`, presets and all production CLI capture
policies leave it Off. Selecting serial/POST output or a display never enables
tracing. `Bus::set_trace_policy` can also configure an existing machine; both
this operation and device-level reconfiguration discard prior history and reset
the counters. Hardware reset preserves policy and starts a new diagnostic
history. `SharedPic` forwards policy, drain and counter to its one live chip.

Machine timing tests in `crates/machine/src/corrected_tests.rs` also explicitly
select rings when asserting port/MMIO timestamps. Shared-PIC tests verify that
cloned faces configure and drain one recorder and expose the same drop count.

Snapshots exclude trace events, policy and counters. All ten device snapshot
formats already excluded access traces, so no version bump is necessary.
Restore neither emits nor restores diagnostics; the host's existing trace epoch
continues. Per-device isolation tests check that draining a trace cannot change
snapshot bytes and restoring into a fresh device cannot import history.

Trace-consuming unit tests opt into rings sized above their bounded access
counts. The scheduler A/B harness uses a 16-event ring per device, drains after
each transaction and asserts zero drops. Its complete comparison stream remains
a test-owned oracle. A slow CLI regression runs BIOS POST to its golden cap,
BIOS floppy boot, FreeDOS prompt, DIR and file-write sessions with Off and a
two-event overflowing ring. It checks every device/open-bus retained length and
observed drops, and compares the complete `BiosRun`: serial, POST, both screens
(including attributes/cursors), capture stamps, final media and stop reason.

No new environment variable or CLI option is needed; the policy is a Rust
observation/configuration choice.

## UART output ownership and snapshot compatibility

The UART sends accepted THR bytes directly to its bound `ByteSink`, without
retaining a device-owned output history. With no sink, transmission discards
the byte without allocating. Assembly already binds COM1 to one explicitly
bounded `ByteCapture`; consumers drain that collector through
`Observation::drain_bytes`. No assembly wiring change is needed. Divisor-latch
writes still emit nothing, and sink overflow does not change device state.

UART snapshot v3 is exactly 16 bytes: version, IER, FCR, LCR, MCR, scratch,
DLL, DLM, then the little-endian 64-bit absolute trace clock. It excludes
transmitted bytes and the former output-length field. Versions 1 and 2 are
rejected rather than importing host capture history. Restore rejects every
truncation and trailing byte before changing registers or time, retains the
receiving instance's port base and sink, and emits no historical output.
Hardware reset also retains the sink; capture epochs remain host-owned.

An isolated mock-bus regression transmits 1 MiB each with no sink, an
order-checking counting sink, and an overflowing three-byte capture. It checks
unchanged snapshot bytes and length, no retained access traces, complete ordered
sink delivery, and bounded capture retention. Snapshot tests separately cover
legacy-version rejection without mutation and continuation through the sink.

## Recommendation

Observe **state for displays** and **semantic emissions for streams**. A display
snapshot comes from the card's current memory and registers. Serial and POST
bytes come from the device at the point it accepts an output byte. Render each
snapshot once through `video::render_text_mode`, then distribute the same
`video::Frame` to capture and presentation sinks. Device access traces remain a
separate, explicitly enabled, bounded debugging facility.

Keep the machine core independent of VGA, UART, POST port numbers, and host
presenters. Add optional, typed observation capabilities to `device::Device` and
resolve their routes in `assembly`, using its existing device-role map. A future
window and a golden runner consume those same routes. A GUI must never implement
CRTC decoding or replay guest writes.

**Measured limitation of compatibility:** the current VGA text golden is fully
identical under state capture and trace replay. The BIOS colour-page text and
attributes are identical, but its cursor is not: state capture gives `(0, 18)`,
replay gives `(0, 0)`. Both are visible. The replay omits monochrome CRTC writes.
Existing golden assertions still pass; full `BiosRun.screen` equality cannot be
promised. See § Prototype evidence and § Migration.

## Inventory of the current paths

This section describes the code before the disposable prototype. `TraceEvent`
has only `Port` and `Mmio` variants (`crates/device/src/lib.rs`); neither is a
semantic output event. All ten concrete devices keep `Vec<TraceEvent>` buffers.
The bus separately records undecoded port accesses. A drain removes history;
there is no trace capacity or enable policy.

| Output | Producer | How it is read today / consumers | Problems |
| --- | --- | --- | --- |
| Colour text screen, glyphs, attributes, start address, cursor | `crates/devices/src/vga_text.rs`, `VgaText`: 32 KiB VRAM and CRTC; the existing `vram`, `start_cell`, `cursor_cell`, `cursor_visible` getters already expose state | `crates/cli/src/lib.rs`, `render_vga`, drains `take_trace`, reconstructs VRAM and a second CRTC register file, calls `render_text_mode`; used by `run_vga_text` and every successful `run_bios_config` assembly | Replay ignores `0x3B4/0x3B5`, unlike the preset and card of that time (the colour card's monochrome aliases were later removed; the MDA card owns `0x3Bx`); uses 256 independent index slots, unlike the card's modulo-25 indexing. A second capture loses prior writes. Restored state cannot be reconstructed from subsequent traces. |
| Card-level text golden | Same `VgaText` state | `crates/devices/tests/vga_text_golden.rs`, `hello_renders_to_captured_grid`, already uses getters → `TextModeState` → `CaptureFrameSink`; checks committed `golden/vga_text_hello.txt`, attribute and cursor | This is the desired path, but CLI duplicates it with replay. |
| MDA console text (current routing) | `MdaText` owns the MMIO aperture at `0xB0000` and its independent CRTC | CLI captures `DeviceId::Mda` through `capture_text`; FreeDOS/DOS goldens read `BiosRun.mda` | Card scanout supplies memory, start address and cursor; capture no longer reads CPU backing memory. |
| Prompt detection (current routing) | Same MDA card scanout | `crates/assembly/src/run.rs`, `console_contains`, renders the `ConsoleProbe` device's scanout; `poll_to_prompt` and `run` use it to stop and inject keys | Separate capture call remains; shared observation scheduling is deferred. Poll cadence and injection schedule stay fixed. |
| COM1 transmitted byte stream | `crates/devices/src/uart16550.rs`, THR branch in `port_write`; `output: Vec<u8>` | CLI `drain_serial`: `Device::as_any_mut` → `Uart16550` → `take_output`. Six serial-returning runners, `BiosRun.com1`, CLI stdout; device tests also inspect `output()` / `take_output()` | Semantic source is correct and excludes DLAB divisor writes. Concrete downcast and silent empty fallback do not validate route/type mismatches. Output and access traces are both unbounded; `save` currently includes pending output. |
| POST checkpoints | `crates/devices/src/post_port.rs`, `PostPort` at configured port (`0x80` or `0x190`) | CLI `drain_post_at` filters `take_trace` for writes to the selected port, casts each value to `u8`; `BiosRun.post`, `Test386Run.post` | No semantic stream or last-code latch independent of trace. Trace policy cannot change until this consumer moves. Reads must never become checkpoints. |
| Port/MMIO diagnostics | UART, VGA, POST, `Pic8259`, `Pit8253`, `I8042`, `CmosRtc`, `Dma8237`, `Upd765`, `SysControl` in `crates/devices/src/` | `Device::take_trace`; all callers enumerated below. `SharedPic` forwards the drain to the same live PIC (`crates/chipset/src/shared_pic.rs`) | Every serviced access allocates retained history. Most machine runs never drain PIC/PIT/keyboard/RTC/DMA/FDC/system-control traces; UART output draining does not drain its trace. Access traffic is not a display/audio model. |
| Undecoded-port diagnostics | `crates/bus/src/lib.rs`, `open_bus_trace` | `Bus::take_open_bus_trace`, only bus unit tests currently consume it | Unbounded recording also occurs outside devices; fixing device buffers alone leaves this path. No current BIOS runner returns these diagnostics despite its source comment mentioning goldens. |
| Run termination / diagnostic site | CPU, bus and machine run results | CLI `StopReason` classification, returned in `Test386Run.stop` / `BiosRun.stop`; errors in other runners remain `anyhow::Result` | Orthogonal to device output. Preserve capture on halt, cap and fault. `Unsupported`/`Unmapped` detail strings include addresses/bytes for diagnosis, not stable golden text; `Exception` carries a vector. |
| Presentation | `crates/video/src/lib.rs`: `Frame::Text`, `TextGrid`, `FrameSink`, `CaptureFrameSink` | `run_vga_text` manually presents one final frame. Card golden and video unit test also present. `CaptureFrameSink` retains only the last frame | BIOS runner returns grids without presenting. `crates/assembly/src/output.rs` declares `OutputConfig.frame_sink` and `FrameSinkKind::{Headless,Capture}`, but `assemble` does not inspect it; all presets choose the default. Selecting `Capture` currently wires no sink. |
| Speaker/audio (future) | Potentially PIT channel 2 plus `SysControl` gate/data-enable | No audio frame, sample sink or runner field exists. `SysControl` stores the control bits but does not wire the gate to PIT; its timer-2 status is a per-read toggle (`crates/devices/src/sys_control.rs`, coverage matrix). PIT exposes `out(n)` (`crates/devices/src/pit8253.rs`) | There is no valid sound output to capture yet. Replaying port writes would implement another timer. Audio needs actual emulated signal production before an observer can consume it. |
| Floppy writes and machine-test capture bytes | `Upd765` / `Drive` mutate mounted image bytes; machine unit tests use mock capture devices | FDC tests inspect transfer/read-back results, CLI WRITE golden observes a serial marker; `crates/machine/src/lib.rs` tests inspect mock devices through `save()` | These are storage effects / test doubles, not additional frontend output fields. Host-file write-back and whole-machine snapshots remain deferred (`backlog/docs/deferred.md`). Do not turn `Device::save` blobs into a display API. |

### Complete runner result and consumer map

Only `Test386Run` and `BiosRun` are public run-result structs in
`crates/cli/src/lib.rs`. There are no hidden audio, disk-image, trace, retired-count
or clock fields in them.

| Runner / result | Every output field or successful return value | Current golden / frontend consumers |
| --- | --- | --- |
| `run_test386` → `Test386Run` | `post: Vec<u8>`, `stop: StopReason` | `crates/cli/tests/test386_golden.rs`: exact POST bytes and unsupported-stop variant |
| `run_bios`, `run_bios_with_cap`, `run_bios_boot`, `run_freedos`, `run_freedos_interactive` → `BiosRun` via `run_bios_config` | `post: Vec<u8>`, `screen: TextGrid`, `screen_stamp: Option<ScreenStamp>`, `mda: TextGrid`, `com1: Vec<u8>`, `floppy_image: Option<Vec<u8>>`, `stop: StopReason` | `bios_boot_golden.rs`: exact POST and cap-stop variant; `bios_floppy_boot_golden.rs`: exact COM1 marker and diagnostic stop print; `freedos_boot_golden.rs`, `dos_interactive_golden.rs`, `dos_write_golden.rs`: MDA console substrings and cap-stop variant. All paths under `crates/cli/tests/`. BIOS goldens also assert display cursor and capture stamps; snapshot tests cover the colour grid. |
| `run_boot` → `Result<Vec<u8>>` | COM1 bytes | `boot_golden.rs`: exact timer markers; `main.rs --boot`: stdout |
| `run_keyboard` → `Result<Vec<u8>>` | COM1 bytes | `keyboard_golden.rs`: exact queued scancode |
| `run_rtc` → `Result<Vec<u8>>` | COM1 bytes | `rtc_golden.rs`: exact RTC marker |
| `run_fdc_write` → `Result<Vec<u8>>` | COM1 bytes | `fdc_write_golden.rs`: exact write/read-back success marker |
| `run_floppy_boot` → `Result<Vec<u8>>` | COM1 bytes | `floppy_boot_golden.rs`: exact boot-sector marker |
| `run_vga_text` → `Result<TextGrid>` | Captured final colour grid | `vga_text_golden.rs`: dimensions, first cells/attributes, untouched cell, first-row string; separate program-byte assertion |

`crates/cli/tests/quantum.rs` additionally checks CPU retired counts and
`Machine::virtual_time` directly; those are timing controls, not output streams.
Hand-encoded program-byte assertions in these tests do not consume device traces.
`TextGrid::rows_as_strings` / `to_dump` preserve the existing Latin-1-like byte to
character conversion, null-to-space mapping and trailing-whitespace trimming.
Those dumps are intentionally distinct from a future presenter's CP437/font
rasterisation (`crates/video/src/lib.rs`).

The remaining direct state/output consumers are unit tests. In
`crates/devices/src/uart16550.rs`, `take_output()` is read by
`thr_write_appends_to_output`, `dlab_banks_divisor_latch_then_restores_thr` and
`mock_bus_output_excludes_divisor_writes`; `output()` is read by
`save_restore_round_trip_preserves_state`, `reset_clears_state` and
`hello_via_output_and_trace`. CLI `drain_serial` is the only production caller
of these UART methods. In `crates/devices/src/vga_text.rs`, the state getters are
read by `mmio_word_write_stores_glyph_and_attribute`,
`mmio_byte_writes_hit_glyph_then_attribute`, `crtc_cursor_location_round_trips`,
`crtc_start_address_round_trips`, `cursor_disable_bit_hides_cursor`,
`reset_clears_buffer_and_registers` and
`save_restore_round_trip_preserves_state`. The pre-spike card golden above is
their only consumer outside that module. `crates/video/src/lib.rs` unit tests
exercise synthetic state, dump conversion and last-frame capture; they do not
reach a machine or device. No further frontend presents frames today.

### Every current `take_trace` caller

Paths and containing symbols identify all call sites, including test-only
consumers. Multiple drains in one symbol are noted. Implementations returning a
buffer are not additional consumers.

| Path | Callers / purpose |
| --- | --- |
| `crates/cli/src/lib.rs` | `drain_post_at`; `render_vga`; `tests::drain_serial_excludes_divisor_writes` (drains trace to prove UART semantic output survives) |
| `crates/chipset/src/shared_pic.rs` | `SharedPic::take_trace` forwards to `inner.borrow_mut().take_trace()`; not a second recording buffer |
| `crates/devices/src/post_port.rs` | `writes_surface_on_trace_in_order`; `trace_is_drained_on_take` (two calls); `with_port_decodes_the_bios_post_port` |
| `crates/devices/src/uart16550.rs` | `mock_bus_output_excludes_divisor_writes`; `hello_via_output_and_trace`; `mock_bus_isolation_harness` |
| `crates/devices/src/vga_text.rs` | `mmio_write_is_traced_with_virtual_time` |
| `crates/devices/src/sys_control.rs` | `writes_and_reads_surface_on_trace` (two calls) |
| `crates/devices/src/pic8259.rs` | `mock_bus_isolation_harness` |
| `crates/devices/src/pit8253.rs` | `trace_seam_records_accesses` |
| `crates/bus/src/lib.rs` | `word_out_sets_crtc_index_then_data`; `word_read_combines_devices_little_endian` (one call per device); `word_access_faults_on_undecoded_high_byte_after_low_byte`; `dword_access_uses_four_byte_cycles_in_order`; `byte_access_keeps_value_width_and_neighbor_unchanged` |

No other production or test caller drains device traces. In particular RTC,
keyboard, FDC and DMA implement the method but have no callers that drain it.
The default implementation lives in `crates/device/src/lib.rs`; all ten concrete
devices implement it. The mock devices in bus and machine tests also implement
it; machine tests do not call it.

For completeness, `take_open_bus_trace` is called only in
`crates/bus/src/lib.rs` tests:
`word_access_applies_open_bus_to_undecoded_high_byte`,
`word_access_faults_on_undecoded_high_byte_after_low_byte`,
`open_bus_accesses_are_traced_with_the_virtual_clock` (including a second empty
drain), `decoded_ports_bypass_open_bus_and_are_not_traced`, and
`strict_mode_still_faults_on_undecoded_ports`.

The disposable prototype renames the old CLI consumer to `render_vga_trace` and
adds drains in `vga_snapshot_survives_trace_drain` and
`host_snapshot_reads_live_state_without_guest_accesses`. These are experimental
verification, not additional production requirements.

## Proposed observation contract

### Typed capabilities and route ownership

Use a small optional `Device` capability for a **borrowed text scanout state**:
VRAM slice, dimensions, display start, cursor location and visibility. A query
takes `&self`, has no guest I/O side effects, and does not drain anything. The
card populates the scalars from its existing getters; only the card knows its
register aliases. Repeated queries without guest execution produce the same
frame, regardless of trace consumption or recording policy.

The neutral text-state data can live in `device`; `video::TextModeState` can
re-export it and the renderer can continue using it. This introduces only the
acyclic dependency `video → device`, not `device → devices` or
`devices → video`. An owned `Frame` is created before the card borrow ends and
before execution resumes. Do not export mutable VRAM or a register-file mirror.
Keep the current renderer's fixed mode dimensions, blank-past-end behavior and
cursor calculation during this migration; hardware rendering changes are a
separate task.

For **semantic byte streams**, attach a typed byte sink when assembly binds an
output route. The UART emits only from its accepted THR path with DLAB clear;
POST emits the low byte of each accepted write, in order, including repetitions,
zero and `0xFF`. Reads emit nothing. The sink receives bytes directly; it never
decodes ports. A byte-output capability describes its stream kind (serial TX or
POST checkpoints) and allows attaching a sink; devices without it return no
capability. Later audio uses a separate signal/sample contract, not a byte-stream
kind with ambiguous units.

`AssembledMachine` should expose an observation facade that owns the route table
and collectors. Resolve a display or stream by stable `DeviceId` and output
channel at assembly time; validate capabilities then. An unavailable or
mismatched requested output is an explicit configuration error, not a blank
screen or empty serial stream. The former Long64 `run_serial` demo and
`golden_serial.rs` are removed; all machine runners use assembled Real16 CPUs.
`main.rs` offers `--boot`; no arguments print usage.

The facade can call a hardware-neutral machine/device visitor to borrow display
capabilities. The machine core only enumerates configured slots and stamps
observation boundaries; it never switches on `DeviceId::Vga` or downcasts to
concrete chips. The assembly/frontend adapter renders card state into `Frame`
and resolves display routes by device role. The exact Rust method names are left to the
implementation; required operations are:

| Operation | Owner / contract |
| --- | --- |
| Query text state | `Device`, optional read-only capability; memory and register-derived state borrowed together |
| Bind semantic byte sink | Device byte-output capability; binding done once by assembly, producer calls the sink at emission |
| Capture selected displays at a boundary | Assembly observation facade, calls renderer once per route, returns/presents owned frames |
| Read captured stream bytes | One collector per route, explicit drain or final result; a frontend and test do not race to drain the device |
| Fan out frames and byte batches | Observation facade/hub, feeds capture and presentation subscribers from the same values |

Keep `as_any_mut` as a pragmatic bridge in the prototype and initial migration,
and for existing host keyboard injection. Replace output downcasts with the
capabilities once two producers are wired. Requiring every presenter to know
`VgaText`, `Uart16550` and `PostPort` would spread concrete-device coupling;
putting those downcasts into `Machine` would violate its swap seam. A save-state
blob is also unsuitable: it is versioned restoration data, not a scanout format.

### Sinks, bounds and lifecycle

The default byte sink discards output without allocating history. A capture sink
records the requested stream under an explicit byte limit; overflow is latched
and causes observation/golden failure, while guest hardware execution continues
unchanged. A live presenter can use a bounded host queue and report overflow.
No sink calls back into the machine, blocks on host I/O, changes interrupts, or
feeds host timing into execution. The hub can buffer or copy already captured
data for host I/O after a run boundary. Dropping intermediate **presentation
frames** is allowed; silently dropping captured serial/POST bytes is not.

Output buffering therefore cannot simply move from an unbounded trace `Vec` to
an unbounded device queue. Direct sink emission removes that queue; captures
remain explicitly bounded, selected and run-scoped. The UART has no
`output`/`take_output` buffer or compatibility adapter. Its snapshot v3 excludes
host output history and rejects the earlier formats, as specified in
§ UART output ownership and snapshot compatibility. Whole-machine snapshots
remain outside this output-ownership change.

Starting a fresh run clears capture state. Device reset preserves installed
routes/sink bindings; the facade explicitly decides when to start a new capture
epoch so earlier POST progress is not accidentally erased. A display restore is
observable immediately from restored state, without replay. Do not retroactively
emit already captured stream bytes on restore. Machine-wide replay remains
deferred; an eventual replay design must specify stream epochs/cursors.

`OutputConfig` should actually select the initial facade/presenter wiring.
`Headless` means no visible presentation; it must not disable state observation
or explicitly requested stream capture. `Capture` connects `CaptureFrameSink`
to the same rendered frames a window would receive. Store collection limits,
display-selection requests and capture cadence in an observation/run policy
beside `RunPolicy`, not as guest hardware identity. Do not change the guest's step bounds to service a window's repaint requests.

### Deterministic boundaries and assertions

Capture displays after a completed machine service boundary (including DMA),
and at every terminal path, including a CPU/bus error. At present the BIOS runner
already extracts output after classifying a failed run; preserve that behavior.
Initial assembly failure returns an explicitly absent/blank compatibility
result, not a claim that a card emitted a frame.

Use retired-instruction virtual time, including the existing halted PIT-clock
extension, to select capture boundaries. The current machine advances devices
before dispatching the CPU stop (`crates/machine/src/lib.rs`, `run_bounded`,
`advance_clock`, `tick_clock`); the spike does not change this order. Stamp a
capture with both retired count and virtual time. If streams need timestamps,
use the existing emission boundary's `VClock` and a stable sequence number to
disambiguate multiple emissions at one time. Do not invent instruction-precise
timestamps by interpolating between CPU blocks. Byte-only goldens ignore batch
boundaries and timestamps and compare concatenated bytes in emission order.

A frame query must not read guest status ports: VGA status reads advance its
retrace phase, and system-control reads advance a status phase. Repeated
observation must not change `Device::save`, traces, virtual time, retired count,
or subsequent guest execution. A UI may repaint the last owned frame on its own
host schedule; only deterministic emulator boundaries produce new observations.

Screen goldens should assert the typed `TextGrid` (dimensions, all glyph and
attribute bytes, cursor), plus the existing human-readable dump where useful.
Test `FrameSink` capture against that same frame. Existing dump encoding is kept
byte-identical. Stream goldens assert exact byte vectors; POST tests also prove
reads do not emit and multi-byte bus accesses retain existing byte-cycle
semantics. Unit tests explicitly enable debugging traces only when asserting
access behavior. An optional trace overflow fails a trace-specific assertion,
never changes screen or serial output.

### The MDA card console

The BIOS/FreeDOS preset now routes `0xB0000..=0xB7FFF` to `MdaText`, rather than
the CPU's RAM. The MDA card owns its CRTC ports (`0x3B0..=0x3B7`), mode/status
ports and unassigned display ports through `0x3BB` plus `0x3BF`. The printer
ports `0x3BC..=0x3BE` stay undecoded. VGA no longer receives monochrome aliases,
so writes to the two cards cannot share a CRTC register file. See IBM PC
Technical Reference, Monochrome Display and Printer Adapter, Programming
Considerations, and the device module's coverage matrix for assumptions.

`BiosRun.mda` captures the MDA route through `capture_text`; `BiosRun.screen`
remains the colour card. Prompt polling's `ConsoleProbe` selects the resolved
MDA bus slot and renders its `Device::text_scanout` with
`video::render_text_mode`. Both paths honour the card's start address and
cursor. They retain separate capture calls; unifying observation scheduling
and presentation belongs to later work. Missing scanout is an explicit run
observation error. The exact `poll_step` and injection schedule stay fixed.

The old RAM probe used cell zero and hid the cursor. Card capture exposes the
hardware cursor and the independent VGA CRTC keeps its own reset state. These
cursor changes are the intended correction to the shared-register artefact. MDA
status preserves the old per-read bit-0/bit-3 phases while its other bits read
high according to the card's documented open-bus assumption. BIOS writes now
trap as MMIO; the slow goldens' screen stamps, stop points, POST and COM1
streams are unchanged, so the execution boundaries did not move.

#### Changed expectations when the MDA card was wired

Cursor tuples below are `(column, row, visible)`. In every scenario the colour
card keeps its power-on cursor because the BIOS only programs MDA; MDA capture
now exposes the BIOS cursor that the RAM probe hid. Only cursor expectations
change; console text, attributes, POST, COM1, media, execution counters and stop
points retain their existing checks.

- BIOS POST: colour `(0, 18, true)` → `(0, 0, true)`;
  MDA `(0, 0, false)` → `(0, 18, true)`.
- BIOS floppy boot: colour `(0, 17, true)` → `(0, 0, true)`;
  MDA `(0, 0, false)` → `(0, 17, true)`.
- FreeDOS prompt: colour `(31, 24, true)` → `(0, 0, true)`;
  MDA `(0, 0, false)` → `(31, 24, true)`.
- DIR: colour `(4, 24, true)` → `(0, 0, true)`;
  MDA `(0, 0, false)` → `(4, 24, true)`.
- File write: colour `(4, 24, true)` → `(0, 0, true)`;
  MDA `(0, 0, false)` → `(4, 24, true)`.

The text-attribute decoding rebase required explicit decoded styles in assembly
unit-test cells, but no further golden expectation changes. The cursor changes
listed above remain the only changed golden expectations for this wiring.

### Traces and later audio

Implemented: `TracePolicy::Off` by default, plus an explicitly sized ring with
deterministic overwrite-oldest behavior, retained-order drain and a dropped-event
count. This covers both device access buffers and `Bus::open_bus_trace`;
`SharedPic` delegates to its existing inner device rather than recording again.
The shared recorder enforces the same capacity/overflow contract everywhere.
Trace tests explicitly select sufficient capacity or assert zero drops before
checking sequences. Off avoids recording allocations altogether. Debugger
filters remain deferred until needed.

The trace bound must not affect semantic streams or current scanout. Turning
tracing off, draining it early or overflowing its ring must yield the same
frames and byte streams. Diagnostic traffic is allowed to be incomplete when
its dropped count says so; golden output is not reconstructed from it.

Later sound needs a deterministic producer from the *actual* speaker/PIT state,
with fixed rational virtual-time conversion and either timestamped signal edges
or fixed-format PCM blocks. Capture and host audio presentation must share that
producer. Pin sample rate, channel count, sample encoding and rounding/resampler
phase in the observation identity; preserve phase across block boundaries.
Host playback may underrun or discard stale presentation blocks without changing
emulated time or the golden samples. None of the speaker hardware or audio
format is implemented by this spike.

## Migration with A/B gates

1. Record current outputs under unchanged ROM/media bytes, CPU revision, machine
   configuration and run policy. Characterise all result fields, not just current
   assertions: the BIOS test currently leaves `screen` unchecked. Pin both full
   frames and legacy dumps so cursor/attribute changes are visible.
2. Add observation capabilities and wire one facade, retaining the old path as
   an A/B oracle. Compare *within the same run*, taking the non-destructive state
   snapshot before the old trace drain. Keep full trace recording only for this
   temporary comparison. Do not run a second guest with different stop timing.
3. Switch VGA capture to current state after checking full grid equality on the
   VGA golden. For the BIOS record the measured cursor discrepancy separately:
   preserve all existing golden bytes and assertions, and add a new explicit
   state-based cursor expectation. Do not reproduce the replay bug in the card.
   If a caller requires exact legacy `BiosRun.screen` compatibility, expose the
   canonical frame separately and retain a labelled legacy cursor projection
   until that caller migrates. That adapter is not used by new presenters.
4. Bind UART and POST semantic sinks; A/B exact concatenated streams against
   existing UART `take_output` and POST trace filtering. Exercise DLAB writes,
   repeated bytes, zero/`0xFF`, POST reads, resets, early drains and error stops.
   Do not switch bounded tracing off while POST still depends on it.
5. Use MDA card scanout for prompt polling, as described in §The MDA card console.
   Compare frames at the same existing polls, with no extra `run_bounded` calls. Re-run BIOS,
   FreeDOS boot, interactive DIR and DOS write-session goldens, checking unchanged
   text, markers, POST and stop kinds. Route `FrameSinkKind` through the facade;
   ensure headless and capture modes yield identical machine results.
6. **Trace migration done:** production output no longer uses replay/downcasts;
   access traces are opt-in and bounded. The slow regression compares tracing
   Off against tiny overflowing rings across all five BIOS/DOS golden workloads.
   Keep bounded-oracle mode only in tests if it still serves a diagnostic
   purpose. No committed golden fixture is rewritten merely to accommodate an
   observation implementation change.

This is a gate on **existing asserted bytes**, not a claim that the old and new
observer are equivalent for arbitrary histories. Monochrome/index aliases (the
monochrome ones since removed from the colour card),
repeated capture and restore deliberately expose errors the old path cannot
represent. Correct-state tests supplement the unchanged golden corpus.

## Prototype evidence

MEASURED against the CPU revision `b0c9ae52820dffc1e2cb75c4928a670788a916d0`
already pinned in `Cargo.lock`. No dependency pin, preset, guest input, run budget
or golden fixture was changed.

The disposable `crates/devices/src/vga_text.rs` change only opts into
`Device::as_any_mut`. In `crates/cli/src/lib.rs`, `render_vga` builds
`TextModeState` from the live card's getters. The original function is retained
as `render_vga_trace`, called after capture to compare all cells, dump and cursor.
The returned grid is the state-based one. Thus the prototype still allocates and
drains traces for measurement; it does **not** solve trace memory growth.

| Run | Full grid equality | Text dump equality | Differing glyph/attribute cells | State cursor / replay cursor |
| --- | --- | --- | --- | --- |
| `crates/cli/tests/vga_text_golden.rs` | Yes | Yes | 0 of 2,000 | `(0, 0, visible)` / `(0, 0, visible)` |
| `bios_boot_golden::bios_boot_stop_point_matches_golden`, fetched 32 KiB BIOS, existing golden cap | **No** | Yes | 0 of 2,000 | `(0, 18, visible)` / `(0, 0, visible)` |

The BIOS rows are blank in the colour window; its console is in the separate
RAM probe. The cursor difference does not indicate missing colour glyphs.

Three regression tests were added **before changing production behavior** and
run against the original implementation:

| Test | Before | After / what it proves |
| --- | --- | --- |
| CLI `vga_snapshot_survives_trace_drain` | Fails: glyph 0 instead of `A` | Passes: glyph/attribute survive an earlier trace drain and two snapshots agree |
| CLI `vga_snapshot_uses_card_crtc_aliases` | Fails: glyph 0 instead of `A` | Passes: mono ports plus the card's index alias set start=1, cursor=3 and disabled cursor; the rendered glyph is `A`, cursor `(2, 0, hidden)` |
| Device `host_snapshot_reads_live_state_without_guest_accesses` | Fails: VGA host observation capability absent | Passes on `testkit::MockBus`: writes through mono ports and an aliased index are visible via getters after trace drain; observing generates no guest-access trace |

This table records the tests as they were added; the colour card's monochrome
aliases were later removed when the MDA card was wired, and the fixtures were
renamed or reduced to colour ports and index aliasing. The alias fixture combined
monochrome port and register-index aliasing; it did
not independently isolate each alias rule. Existing device tests cover register
and port behavior, while this fixture checks that the frontend uses their result.

Validation commands use the supplied Nix devShell environment and offline Cargo.
Results and remaining unmeasured migration gates are recorded below; no broad
slow-suite success is inferred from a named BIOS run.

| Command (after loading the supplied devShell environment) | MEASURED result |
| --- | --- |
| `which cc cargo` | Both resolve under `/nix/store` |
| `cargo test --offline -p cli --lib vga_snapshot` before implementation | Expected failure: 0 passed, 2 failed, 1 filtered out; both failed glyph assertions read 0 instead of 65 |
| `cargo test --offline -p devices --lib host_snapshot_reads_live_state_without_guest_accesses` before implementation | Expected failure: 0 passed, 1 failed, 209 filtered out; downcast capability absent |
| `cargo fmt --all` | Exit 0; formats prototype only |
| `cargo test --offline -p cli --lib vga_snapshot -- --nocapture` after implementation | 2 passed, 0 failed, 1 filtered out |
| `cargo test --offline --workspace` | Exit 0: 329 passed, 0 failed, 5 ignored; additionally 2 passing watchdog subprocess cases (331 passing result entries if those are counted twice) |
| `cargo test --offline -p cli --test bios_boot_golden bios_boot_stop_point_matches_golden -- --ignored --exact --nocapture` | 1 passed, 0 failed; ROM present, A/B executed; test duration 2.82 s |
| `cargo test --offline -p cli --test vga_text_golden -- --nocapture` | 2 passed, 0 failed; A/B executed |
| `cargo build --offline --all-targets` | Exit 0 |
| `cargo clippy --offline --all-targets --all-features -- -D warnings` | Exit 0, no warnings |
| `cargo fmt --check` | Exit 0 |
| `git diff --check` | Exit 0 |

This is historical prototype evidence. At that stage the complete ignored suite
had not run, and FreeDOS/session byte equivalence, restore equivalence and
tracing-off/overflow independence were still gates. The implemented trace policy
and regression above now cover the trace gate; audio remains deferred. No
prototype timing number above measures the bounded recorder.

## Proposed implementation beads

These are titles and scopes for the supervisor to file or fold into existing
work. They are a dependency order, not a second task board. Existing bead scopes
here come from the spike brief; the tracker was not opened by this worker.

| Order | Proposed title | One-line scope / relationship |
| --- | --- | --- |
| 1 | Define typed output capabilities and observation routes | Add read-only text state and byte-sink capabilities, resolve/validate routes by role, and test unsupported routes and passive observation. |
| 2 | Capture VGA goldens from the card's scanout state | Absorb `upc-x83.1`: remove CLI CRTC/VRAM replay, A/B existing VGA and BIOS outputs, add full-grid/cursor and repeat-capture regressions. Depends on 1. |
| 3 | Capture UART and POST through semantic byte sinks | Preserve exact COM1/POST streams, DLAB and byte-cycle rules; bound explicit captures, define reset/overflow ownership and retain UART snapshot compatibility. Depends on 1. |
| 4 | Share console observation across run policies and presenters | Unify MDA card observation across prompt polling and final capture, wire `OutputConfig`/`FrameSinkKind`, fan out the same frames to capture and presentation. Depends on 2 and 3. |
| 5 | Bound and disable access traces independently of output | Expand `upc-kk2.7` to all devices plus open-bus recording, add off/ring policies and drop counts, opt trace tests in, prove identical output with tracing off/overflowing. Depends on 2 and 3; final end-to-end gate also uses 4. |

Audio hardware and a window implementation remain follow-up consumers, not
prerequisites for this seam. Do not add a speaker model or GUI as part of these
observation fixes.
