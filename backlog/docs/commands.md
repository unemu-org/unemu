# Commands

Everyday commands for this project. Keep this file **flat and copy-pasteable** — agents and humans both grep it.

## Build / run / test

```sh
cargo build                                  # build the workspace
cargo run -p cli -- <config>                 # run the emulator
cargo test --workspace                       # unit + golden + integration
cargo test --workspace -- --ignored          # slow goldens: BIOS POST, FreeDOS boot, DOS sessions (~2 min)
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --workspace -- --ignored # fail if a fixture is missing
# Opt in to Corrected-vs-Legacy A/B and informative host timing (slow in debug):
UNEMUPC98_REQUIRE_FIXTURES=1 UNEMUPC98_SCHEDULER_AB=1 cargo test --offline -p cli --test scheduler_ab -- --include-ignored --nocapture
cargo test -p devices                        # one crate's tests
```

The plain ignored set retains PerClock-vs-Deadline comparisons. Corrected-vs-Legacy
comparisons and both wall-time tests return early unless `UNEMUPC98_SCHEDULER_AB=1`.
The Corrected DOS file-write comparison uses four times the legacy command step
budget: each completed REP iteration counts as a step instead of only the whole
instruction. Both modes must reach the `3 file(s)` listing.

## Typecheck / lint / format

```sh
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
cargo fmt --all -- --check                   # CI check
cargo deny --all-features list               # inspect dependency licences
cargo deny --all-features check licenses bans sources
```

The `deny.toml` allowlist covers the all-features graph: AGPL-3.0-or-later for
workspace crates, MIT, Apache-2.0, and Unicode-3.0 for dependencies. New licences
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
bash scripts/ci-gate.sh fixtures                 # network fetch + build BIOS, FreeDOS, test386
bash scripts/ci-gate.sh goldens                  # slow tests (~2 min); requires fixtures
```

Fixture-dependent tests print a skip message when a fetched vendor file is missing.
Set `UNEMUPC98_REQUIRE_FIXTURES=1` to make a missing fixture fail with its path and
fetch script instead. The `goldens` gate always sets this variable, so a failed or
incomplete fixture fetch cannot silently pass CI.

Each gate uses `nix develop --no-update-lock-file --command` (except `guard`, which
runs directly), preserves its exit status, and tees combined output to the gitignored
`ci-logs/<step>.log`. Set `CI_LOG_DIR` to choose another log directory. Cargo builds/tests
use `--locked`; formatting uses `cargo fmt --check`. CI runs every gate even after failures,
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
