# Test timings

unemu maintains front-end gate timings; unemupc maintains core build and hardware
test timings. The measurements below predate the split and are historical,
not measurements of this 22-crate workspace. The full pre-split report is in
[unemupc at the split point](https://github.com/unemu-org/unemupc/blob/0f2795402e947e235a7b892c931910b05f05d61f/backlog/docs/test-timings.md).

## Historical front-end measurements (2026-11-19)

Measured in the Nix devShell (Rust 1.99.1) on a shared macOS machine, with
6 compile jobs, 4 test threads, one target directory per worktree and
`CARGO_INCREMENTAL=0`. Concurrent load varied, so these are observations,
not budgets. The old `cli` crate has since become `cli-core`, `cli-pc` and
`upcli`; desktop models and session tests now live in separate crates.

| Pre-split front-end crate | Fast tests (cargo test) | Golden tier |
| --- | ---: | ---: |
| pc-image | 149 s | 99 s |
| window | 131 s | 94 s |
| media | 45 s | 31 s |
| image-store | 18 s | 38 s |
| image-registry | 7 s | 10 s |

The tiers ran under different load. `pc-image`'s `publication_death` took
34–71 s, `workspaces` 18–39 s, `template` 12–18 s and `large_disk` 7–14 s.
`image-store`'s `metadata` took 8–15 s and `store` 6–13 s. These are useful
places to measure again after a store change; no new timings are implied.

## Front-end gate now

Run `bash scripts/affected-tests.sh --plan` before its runner. Use the
[affected-test workflow](commands.md#affected-tests-and-the-bead-gate) to
measure the selected crates, dependents and tier. Core pins and fixture
changes require the front-end goldens against the locked core, using the
verified `UNEMUPC_FIXTURE_ROOT`. The core's own test binaries run upstream.

Cargo-nextest overlaps tests across binaries; the shared-machine wrapper still
limits concurrency. Record per-step wall times and slowest test binaries,
toolchain, core pins, fixture policy and load with each new measurement.
Repeat measurements before using them to justify another crate split.
