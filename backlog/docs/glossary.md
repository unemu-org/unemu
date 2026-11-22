# Glossary

unemu maintains application terms; unemupc maintains hardware terminology and execution details.

Domain and project terminology. One term per entry. Keep definitions short — link out for depth.

## Project

- **unemu** — this desktop and command-line front end, with library, registry, media and recipes.
- **unemupc** — the PC core, pinned by git revision; owns configuration and execution.
- **unemu-core-api** — the external normative contract shared by all cores and front ends.

- **LLE** — low-level emulation: modelling hardware at the device/register level
  (vs high-level emulation, which shims APIs).
- **x86jit** — the external x86 recompiler used as the CPU core; a separate project
  (beads prefix `xj`), consumed as a git dependency pinned to an exact revision of its `main`.
- **bead** — one unit of open work in the beads (`bd`) tracker; front-end ids look like `ufe-12.3`
  (the dotted suffix names the parent epic). Foreign ids carry a tag: `xj-6vd.9 (x86jit)`.
- **seam** — a self-contained swap boundary or capability with its own tests; also
  the unit of merge back to x86jit `main` (see [decision-2 (unemupc)](https://github.com/unemu-org/unemupc/blob/main/backlog/decisions/decision-2%20-%20CPU%20core%20via%20x86jit%20git%20worktree%20and%20merge%20discipline.md)).
- **swap seam** — one of the six trait boundaries (`Cpu`, `Chipset`, `Bus`,
  `Device`, `FrameSink`, tracing) that make hardware replaceable ([decision-7 (unemupc)](https://github.com/unemu-org/unemupc/blob/main/backlog/decisions/decision-7%20-%20Swappability%20via%20Cpu%20Chipset%20Device%20FrameSink%20traits.md)).
- **virtual clock / vclock** — deterministic time base measured in retired
  instructions ([decision-5 (unemupc)](https://github.com/unemu-org/unemupc/blob/main/backlog/decisions/decision-5%20-%20Virtual%20clock%20in%20retired%20instructions.md)), not wall-clock, not JIT blocks.
- **golden test** — a test comparing captured output/trace against a committed
  reference; front-end goldens verify the locked PC-core revision.
- **MMIO** — memory-mapped I/O; in x86jit a `RegionKind::Trap` region whose
  accesses exit as `Exit::MmioRead`/`MmioWrite`.
- **PortIo** — ISA port I/O (`IN`/`OUT`); `Exit::PortIo` + `complete_port_in`.
- **IVT** — real-mode Interrupt Vector Table at physical `0`; where `INT n` and
  hardware interrupts vector.
- **INTA / irq_ack** — the interrupt-acknowledge cycle: the CPU asks the 8259 PIC
  for the pending IRQ vector.
- **note_write** — the x86jit hook that invalidates translated blocks on a guest
  memory write; all RAM writes must go through it ([decision-6 (unemupc)](https://github.com/unemu-org/unemupc/blob/main/backlog/decisions/decision-6%20-%20Guest%20memory%20writes%20via%20note_write%20for%20SMC%20safety.md)).
- **SingleStepTests / ProcessorTests** — silicon-derived per-instruction test
  corpus used as the real-mode CPU oracle.

## Audio

- **PCM frame** — one pair of signed i16 left/right samples at 48 kHz; four
  little-endian bytes in captures. See [audio design](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/design/audio.md).
- **Zero-order hold** — resampling by keeping each native sample constant until
  its next virtual-time boundary; deterministic but without anti-alias filtering.

- **PCM clock area** — the exact signed sum of sample level times PIT-clock
  duration; source areas are summed before one division to output PCM.

## Backlog.md

- **task-N** — a Backlog.md task id (`task-1`, …; prefix set by `task_prefix`).
- **status** — a task's column: `To Do`, `In Progress`, `Done`.
- **decision** — an architecture record in `backlog/decisions/` (an ADR).
