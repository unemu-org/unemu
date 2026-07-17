# unemupc98

A low-level emulator (LLE) of an **IBM PC/AT-class (Intel 80286) personal
computer**, built around the [`x86jit`](../x86jit) x86 JIT as its CPU core.

This is a **research project**. Its priorities, in order:

1. **Testability** — every device is exercised in isolation on a mock bus;
   every machine-level feature has a deterministic golden/integration test.
2. **Swappability** — CPU cores, chipsets, individual devices, and graphics
   output sit behind traits, so hardware can be added or replaced without
   touching the machine core.
3. **Documentation** — dense doc-comments in the source (modelled hardware +
   spec cross-references) and a curated knowledge tree under `backlog/docs/`.

The first end-to-end target is **headless serial**: a guest program that writes
to a 16550 UART, captured deterministically for golden tests. Graphics
(VGA text, framebuffer) come later as swappable graphics modes.

## Status

Bootstrapping. No emulator code yet — the architecture, collaboration rules,
and test harness are being designed. See:

- `backlog/docs/architecture.md` — layers, swap seams, interrupt/clock/memory models
- `backlog/docs/testing.md` — the test-harness strategy
- `backlog/docs/design/spec.md` — the growing numbered specification
- `backlog/decisions/` — architecture decision records (ADRs)
- `backlog task list --plain` — the roadmap (milestones M0–M4)

## CPU core: x86jit

The CPU is not vendored. `x86jit` lives in its own repository and is developed
for this project inside a dedicated **git worktree** on a `pc-emulator` branch,
consumed as a pinned git dependency. Real-mode support (segments, `IF`, IVT,
hardware IRQ injection) is added there under `CpuMode::Real16`, then merged back
to `x86jit` main under strict criteria so the primary consumer (PS4 emulation)
is never disturbed. See `backlog/decisions/` for the merge discipline.

## Getting started

```sh
nix develop        # Rust toolchain + backlog + nasm + qemu, or let direnv auto-load
cargo test         # (once crates exist)
backlog task list --plain
```

## Documentation for agents

`AGENTS.md` (and its Claude-facing alias `CLAUDE.md`) is the entry point for
coding agents: collaboration rules, the pointer table into `backlog/docs/`, and
the hard rules (testability-first, swappability-first, docs-first, determinism,
x86jit merge discipline).

## License

[AGPL-3.0-or-later](LICENSE).
