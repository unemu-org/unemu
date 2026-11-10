# Commands

Everyday commands for this project. Keep this file **flat and copy-pasteable** — agents and humans both grep it.

## Build / run / test

```sh
cargo build                                  # build the workspace
cargo run -p cli -- --boot                   # Real16 timer demo; no args print usage
cargo test --workspace                       # unit + golden + integration
cargo test --release -p devices              # release-only device behaviour (CI runs it too)
cargo test --workspace -- --ignored          # slow goldens: BIOS POST, FreeDOS boot, DOS sessions (~2 min)
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --workspace -- --ignored # fail if a fixture is missing
# Corrected per-clock oracle versus production deadlines (no environment gate):
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test scheduler_ab -- --include-ignored --nocapture
# Opt-in host timing only, BIOS POST and FreeDOS prompt, median of three per engine:
UNEMUPC98_REQUIRE_FIXTURES=1 UNEMUPC98_SCHEDULER_WALL_TIMES=1 cargo test --offline --release -p cli --test scheduler_ab scheduler_engine_wall_times -- --ignored --nocapture
# DOS sessions and their final console screens (requires fetched fixtures):
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test dos_interactive_golden -- --ignored --nocapture
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test dos_write_golden -- --ignored --nocapture
cargo test -p devices                        # one crate's tests
cargo build --offline -p window --bin unemupc98-window
cargo run --offline -p window --bin unemupc98-window # default vendor BIOS + FreeDOS image
cargo run --offline -p window --bin unemupc98-window -- vendor/kiselev-bios/bios-xi8088.bin vendor/freedos/freedos.img
cargo tree --offline -i window               # window is a leaf frontend
```

The window binary requires a graphical desktop and fetched BIOS/FreeDOS fixtures.
It uses a fixed 2× pixel scale and stops advancing at the FreeCOM date prompt,
a natural halt or a CPU failure, keeping the last screen visible until closed.
There is no host keyboard input or pacing yet. Blink stays visible. Workspace
and presenter tests remain headless. Manual desktop acceptance confirmed the
FreeDOS date prompt, font and colours. Progress follows redraw delivery:
minimising or occluding the window can pause execution, and a resize-triggered
redraw advances a chunk until pacing is added.

Default fixture paths are resolved against the workspace location recorded at
build time, independently of the current directory; explicit paths remain
relative to the current directory. From outside the workspace, Cargo also needs
`--manifest-path /path/to/workspace/Cargo.toml`. Missing-file diagnostics name the
path and the relevant `vendor/kiselev-bios/fetch.sh` or `vendor/freedos/fetch.sh`.
`--help`, `--version` and `--about` include the embedded fonts' attribution.
On Linux the window requires the host's X11 or Wayland client libraries at run
time; a bare Nix devShell may not put them on the dynamic loader's search path.

The ignored set includes firmware/DOS goldens and corrected PerClock-vs-Deadline
comparisons, including final RAM/register/device state. The per-clock engine is
available through the non-default `reference-scheduler` feature enabled by CLI
dev-dependencies, never through `MachineConfig`. Legacy comparisons and
`UNEMUPC98_SCHEDULER_AB` are retired. Step budgets include REP iterations; the
command budget is calibrated to the final DOS file-write listing. No arguments
print usage to stdout with status 0; unknown or extra arguments print usage to
stderr with status 2. The informative
`scheduler_engine_wall_times` test skips with one line unless
`UNEMUPC98_SCHEDULER_WALL_TIMES=1` is set, including in ordinary slow/CI runs.

## Typecheck / lint / format

```sh
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --locked -p cli --bin unemupc98 -- -D warnings # production, no reference-scheduler
cargo fmt --all
cargo fmt --all -- --check                   # CI check
cargo deny --all-features list               # inspect dependency licences
cargo deny --all-features check licenses bans sources
```

The `deny.toml` allowlist covers the all-features graph: AGPL-3.0-or-later for
workspace crates, MIT, Apache-2.0, Unicode-3.0, BSD-2-Clause, BSD-3-Clause
and ISC for dependencies. CC-BY-SA-4.0 is allowed only for the `window` crate
(a crate-scoped exception), whose embedded font assets carry it; see
`crates/window/assets/fonts/NOTICE.md`. New licences
require review before extending it. Unknown registries and git sources are denied;
only the pinned x86jit repository is explicitly allowed. Wildcard dependencies
are denied and duplicate versions warn. These checks do not need the advisory database.

## CI

Forgejo Actions runs `.forgejo/workflows/ci.yml` on pushes to `main` and manual
dispatches on the x86-64 runner. From the repository root, run the same gates locally:

```sh
bash scripts/ci-gate.sh devshell
bash scripts/ci-gate.sh fmt
bash scripts/ci-gate.sh guard
bash scripts/ci-gate.sh clippy
bash scripts/ci-gate.sh deny                     # licences, bans, sources; all features
bash scripts/ci-gate.sh tests
bash scripts/ci-gate.sh fixtures                 # build/hash-check BIOS, FreeDOS, test386; fetch missing inputs
bash scripts/ci-gate.sh goldens                  # slow tests (~2 min); requires fixtures
```

Fixture-dependent tests print a skip message when a fetched vendor file is missing.
Set `UNEMUPC98_REQUIRE_FIXTURES=1` to make a missing fixture fail with its path and
fetch script instead. The `goldens` gate always sets this variable, so a failed or
incomplete fixture fetch cannot silently pass CI. The µPD765 FAT12 test follows
the same policy when `mformat` is missing and points to `nix develop`.

All three fixture scripts check pinned output SHA-256 hashes and rebuild offline
when their pinned source commits and release archives are cached. FreeDOS also
fixes the volume serial (`2895-9853`), label (`FREEDOS`) and all filesystem
timestamps (1980-01-01 UTC). A hash mismatch requires checking the sources and
toolchain and deliberately rebuilding the goldens' expectations before changing
the expected hash. See each `vendor/*/README.md` for cache paths and hashes.

Each gate uses `nix develop --no-update-lock-file --command` (except `guard`, which
runs directly), preserves its exit status, and tees combined output to the gitignored
`ci-logs/<step>.log`. Set `CI_LOG_DIR` to choose another log directory. Cargo builds/tests
use `--locked`; the clippy gate lints all features and then the production CLI
binary without dev-dependency feature unification; formatting uses `cargo fmt --check`. CI runs every gate even after failures,
saves the Cargo cache, and reports failing steps in commit status `ci / x86_64`.
See [`dev-setup.md`](dev-setup.md#ci) for the runner and cache setup.

## x86jit (CPU core)

```sh
# develop against a local x86jit checkout: a gitignored override, never a path dep in Cargo.toml
cat > .cargo/config.toml <<'TOML'
[patch."<the git URL x86jit-core is pinned to in Cargo.toml>"]
x86jit-core = { path = "<your x86jit checkout>/x86jit-core" }
TOML
git -C <your x86jit checkout> rev-parse HEAD    # must match the rev in Cargo.lock for goldens to be valid
```

A change the machine needs from the CPU lands in x86jit under its own beads and gate; this repo
then re-pins and re-runs the slow goldens. See `dev-setup.md` § CPU core.

## Guest test-ROMs

```sh
nasm -f bin boot.asm -o boot.bin             # flat binary for the guest
# (or nasm -f elf + ld + objcopy for larger programs)
```

## Pre-commit

```sh
pre-commit install                                  # one-time, per clone
pre-commit run --all-files                          # run active hooks
pre-commit run --all-files --hook-stage manual      # run staged-as-manual hooks too
```

## beads (`bd`) — open work

```sh
bd memories                                      # the shared working rules — read first
bd ready --exclude-type epic                     # unblocked work, highest priority first
bd list --status in_progress                     # what is claimed
bd show <id>                                     # one bead
bd list --type epic                              # the themes
bd create --title "<t>" --description "<d>" --acceptance "<ac>" --type task --priority P2 \
          --parent <epic-id> --no-inherit-labels --labels <labels>
bd update <id> --claim                           # before writing code
bd update <id> --append-notes "<progress>"       # never --notes on an existing bead
bd close <id> --reason "<commit> — <what landed>"
bd list --external-ref backlog:TASK-41           # the bead an old task id became
```

Details: [`working-on-tasks.md`](working-on-tasks.md).

## Backlog.md — docs and decisions only

```sh
backlog doc create "<title>"                      # → docs/doc-N - Title.md
backlog decision create "<title>"                 # → decisions/decision-N - Title.md
```

Never `backlog task create`: tasks moved to beads.
