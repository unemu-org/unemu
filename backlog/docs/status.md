# Status

Snapshot of what works, what's in flight, what's broken. **Not the roadmap** — roadmap lives in Backlog.md tasks (`backlog task list --plain`).

Update this when a feature lands, breaks, or gets pulled. Stale status is worse than no status — if you can't keep it fresh, link straight to `backlog task list` filters instead.

## Works

- Bootstrap, architecture docs, ADRs, roadmap (`task-1`).
- Walking skeleton (`task-2`): Cargo workspace with the `cpu` / `bus` / `chipset` /
  `device` seams, an x86jit Long64 backend, a 16550 UART, and a deterministic
  golden where a guest writes "hello" to COM1 over port I/O. `cargo test`: 18 pass.

## In flight

- Nothing actively in progress. Next candidates: M4 devices + VGA text (`task-5`),
  `#UD`-in-guest delivery + segment-limit `#GP` (task-11), `test386.asm` (`task-7`).

## Recently landed

- **Real-mode fault delivery on x86jit `main`** (`task-11`) @ cbba7d9: in-guest
  `#UD` (invalid-on-286 opcodes), `BOUND`/`#BR`, and the 80286 segment-limit /
  instruction-length `#GP` — all Real16-gated. 80286 corpus jumped to **95.98%
  decided / 92.91% exception delivery**; no Long64 regression.
- **M4 devices + first graphics** (`task-5`, device level): 8042 keyboard, MC146818
  CMOS/RTC, 8237 DMA (via a `note_write`-safe `DmaMemory` handle), and VGA text
  mode as a `Device` behind the new `FrameSink` seam (`video` crate) with a
  rendered-frame golden. All isolated + unit/golden tested; workspace 145 tests
  pass. Machine wiring (keyboard→IRQ1, VGA→sink) is the integration follow-up.
- **Real mode graduated to x86jit `main`** (`task-3`) @ 40acdbc: the full
  real-mode (16-bit) seam — addressing, INT/IRET/IVT/IF, IRQ injection, the
  complete instruction set — now lives on x86jit main, entirely gated so the
  Long64/Compat32 (PS4) paths are byte-identical. `pc-emulator` is fast-forwarded
  to main; no drift. 80286 corpus 89.7%/95.2%, cf16 34/34, PS4 jit suite intact.
- SingleStepTests/80286 authoritative silicon oracle (`task-10`) — our exact
  target CPU, now the **sole** real-mode oracle on x86jit `main`: 1,478,000 tests,
  **89.7% overall / 95.2% decided** after the lift-gap work. The redundant 8088
  oracle (`task-6`) was retired — it produced false failures on 8088-vs-286
  differences and the 286 corpus supersedes it.

- Timer-interrupt boot (`task-4`): the real-mode machine takes an IRQ0 timer
  interrupt via the in-guest IVT and prints to serial. `AtChipset` = 16550 UART +
  8259 PIC + 8253 PIT; PIT OUT → edge-detect → PIC → `inject_irq`; deterministic
  virtual clock from the retired-instruction counter. Boot golden passes.

## Broken / regressions

None.

## Not started

See `backlog task list --plain` (or `backlog board`) filtered by label/milestone. Don't duplicate the task list here.
