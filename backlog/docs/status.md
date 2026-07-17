# Status

Snapshot of what works, what's in flight, what's broken. **Not the roadmap** — roadmap lives in Backlog.md tasks (`backlog task list --plain`).

Update this when a feature lands, breaks, or gets pulled. Stale status is worse than no status — if you can't keep it fresh, link straight to `backlog task list` filters instead.

## Works

- Bootstrap, architecture docs, ADRs, roadmap (`task-1`).
- Walking skeleton (`task-2`): Cargo workspace with the `cpu` / `bus` / `chipset` /
  `device` seams, an x86jit Long64 backend, a 16550 UART, and a deterministic
  golden where a guest writes "hello" to COM1 over port I/O. `cargo test`: 18 pass.

## In flight

None. Next up: real mode in x86jit (`task-3`).

## Broken / regressions

None.

## Not started

See `backlog task list --plain` (or `backlog board`) filtered by label/milestone. Don't duplicate the task list here.
