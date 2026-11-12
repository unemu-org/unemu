# Machine loop wall-time profile

Measured on 2026-11-09, macOS 27.0.1, arm64, Rust 1.95.0 / LLVM 21.1.8,
using the project Nix devShell. Cargo's default **release** profile was used
(`opt-level=3`, no LTO, debug assertions off), offline, with x86jit pinned to
`b0c9ae52820dffc1e2cb75c4928a670788a916d0` (`b0c9ae5`). This is a measurement
spike; no guest behaviour or scheduler change is proposed.

## String instruction accounting

The current CPU pin is `fd6792777ea864745a776ccdd3279a2fa9dd4bff`.
The profiling tables below retain the revisions on which they were measured;
they are historical measurements, not new measurements of this pin.

Real16 machine time uses completed `run_steps` units: one per ordinary
instruction, one per nonempty REP element, and one for an empty REP. REP
MOVS/STOS and REP INS/OUTS follow the same per-element convention. Each
INS/OUTS element produces one port exit and charges one step at handoff;
supplying an input value and finishing its deferred ES store add no step.
The core retains an answered INS across a faulting store and blocks IRQ
acceptance until that store completes. An active cap can retain that answered
handoff; the next positive run finishes its store before executing another
unit. This is deterministic engine accounting, not hardware cycle timing
(Intel 80286 Programmer's Reference Manual, REP, INS and OUTS descriptions).
Whole-instruction retirement is a separate diagnostic: a nonempty REP
retires once, at its final handoff for port strings. IRQ entry costs no step;
handler instructions and IRET count normally.

The machine tests in `crates/machine/tests/port_strings.rs` check per-element
caps and port timestamps, pending input at a cap, empty REP, MMIO access
ordering, final memory/registers, and IRQ/IRET resumption.

## Scope and method

The temporary `machine/phase-profile` feature selects an instrumented **copy** of
`Machine::run_bounded`; without it the original loop runs without inner timers
or counters. Consecutive `Instant` timestamps delimit CPU, clock, interrupt,
stop-dispatch and DMA phases. The halted branch is timed inclusively, including
its ticks and services. A separate timer covers each complete runner call.
All timestamps are observations; guest execution, caps and inputs still depend
only on virtual time. Fixture loading, assembly, assertions and final RAM hashing
are outside the timed interval. FreeDOS policy polling is included.

Workloads use `assembly::bios` and the existing frontend constants and policies:

- **(a) BIOS spin:** no attached floppy, quantum 65,536 blocks. Run untimed to
  `BIOS_GOLDEN_MAX_RETIRED=10,000,000`, assert POST reached checkpoint `00`, then
  measure another 100,000,000 virtual instructions. This isolates the INT 19h
  retry loop rather than including POST.
- **(b) FreeDOS boot:** attached FreeDOS floppy, quantum 65,536 blocks, reset to
  the date prompt detected in 25,000,000-instruction polling chunks. This is
  `freedos_reaches_shell_prompt`'s machine and stopping policy. The observed
  prompt is **Enter new date**, not yet `A:\>`.
- **(c) Interactive session:** the same boot with quantum **64 blocks**, followed
  by `dos_dir_script()` and 80,000,000 further virtual instructions, exactly the
  two phases of `freedos_runs_dir_from_the_keyboard`. The reported total includes
  boot, DIR and the subsequent wait for input. Intermediate boot counters permit
  subtraction of the post-prompt interval without changing execution.

Each cap can overshoot by a CPU quantum. The counts below are actual deltas.
Five baseline/profile pairs ran sequentially, alternating builds; the exploratory
pilot was excluded. Commands below ran inside the already loaded devShell:

```sh
# Run these two commands alternately, five times each.
PROFILE_REPEATS=1 cargo test --offline --release -p cli --test machine_time_profile -- --ignored --exact machine_time_profile --nocapture
PROFILE_REPEATS=1 cargo test --offline --release -p cli --test machine_time_profile --features machine/phase-profile -- --ignored --exact machine_time_profile --nocapture

# Separate, more intrusive diagnostic pass; excluded from the main tables.
PROFILE_REPEATS=3 cargo test --offline --release -p cli --test machine_time_profile --features machine/tick-profile -- --ignored --exact machine_time_profile --nocapture
```

Inputs (SHA-256):

```text
vendor/kiselev-bios/bios-xi8088.bin  1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8
vendor/freedos/freedos.img         e13c52543c17469f6c1e2fd5e132e1055566ba8a05e92c5f228f0815b763d051
```

## Results

Wall times and MIPS are medians of five runs; MIPS means retired instructions
per wall second. A boundary is one completed `cpu.run`, not an outer policy call.

| Workload | Retired instructions = `tick_all` calls | CPU boundaries | Port/MMIO stops | Baseline wall (s) | Profile wall (s) | Baseline / profile MIPS |
|---|---:|---:|---:|---:|---:|---:|
| (a) BIOS spin | 100,081,616 | 1,018 | 509 | 2.974828 | 3.135575 | 33.64 / 31.92 |
| (b) FreeDOS boot | 25,189,399 | 375,827 | 375,763 | 3.430247 | 3.407624 | 7.34 / 7.39 |
| (c) Interactive, including boot | 105,000,511 | 808,419 | 447,662 | 16.850111 | 16.739208 | 6.23 / 6.27 |

There were **zero halted clocks and zero halted-branch iterations** in every run.
Thus retired time, virtual time and PIT clocks have identical deltas. Each
`tick_all` count is derived from the observed clock delta: `tick_clock` advances
by one and calls `tick_all` exactly once. Each call visits 10 devices in (a),
11 in (b)/(c).

The phase table uses the **single median-wall profiled run of each workload**,
so its absolute durations and shares have the same denominator. Entries are
**milliseconds (% of measured wall time)**; they are raw observations without
timer-floor subtraction. Dispatch includes the stop match, read-completion
handshakes and the empty budget-stop path. Residual includes loop bookkeeping,
cap checks, timer bookkeeping and policy work outside the phase intervals.

| Phase | (a) BIOS spin | (b) FreeDOS boot | (c) Interactive, including boot |
|---|---:|---:|---:|
| `cpu.run` | 1610.353 (51.36%) | 2945.154 (86.43%) | 14871.773 (88.84%) |
| `advance_clock`: ticks + edge detection | 1524.973 (48.63%) | 430.047 (12.62%) | 1798.822 (10.75%) |
| `service_interrupts` | 0.075 (0.0024%) | 7.219 (0.212%) | 16.315 (0.097%) |
| Stop dispatch | 0.100 (0.0032%) | 9.994 (0.293%) | 19.701 (0.118%) |
| `service_dma` | 0.047 (0.0015%) | 8.298 (0.244%) | 16.947 (0.101%) |
| Halted branch, inclusive | 0 (0%) | 0 (0%) | 0 (0%) |
| Residual | 0.027 (0.0009%) | 6.912 (0.203%) | 15.650 (0.093%) |
| Total | 3135.575 (100%) | 3407.624 (100%) | 16739.208 (100%) |

Across all five profiled runs, the clock share was 48.63–49.95% in (a),
11.89–12.71% in (b), and 10.75–11.58% in (c). Subtracting boot from the selected
(c) run leaves **80,000,194 instructions, 345,893 boundaries, 71,669 I/O stops,
13.425781 s**: CPU 89.51%, clock 10.27%, and the three service/dispatch phases
together 0.172%.

## Instrumentation overhead and limits

| Workload | Baseline min–max (s) | Profile min–max (s) | Difference of medians |
|---|---:|---:|---:|
| (a) BIOS spin | 2.932–3.407 | 2.907–3.567 | +0.160746 s (+5.40%) |
| (b) FreeDOS boot | 3.344–4.637 | 3.325–3.955 | −0.022623 s (−0.66%) |
| (c) Interactive | 15.809–20.520 | 15.577–17.281 | −0.110903 s (−0.66%) |

These are measured net wall differences, including build/code-layout effects
and host variability; they do **not** resolve a pure instrumentation cost.
Negative differences are not evidence of acceleration. An empty `Instant`
interval averaged 12.86–15.47 ns in the phase runs, comparable to individual
service calls. Consequently the small service shares include a substantial
measurement floor; they should be treated as upper-bound observations rather
than precise isolated service costs. No CPU affinity or exclusive-host control
was used. `samply` and `cargo-flamegraph` were unavailable.

The separate `tick-profile` pass samples one clock every 4,093 clocks, times each
device, and samples aggregate tick/edge costs on the adjacent clock. Its timer
floor and perturbation prevent a trustworthy per-device attribution; those
samples must not be extrapolated into full-run percentages.
There were 24,452 / 6,154 / 25,653 samples per device in (a)/(b)/(c).
Taking the median of three trials for each device, the timer-inclusive intervals
ranged across devices from 15.9–20.1 / 15.6–20.6 / 17.3–21.8 ns per call.
The diagnostic pass's median wall times were 4.303 / 6.290 / 25.686 s,
respectively: +37.2% / +84.6% / +53.4% against the main profiled medians.
This substantially more intrusive pass does not support ranking device costs.

All builds produced identical instruction/tick counts and identical FNV-1a
digests of RAM `[0, 0xB8000)`: (a) `3daefb9c9c31d4e4`, (b)
`dc74447b9b2c0880`, (c) `dff83de8a1b0d0ae`. Assertions also checked the POST
checkpoint, kernel/FreeCOM/date prompt, and the echoed DIR and directory listing.
This verifies the measured outputs, not equivalence of every internal state.

## Scheduler implications

- Per-clock work is about **half** of the INT 19h spin, but only **11–13%** of
  these FreeDOS runs. The hypothesis that it dominates both spin and boot is
  rejected by the phase measurements; FreeDOS is mostly inside `cpu.run`.
- Removing the entire measured clock phase would give optimistic ceilings of
  about **1.95×, 1.14× and 1.12×** respectively. An event-driven scheduler can
  target repeated device visits and edge polls, but still has to perform real
  device transitions and preserve IRQ edges; these ceilings are not forecasts.
- Direct machine-boundary services and dispatch total under **1%**, even with
  the 64-block quantum. Almost every boot boundary is already I/O-forced;
  increasing the budget cannot remove those exits. `cpu.run` includes core-side
  entry/exit work, so this experiment does not isolate that part of boundary cost.
- The post-prompt interval confirms CPU dominance independently of boot. Its
  80-million-instruction budget includes waiting after DIR; this is not a timing
  of DIR alone or of keyboard-response latency.
- These workloads provide **no measured HLT fast-forward benefit**. Scheduling
  changes must also preserve the current single-clock edge contract (Intel 8254
  datasheet, Mode 2), even though the largest measured opportunity here is the
  much more frequent running-CPU tick path.

## Measurement source dependencies

The instrumentation is not on `main`. It lives on the branch
`spike/upc-wk6.1-instrumentation` (commit `16a3d2a`, on `origin`):
`crates/cli/tests/machine_time_profile.rs`, `crates/machine/src/time_profile.rs`,
the measurement hooks in `crates/machine/src/lib.rs` and `crates/bus/src/lib.rs`,
and the feature declarations in both crates' `Cargo.toml`. Check that branch out
to rerun the commands above.

Workload definitions come from `crates/cli/src/lib.rs`,
`crates/assembly/src/{presets,run,assemble}.rs`, and the three existing golden
tests `crates/cli/tests/{bios_boot,freedos_boot,dos_interactive}_golden.rs`.
The CPU revision is pinned by `Cargo.toml` / `Cargo.lock`; fixtures are listed
above. The tables remain the retained result after the temporary code is removed.

## Re-measurement after adopting corrected timing (2026-11-09)

Same host and toolchain, release build, x86jit pinned at `70be008`, main after
the corrected loop became the only loop. Method: the release build of the
interactive DOS golden (`dos_interactive_golden`, BIOS POST, FreeDOS boot to the
date prompt, `DIR`) sampled with the macOS `sample` tool at 1 ms, three runs;
inclusive samples of the test thread grouped by frame. The whole session takes
about 0.85 s for roughly 11 million steps. Categories overlap (lifting happens
inside dispatch; invalidation is reached from guest stores).

| Inclusive share of the session | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| CPU core, `Vcpu::run_steps` | 98.9 % | 98.2 % | 97.1 % |
| — translation-cache invalidation (`Memory::dispatch_smc` → `TranslationCache::invalidate_range`) | 50.6 % | 46.1 % | 49.4 % |
| — dispatch and block resolution | 25.5 % | 29.2 % | 25.6 % |
| — interpreting IR | 21.3 % | 21.2 % | 20.4 % |
| — decoding and lifting to IR | 18.9 % | 21.5 % | 19.9 % |
| bus: port and MMIO access, settle, deadlines | 0.4 % | 1.0 % | 1.9 % |
| devices | 0.2 % | 0.2 % | 0.3 % |

Findings:

- The machine side (bus, devices, scheduler) is now **under 2 %** of the DOS
  session; the old per-clock tick path (11–13 % above) is gone.
- About **half of all time** is self-modifying-code bookkeeping in the CPU core:
  guest stores into pages that also hold translated code invalidate a range of
  the translation cache, and the invalidation itself is the cost. Real-mode DOS
  and BIOS code keeps data next to code, so ordinary stores hit code pages.
- Interpreting the IR is only about a fifth of the time; lifting cold code is
  another fifth. Isolated interpreter loops run at 13–19 ns per instruction with
  warm blocks and 67–71 ns on first execution (lift included).
- The remaining large speed-ups are in the CPU core, not in this repository;
  they were reported to the CPU core project together with the interpreter
  measurements.

## At the close of the 286 foundation (2026-11-10)

Same method, `main` at `479f8ed`, x86jit pinned at `54fad17` (the fixes for
the step-budget dependence). The session takes 0.85–0.86 s wall time in three
plain runs.

| Inclusive share of the session | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| CPU core, `Vcpu::run_steps` | 97.7 % | 96.8 % | 97.6 % |
| — translation-cache invalidation | 51.2 % | 44.7 % | 46.5 % |
| — dispatch and block resolution | 29.1 % | 30.6 % | 29.2 % |
| — interpreting IR | 15.4 % | 19.2 % | 19.3 % |
| — decoding and lifting to IR | 23.0 % | 21.9 % | 23.9 % |
| bus: port and MMIO access, settle, deadlines | 0.6 % | 1.4 % | 1.3 % |
| devices | 0.2 % | 0.5 % | 0.4 % |

The shape is unchanged from the previous section: the machine side stays
under 2 %, and translation-cache invalidation is still about half of the
session. The CPU core project is replacing its whole-cache invalidation scan
with a page-indexed span map; re-measure here when that is pinned.

## After the page-indexed invalidation in the CPU core (2026-11-10)

Same method, `main` at `0a2d285`, x86jit pinned at `96a49a9` (page-indexed
translation-cache invalidation that drops only the blocks whose bytes were
written). Behaviour is unchanged: the fast tests, the release device tests and
the 13 slow goldens (including the per-clock/deadline engine comparisons)
pass with the same expectations, with the fixtures required.

Wall time of the release `dos_interactive_golden` binary, built from both pins
in the same session and run alternately three times each:

| Pin | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| `54fad17` | 0.84 s | 0.83 s | 0.84 s |
| `96a49a9` | 0.50 s | 0.50 s | 0.49 s |

Share of the test thread's samples per category, pooled over three sampled
runs (category samples summed over the runs, divided by the summed totals), and the estimated time (share times the mean wall time). The sampler
does not take the same number of samples per second in both runs, so the
estimate is rough. Categories overlap: lifting is reached from dispatch, and
invalidation from guest stores, so the CPU-core rows do not add up; the rows
left out are the machine loop itself and the test harness.

| Category | `54fad17` share | time | `96a49a9` share | time |
|---|---|---|---|---|
| CPU core, `Vcpu::run_steps` | 97.3 % | 0.81 s | 97.4 % | 0.48 s |
| — translation-cache invalidation | 47.3 % | 0.40 s | 49.3 % | 0.25 s |
| — dispatch and block resolution | 29.7 % | 0.25 s | 4.7 % | 0.02 s |
| — interpreting IR | 18.1 % | 0.15 s | 39.7 % | 0.20 s |
| — decoding and lifting to IR | 22.9 % | 0.19 s | 1.3 % | 0.01 s |
| bus: port and MMIO access, settle, deadlines | 1.1 % | 0.01 s | 1.3 % | 0.01 s |
| devices | 0.4 % | < 0.01 s | 0.3 % | < 0.01 s |

Findings:

- The session takes **40 % less wall time** (0.84 s to 0.50 s). Lifting and
  dispatch almost disappear: a store next to code no longer drops blocks
  whose bytes it did not write, so those blocks are not lifted again.
- Interpreting IR did not get cheaper; it is now the second largest part.
- Invalidation costs less in absolute time but is still about half of the
  session. The time is in the per-store candidate search (x86jit
  `x86jit-core/src/cache.rs`, `SpanIndex::candidates` and
  `TranslationCache::invalidate_range`): each guest store to a code page
  copies the key of every block on that page into a new hash set (the standard
  library's default hasher), looks each key up again to test the overlap, and
  collects the overlapping ones into a new vector, also when no block covers
  the written bytes. This was sent to the CPU core project with the profile.

## After byte-coverage SMC rejection (2026-11-11, MEASURED)

Both pins were built offline in the same session, in separate target
directories, using Cargo's default release profile and the project Nix
devShell: old `73ed5d6ef4fc68f91cdad10b1e7c1c370363d030`, new
`8979fc95cd81d73763d50d1d627b40ded3f8f284`. Host: macOS 27.0.1,
arm64 (`aarch64-apple-darwin`), rustc 1.99.1
(`58cf8f9c3`, 2026-10-13), LLVM 23.1.1.

The workload is the preceding section's release `dos_interactive_golden`:
BIOS POST, INT 19h floppy boot, FreeDOS date/time prompts, keyboard `DIR`,
then the bounded wait at the shell. Each binary was warmed once, then run
back to back in the order old/new, old/new, old/new. Wall time was observed
with Python `time.perf_counter()` around process start/wait, including the
test harness, fixture loading and assertions; no Cargo build time is included.
Warmups and an unsuccessful sampler pilot are excluded from the table.

Reproduction, with separate `OLD_TARGET` and `NEW_TARGET` directories and the
devShell loaded before each Cargo invocation:

```sh
# Build at each corresponding pin, before any timed runs.
cargo test --offline --release -p cli --test dos_interactive_golden --no-run --target-dir "$OLD_TARGET"
cargo test --offline --release -p cli --test dos_interactive_golden --no-run --target-dir "$NEW_TARGET"
# Run the emitted test binaries directly, alternately, with these arguments.
UNEMUPC98_REQUIRE_FIXTURES=1 "$BINARY" --ignored --exact freedos_runs_dir_from_the_keyboard --nocapture
```

| Pin | Run 1 (s) | Run 2 (s) | Run 3 (s) | Median (s) | Mean (s) |
|---|---:|---:|---:|---:|---:|
| `73ed5d6` | 1.057979 | 1.124007 | 1.157828 | 1.124007 | 1.113271 |
| `8979fc9` | 0.821589 | 0.982200 | 0.772619 | 0.821589 | 0.858803 |

MEASURED: the median fell **26.9%**, or **1.37×** throughput for this fixed
session. These are observations under concurrent test load, not an
exclusive-host estimate. Our builds and test gates had finished before timing;
another worker was stress-testing the fast suite on the same host. `uptime`
reported these 1/5/15-minute load averages immediately before each pair:

| Pair | Load averages |
|---|---|
| 1 | 20.31 / 32.27 / 40.26 |
| 2 | 20.68 / 32.15 / 40.17 |
| 3 | 20.68 / 32.15 / 40.17 |

**Sampling results (MEASURED):** a separate batch outside the worker sandbox
used `/usr/bin/sample PID 30 1 -mayDie`, attaching immediately after each test
process was spawned, at a nominal 1 ms interval for up to 30 s. It used the
same release test binaries and exact `freedos_runs_dir_from_the_keyboard`
test, with `--test-threads 1` and fixtures required. Three runs per pin
alternated old/new. Host load averages before this batch were
**5.79 / 19.89 / 33.33**.

Attribution includes only the `freedos_runs_dir_from_the_keyboard` test
thread. Each category counts inclusive samples at the outermost matching
frame on each stack branch: `invalidate_range` or `dispatch_smc` for
invalidation, x86jit's `interp` module for interpreting IR, and its `lift`
module for lifting. Invalidation is reached from guest stores inside the
interpreter, so the rows overlap and do not add up. Pooled shares divide
summed category samples by summed test-thread samples, rather than averaging
the per-run percentages. Parentheses below give pooled counts or shares.

| Test-thread samples / category | `73ed5d6`: runs 1 / 2 / 3 (pooled) | `8979fc9`: runs 1 / 2 / 3 (pooled) |
|---|---|---|
| Samples | 383 / 414 / 506 (1,303) | 253 / 269 / 231 (753) |
| Translation-cache invalidation | 36.0 / 34.5 / 38.9% (36.7%) | 3.2 / 2.2 / 5.6% (3.6%) |
| Interpreting IR | 41.5 / 39.6 / 40.1% (40.4%) | 60.1 / 62.1 / 63.6% (61.9%) |
| Lifting | 0.8 / 0.2 / 0.6% (0.5%) | 1.2 / 1.1 / 2.2% (1.5%) |

Invalidation fell from about **37% to 4%** of the test thread. Using the
pooled shares and mean per-run sample counts at the nominal 1 ms interval,
its estimated cost per session fell from roughly **160 ms to 9 ms**:
`(478 / 1303) × ((383 + 414 + 506) / 3) × 1 ms = 159.3 ms` before, and
`(27 / 753) × ((253 + 269 + 231) / 3) × 1 ms = 9.0 ms` after. These are
approximate durations of the sampled work; attachment gaps and host
scheduling limit their accuracy. **Interpreting IR is now the dominant
cost**, at 61.9% of the pooled new-pin test-thread samples.

MEASURED inputs (SHA-256), shared by both pins:

```text
vendor/kiselev-bios/bios-xi8088.bin  1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8
vendor/freedos/freedos.img         b7b088333f7f30584d768941fecea8288b63f2d61e13ec6c9472439eda66e51f
```

The FreeDOS image hash differs from the original instrumentation spike's
historical input above. This comparison uses the current machine file and
current fixtures for both pins. All six timed runs passed, stopped with
`BudgetExhausted`, and emitted byte-identical MDA consoles: 899 bytes,
SHA-256 `e580192e19d8b2bbf463a14593ff9395752a580fc65f86df95b2a70b5081f80f`.
The new pin also passed the full fast suite (1,431 passed, 16 ignored),
release device suite (490 passed), and required-fixture slow set (16 passed),
with no changed golden expectations or re-baselining. The opt-in scheduler
wall-time test returned without timing because its environment gate was unset.

## After the Real16 interpreter update (2026-11-11, MEASURED)

Both pins were built offline in this worktree in the same session, in separate
target directories, using Cargo's default release profile (`opt-level=3`, no
LTO, debug assertions off) and the project Nix devShell:

- Old: `8979fc95cd81d73763d50d1d627b40ded3f8f284`.
- New: `a92b676e081ce862dcb48c82645b3d3a8aa2364f`.

Host: macOS 27.0.1 (26A434), arm64 (`aarch64-apple-darwin`), rustc 1.99.1
(`58cf8f9c3`, 2026-10-13), LLVM 23.1.1. READ: the machine still selects the
original `CpuMode::Real16` application profile; this re-pin does not opt into
the I286/I386 machine execution profile or select an absent FPU.
No adapter source changes were required by the compiler. The new
`Exit::UnsupportedSystemContext` is already reported as
`CpuError::UnexpectedExit` by the adapter's fallback (`crates/cpu/src/x86jit_backend.rs`,
`X86JitCpu::map_exit`), rather than treated as a resumable machine stop.

The workload, build commands and process timing follow the preceding section:
release `dos_interactive_golden`, exact test
`freedos_runs_dir_from_the_keyboard`, fixtures required, `--ignored --nocapture`.
It performs BIOS POST, floppy boot, FreeDOS date/time input, keyboard `DIR`
and a bounded wait at the shell. Each binary was warmed once, then run
old/new, old/new, old/new. Python `time.perf_counter()` surrounds process
start/wait with combined output captured, including the harness, fixture
loading and assertions. Warmups and Cargo builds are excluded from the table.
All local build and test gates had completed before the timing batch.

| Pin | Run 1 (s) | Run 2 (s) | Run 3 (s) | Median (s) | Mean (s) |
|---|---:|---:|---:|---:|---:|
| `8979fc9` | 0.454241 | 0.442651 | 0.449007 | 0.449007 | 0.448633 |
| `a92b676` | 0.436097 | 0.440058 | 0.436899 | 0.436899 | 0.437685 |

MEASURED: the observed median fell **2.7%**, or **1.028×** throughput for this
fixed session. These three alternating pairs are observations under shared
host load, not an exclusive-host estimate. `uptime` immediately before each
pair reported:

| Pair | Load averages (1 / 5 / 15 minutes) |
|---|---|
| 1 | 31.66 / 26.46 / 21.01 |
| 2 | 31.66 / 26.46 / 21.01 |
| 3 | 29.77 / 26.15 / 20.94 |

MEASURED inputs (SHA-256), identical for both pins and to the preceding section:

```text
vendor/kiselev-bios/bios-xi8088.bin  1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8
vendor/freedos/freedos.img         b7b088333f7f30584d768941fecea8288b63f2d61e13ec6c9472439eda66e51f
```

All six timed runs and both warmups passed, stopped with `BudgetExhausted`,
and emitted byte-identical MDA consoles. Including the final newline, each
console transcript is 899 bytes, SHA-256
`e580192e19d8b2bbf463a14593ff9395752a580fc65f86df95b2a70b5081f80f`,
matching the previous profile. Raw console text without that newline is 898
bytes, SHA-256 `c192c44707799aac3db38f3a52b3324422900c17140ce8cacc7dca3d76a2dd37`.

MEASURED validation: the fast suite passed (1,666 passed, 18 ignored), as did
the release device suite (611 passed). The required-fixture slow set initially
reported 17 passed and one failure: the IDE golden asserted the old Cargo.lock
pin before executing the guest. Only that revision assertion and its matching
doc-comment were updated; its targeted rerun passed with the existing retired
counts, screen hashes and POST expectations. All 18 slow test entries therefore
passed across the original run and targeted retry, without re-baselining.
The scheduler wall-time entry remained environment-gated, and the optional
user-ROM identity entry does not qualify proprietary firmware execution.
Build-all-targets, clippy, fmt, publication guard, offline dependency
license/ban/source checks and rustdoc with warnings denied passed.

**Sampling results (MEASURED by the supervisor):** outside the worker sandbox,
macOS `/usr/bin/sample PID 30 1 -mayDie` attached immediately after process
spawn. Both pins' release `dos_interactive_golden` test binaries were built
with separate target directories. The exact
`freedos_runs_dir_from_the_keyboard` test ran with `--test-threads 1` and
fixtures required, alternating old/new for three accepted runs per pin.
The first old-pin run was repeated because the sampler attached after its
test thread had ended; that unsuccessful attachment is excluded below.

Attribution follows the preceding section: only the test thread is included,
with inclusive samples counted at the outermost matching frame on each stack
branch. Pooled shares divide summed category counts by summed test-thread
counts, rather than averaging per-run percentages. Categories overlap.

| Test-thread samples / category | `8979fc9`: runs 1 / 2 / 3 (pooled) | `a92b676`: runs 1 / 2 / 3 (pooled) |
|---|---|---|
| Samples | 265 / 370 / 369 (1,004) | 342 / 724 / 1,119 (2,185) |
| Translation-cache invalidation | 3.8 / 2.2 / 0.3% (1.9%) | 1.5 / 1.4 / 5.1% (3.3%) |
| Interpreting IR | 61.5 / 64.6 / 87.5% (72.2%) | 49.7 / 63.0 / 56.0% (57.3%) |
| Lifting | 0.8 / 1.4 / 0.0% (0.7%) | 0.6 / 2.8 / 1.7% (1.9%) |

Host load was high while other workers were building: `uptime` reported
**50.26 / 25.23 / 20.85** before the batch and **17.59 / 21.35 / 19.99**
before the repeated run (1 / 5 / 15-minute load averages). The per-run spread
is large, so this comparison is indicative only.

The new pin inlines interpreter helpers, moving some work out of frames
matched as the `interp` module. These shares are therefore not directly
comparable to wall time. Wall time remains the primary measure: the observed
median fell from **0.449007 s to 0.436899 s (2.7%)**, also under host load.
