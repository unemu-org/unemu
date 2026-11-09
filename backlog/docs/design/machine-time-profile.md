# Machine loop wall-time profile

Measured on 2026-11-09, macOS 27.0.1, arm64, Rust 1.95.0 / LLVM 21.1.8,
using the project Nix devShell. Cargo's default **release** profile was used
(`opt-level=3`, no LTO, debug assertions off), offline, with x86jit pinned to
`b0c9ae52820dffc1e2cb75c4928a670788a916d0` (`b0c9ae5`). This is a measurement
spike; no guest behaviour or scheduler change is proposed.

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
