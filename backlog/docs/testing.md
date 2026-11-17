# Testing strategy

Testability is the top project priority. This document is the test-harness
design; implementation arrives with the crates it describes. Cross-reference the
numbered spec in [`design/spec.md`](design/spec.md).

## Principles

- **No wall-clock, ever, in the execution path.** Virtual time is retired
  instructions (see [`architecture.md`](architecture.md) clock model). This is
  what makes goldens reproducible.
- **Every device is testable without a CPU.** Devices talk to a mock bus.
- **Every machine feature has a deterministic end-to-end test.**
- **Golden traces pin the x86jit revision** — a golden is only valid against a
  known CPU codegen.

## Test levels

### 1. Device unit tests (no CPU)

Each device driven in isolation on a **mock bus**: a scripted sequence of
port/MMIO reads and writes, asserting internal state and IRQ-line transitions.
Fast, exhaustive, the default level. Lives next to each `devices/*` crate.

### 2. Device fuzzing + property-based

- **Fuzzing**: random sequences of port/MMIO writes → the device never panics and
  its documented invariants hold.
- **Property-based**: encode datasheet invariants, e.g. PIT counter period ==
  programmed divisor; 8259 priority resolution after EOI; UART THRE/DR flag
  transitions. Tooling: `proptest` (candidate; confirm with a decision when added).

### 3. Hardware coverage matrix

A per-chip document: **what the datasheet defines vs what we model vs what we
skip**. 8259 modes (fixed/rotating priority, special mask, poll, AEOI…), 8253's
six counter modes, 16550 FIFO behaviour, etc. This is a research artifact and a
guard against silent simplifications. Kept alongside each device or in
`design/spec.md`.

### 4. Boot / integration tests

Small hand-assembled programs (real mode, or long mode for the M1 skeleton) run
end-to-end through `cli`/`machine`: program a device, take an interrupt, write to
serial → assert the captured serial output. Built with `nasm` + `objcopy`
(flat binaries) from the devShell.

Project-owned guest programs that need firmware are booted from a floppy
instead. Their source is committed next to a build script under `vendor/`,
the built image is gitignored and pinned by SHA-256, and the CI `fixtures`
gate builds it. The first is the open AT protected-mode return test
([`design/at-pm-test.md`](design/at-pm-test.md)): 80286 protected-mode entry,
the 8042 and triple-fault returns through the CMOS shutdown byte, A20
through the 8042 and the BIOS's INT 15h AH=88h and AH=87h, reported line
by line on COM1. The open AT LOADALL test
([`design/at-loadall-test.md`](design/at-loadall-test.md)) moves a block
above 1 MiB and back from real mode through the 80286 LOADALL, as a 286
extended-memory manager does.

### 5. Golden / trace tests

Run a ROM; dump serial output + a device-state trace; compare against a committed
golden (RON / plain text). Deterministic via the virtual clock. Goldens pin the
x86jit revision; the re-baseline procedure lives in the clock decision. The trace
comes from the first-class tracing seam, not ad-hoc printlns.

### 6. Differential oracles

- **CPU-level, real mode**: validate against the **SingleStepTests /
  ProcessorTests** corpus (8088 / 80186 / 80286 / V20 — JSON per instruction,
  before/after CPU+memory state, generated from real silicon). Brutally accurate;
  the right oracle for the M2 real-mode seam. Unicorn in 16-bit real mode is a
  second reference (x86jit already has a Unicorn harness). **The existing long64
  suite does NOT cover real mode — do not lean on it here.**
- **Machine-level (optional)**: run the same ROM under QEMU/Bochs and compare
  **final state / serial output** — not a timing-sensitive trace (traces aren't
  1:1 comparable across emulators).

### 7. Snapshot / replay

`Device::save` / `restore` (in the trait from day one) enable record/replay:
snapshot machine state, replay an input log for reproducible debugging.

## Toolchain (devShell)

- `cargo test` — unit, property, golden, integration.
- `nasm` + `binutils` — build guest test-ROMs.
- SingleStepTests / ProcessorTests corpus — real-mode CPU oracle (fetched, not vendored).
- `qemu` — optional final-state differential oracle.

## Host filesystem assumptions

CI (`.github/workflows/ci.yml`) runs the tests as the non-root `runner` user on
an x86-64 Linux VM, but a developer or another CI may run them **as root**.
Root ignores permission bits, so a test that removes access with `chmod` must
first probe the denial (try the access) and, when it succeeds, skip only the
checks that need it with an `eprintln!` naming the reason; the rest of the test
still runs. Never `#[ignore]` such a test.

Linux also stamps file times from a coarse clock (the timer tick), so a
same-size rewrite within one tick keeps size, inode, mtime and ctime. Tests must
not rely on a quick rewrite changing metadata: set a distinct mtime explicitly,
or, for the firmware hash cache, inject the stale entry or the hash time through
the cache's test seams.

## Fetched fixtures and their source

Firmware and media fixtures are fetched or built by `vendor/*/fetch.sh` (or
`build.sh`) and pinned by SHA-256 in those scripts; tests read the pins from
the scripts, so a test and its fixture cannot disagree silently. A missing
fixture skips its test unless `UNEMUPC_REQUIRE_FIXTURES=1`, which the CI
`goldens` gate sets (see [`commands.md`](commands.md)).

Every test that reads a vendor fixture goes through the fixture helper
(`fixture()` in `crates/pc-harness/tests/support/fixtures.rs`, which other crates'
tests include by `#[path]`), and checks it before anything else needs the
file. That includes tests that import a shipped machine file under
`machines/`, which names vendor images. CI's `tests` step runs before its
`fixtures` step builds them, so a test that reads `vendor/` directly fails
there instead of skipping.

The open GPL/LGPL firmware fixtures (Kiselev BIOS, XTIDE Universal BIOS, the
VGA BIOS fork) also get a deterministic source archive each, pinned the same
way. In `crates/pc-harness/tests/media_fixtures.rs`, a default-suite test checks that
every archive pin parses. An ignored test, run by the `goldens` gate with
`UNEMUPC_REQUIRE_FIXTURES=1` after the `fixtures` gate has built the
archives, checks each archive's hash, its top-level entries, its `NOTICE` (the
committed `source-notice.txt`), that it holds no prebuilt image (`.bin`,
`.rom`) or excluded upstream path, and that no member carries host data
(owner, owner names, per-file times, links). The fixture scripts themselves
rebuild a shipped image from each Kiselev and XTIDE archive's staged source
before writing it.

## What "done" means for a testable unit

- New device: unit tests + fuzz target + its row in the coverage matrix.
- New machine feature: a golden or integration test that exercises it end-to-end.
- New x86jit real-mode instruction: passes the SingleStepTests corpus for that opcode.
