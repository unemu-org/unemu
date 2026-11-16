# unemupc specification

A growing, numbered specification for the emulator. Doc-comments in the source
cross-reference these sections by number (`§3.2`), the way x86jit's own
`backlog/docs/design/spec.md` does. This is a **living document** — sections are
added as subsystems are designed and built. As of M0 it is an outline; the
detailed prose is filled per milestone.

Related: [`../architecture.md`](../architecture.md) (current shape),
[`../testing.md`](../testing.md) (test harness), `../../decisions/` (the why).

## §1 Scope

§1.1 Target machine: IBM PC/AT-class (Intel 80286), real-mode boot.
§1.2 Priorities: testability, swappability, documentation (research project).
§1.3 Out of scope for now: protected mode, paging (386+), APIC, PCI, real BIOS,
graphics beyond serial. See [`../deferred.md`](../deferred.md).

## §2 CPU core (x86jit)

§2.1 Integration surface that works today (`RegionKind::Trap`, `Exit::PortIo`,
`Backend`, exact block counter).
§2.2 Real-mode gap: segment registers, `IF`, `segment:offset` addressing,
IVT-vectored `INT`/exceptions — added under `CpuMode::Real16`.
§2.3 ABI stability: `CpuState` is `#[repr(C)]`; append-only; offset-assertion test.
§2.4 Worktree + merge discipline (mergeability criteria).

## §3 Machine assembly

§3.1 `Config` → `Machine` composition root.
§3.2 Swap seams: `Cpu`, `Chipset`, `Bus`, `Device`, `FrameSink`, tracing.
§3.3 Chipset declares the decode map; Bus executes it.

## §4 Memory & bus

§4.1 Address-space and port-space decode.
§4.2 RAM / MMIO (`Trap`) / ROM regions.
§4.3 Writes via `note_write` (SMC safety) — DMA, BIOS loader, shadow RAM.
§4.4 Board-driven A20 gate: the effective line is the OR of 8042 output-port
bit 1 and port 92h bit 1. A disabled line clears physical address bit 20. CPU-only reset retains the line; host
physical-memory access and ISA DMA bypass it. Source-free bare rigs stay flat.
See [machine-config.md §A20 line and System Control Port A](machine-config.md#a20-line-and-system-control-port-a).

## §5 Interrupt model

§5.1 IRQ lines → 8259 PIC → INTR.
§5.2 `inject_irq` contract: between-`run()` only; deferred while pending completion.
§5.3 STI shadow, HLT wakeup.
§5.4 INTA / `irq_ack() -> vector`; IVT dispatch.

## §6 Virtual clock

§6.1 Completed-step time base, including REP iterations (not blocks).
`device::clock` defines exact rational clock domains on the `VClock` timeline.
One tick remains one PIT input clock (`TICKS_PER_PIT_CLOCK = 1`);
`VClock::from_pit` converts PIT durations. The single board crystal is OSC =
315/22 MHz; PIT is OSC/12 and OPL3 samples are OSC/288. Independent domains
cover CGA/VGA dots, UART, RTC, audio output, and AT 12/16 MHz CPU crystals.
`edges_between(a,b)` counts input edges in `(a,b]`; `edge_at` rounds upward,
and `next_edge_after` retains epoch phase without adding rounded periods.
See [`architecture.md` §Virtual clock](../architecture.md#virtual-clock) for
constants, overflow contracts and the distinction from CPU instruction rates.
§6.2 Step bounds, instruction-start accesses and IRQ-readiness ordering; see
[`scheduler.md`](scheduler.md#adoption) for the budget-independence contract.
§6.3 Golden pinning + re-baseline procedure.

## §7 Devices

§7.1 `Device` contract (mmio/port, `tick`, IRQ, `reset`, `save`/`restore`, trace).
§7.2 16550 UART (serial) — first device.
§7.3 8259 PIC. §7.4 8253 PIT. §7.5 8042 keyboard controller.
The 8042 optionally owns a byte-level `AuxDevice`; `Ps2Mouse` implements the
standard ID-00 pointing-device protocol (IBM Personal System/2 Hardware Interface
Technical Reference, Common Interfaces, "Keyboard and Auxiliary Device Controller"
and "Pointing Device"). The shared output buffer distinguishes keyboard and aux
provenance; device interrupt outputs 0/1 expose IRQ1/IRQ12 without board wiring.
Clock inhibit retains queued aux bytes, controller replies precede serial traffic,
and simultaneously ready keyboard/aux bytes alternate. Mouse host motion uses
counts with positive Y up; stream-only 2:1 scaling and remote/wrap modes are
supported. Sample rate is stored/reported, with stream packets generated at
injection boundaries rather than paced in virtual time. Controller snapshot v6
includes the attached peripheral identity and complete protocol state. Machine
configuration and frontend/PIC routing are separate work.

§7.6 CMOS/RTC. §7.7 8237 DMA. §7.8 µPD765 floppy controller (reads via §7.7 DMA).

## §8 Firmware

§8.1 Minimal BIOS stub (IVT setup + serial print).
§8.2 BIOS blob loader (later); 286 fidelity limits for a real BIOS.

## §9 Video

§9.1 `FrameSink` seam; cards are Devices.
§9.2 VGA text 80×25 (first graphics mode, later).
§9.3 MDA text 80×25. The MDA display device exposes fixed 80×25 text scanout, 4 KiB of RAM mirrored
through B0000–B7FFF, the MC6845 register file through the 3B0–3B7 port mirrors,
and mode/status ports 3B8/3BA. Status uses a deterministic read-count phase.
The shared text renderer decodes monochrome attributes into non-display,
underline, normal and reverse-video effects with character intensity. Mode-control
bit 5 at 3B8 selects blink versus background intensity; non-display stays black.
Glyph rasterisation and blink timing remain presenter concerns. The BIOS/FreeDOS
profiles route B0000–B7FFF, display ports 3B0–3BB and port 3BF to the MDA card.
The printer ports 3BC–3BE are not routed.

§9.4 Optional host presenter (`window` crate). It subscribes a boxed `FrameSink`
to the assembly observation facade; no test-path crate depends on it. The pure
rasteriser embeds the unmodified Ultimate Oldschool PC Font Pack v2.2 by VileR
(CC BY-SA 4.0), parsing MZ/NE RT_FONT resources and fixed-width FNT v2/v3
column-major glyphs into 256-entry CP437 atlases. MDA cells use the 9×14 font;
colour cells use 8×16 and the standard 16-colour CGA/VGA text palette. Output is
row-major `0x00RRGGBB`, replicated by a nonzero integer scale. Buffers larger
than 16,777,216 pixels are rejected. MDA normal foreground is `AAAAAA`, intense
foreground `FFFFFF`, background black; background intensity is `555555`, or
`FFFFFF` behind reverse video. Reverse foreground intensity leaves black ink
black and normal grey paper unchanged (attribute 78h); only background intensity
brightens reverse paper. Non-display remains black. Underline is scanline 12
(zero-based). The visible cursor uses the cell foreground on BIOS default rows
11–12 for MDA and 13–14 for VGA. The programmed CRTC cursor start/end registers
are not yet exposed by `TextModeState`. Blink is steady pending timed
presentation. No programmable fonts, cursor shape, phosphor simulation,
aspect correction, graphics or host input.

The winit/softbuffer binary assembles the FreeDOS BIOS preset, runs fixed
100,000-unit virtual instruction-equivalent chunks (including halted virtual
time) and captures MDA after each chunk before painting. An effectively unlimited
idle wait (`set_idle_limit(Some(u64::MAX))`) lets HLT wait for IRQs; each chunk's
absolute virtual-time cap still bounds execution. It reads no host clock,
spawns no threads and never ticks devices itself. The FreeCOM date prompt, a
natural halt or a CPU failure stops advancement; the last screen remains visible.
Headless capture is still the default. Font provenance is recorded in
`crates/window/assets/fonts/NOTICE.md` and in the binary's `--help`, `--version`
and `--about` output; build/run commands are in `commands.md`.

## §10 Testing

Mirrors [`../testing.md`](../testing.md): unit, fuzz/property, coverage matrix,
boot/integration, golden/trace, differential (SingleStepTests), snapshot/replay.
