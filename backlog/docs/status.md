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

- **Real mode graduated to x86jit `main`** (`task-3`) @ 40acdbc: the full
  real-mode (16-bit) seam — addressing, INT/IRET/IVT/IF, IRQ injection, the
  complete instruction set — now lives on x86jit main, entirely gated so the
  Long64/Compat32 (PS4) paths are byte-identical. `pc-emulator` is fast-forwarded
  to main; no drift. 80286 corpus 89.7%/95.2%, cf16 34/34, PS4 jit suite intact.
- SingleStepTests/80286 authoritative silicon oracle (`task-10`) on the x86jit
  `pc-emulator` branch — our exact target CPU: 1,478,000 tests, **79.3% overall /
  91.9% decided**. Surfaced real target-CPU gaps (LOCK mis-decode, missing
  segment-limit `#GP` → `task-11`, under-delivered `#UD`, unlifted opcodes →
  `task-8`). Being closed by the lift-gap effort now in flight.
- TomHarte 8088 silicon oracle (`task-6`): 1,824,000 tests, 83.6% / 92.9%. Kept as
  a secondary regression net for shared 8086/8088 behaviour.

- Timer-interrupt boot (`task-4`): the real-mode machine takes an IRQ0 timer
  interrupt via the in-guest IVT and prints to serial. `AtChipset` = 16550 UART +
  8259 PIC + 8253 PIT; PIT OUT → edge-detect → PIC → `inject_irq`; deterministic
  virtual clock from the retired-instruction counter. Boot golden passes.

## Broken / regressions

None.

## Not started

See `backlog task list --plain` (or `backlog board`) filtered by label/milestone. Don't duplicate the task list here.
