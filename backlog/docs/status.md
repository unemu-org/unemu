# Status

Snapshot of what works, what's in flight, what's broken. **Not the roadmap** — roadmap lives in Backlog.md tasks (`backlog task list --plain`).

Update this when a feature lands, breaks, or gets pulled. Stale status is worse than no status — if you can't keep it fresh, link straight to `backlog task list` filters instead.

## Works

- Bootstrap, architecture docs, ADRs, roadmap (`task-1`).
- Walking skeleton (`task-2`): Cargo workspace with the `cpu` / `bus` / `chipset` /
  `device` seams, an x86jit Long64 backend, a 16550 UART, and a deterministic
  golden where a guest writes "hello" to COM1 over port I/O. `cargo test`: 18 pass.

## In flight

- Real mode in x86jit (`task-3`): sub-seams (a) addressing, (b) INT/IRET/IVT/IF,
  (c) IRQ injection + retired-instruction counter — all merged to the x86jit
  `pc-emulator` branch (differential cf16 16/16 vs Unicorn MODE_16). Graduating
  `pc-emulator` → x86jit `main` (PS4 benchmark + churn cleanup) is a later gate.
- Nothing actively in progress. Next candidates: `task-8` (close the real-mode
  lift gaps the 8088 corpus found), `task-9` (separate 8088-vs-286 differences
  from real failures), M4 devices + VGA text (`task-5`).

## Recently landed

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
