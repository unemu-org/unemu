# Commands

Everyday commands for this project. Keep this file **flat and copy-pasteable** — agents and humans both grep it.

## Build / run / test

```sh
cargo build                                  # build the workspace
cargo run -p cli -- <config>                 # run the emulator (once cli exists)
cargo test                                   # unit + golden + integration
cargo test -p devices                        # one crate's tests
```

## Typecheck / lint / format

```sh
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
cargo fmt --all -- --check                   # CI check
```

## x86jit worktree (CPU core)

```sh
# create the dev worktree once (from the x86jit repo)
git -C ~/src/x86jit worktree add ../x86jit-pc pc-emulator
# this repo depends on it via a pinned git dep + a local [patch]/.cargo/config.toml override
git -C ~/src/x86jit-pc status                # work on the CPU seam here
```

Before merging a seam back to x86jit `main`, run x86jit's own full suite
(interp / JIT / Unicorn / fuzz) — see the merge-discipline decision.

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

## Backlog.md

See the [`backlog` skill](../../.agents/skills/backlog/SKILL.md) and `backlog instructions overview`
for the canonical cheat-sheet. Most-used:

```sh
backlog task create "<title>" -d "<desc>" --ac "<criterion>"
backlog task list --plain                       # AI-friendly view
backlog task list -s "In Progress" --plain
backlog task <id> --plain                        # show one task
backlog task edit <id> -s "In Progress" --plan "<approach>"
backlog task edit <id> --check-ac 1 --notes "<progress>"
backlog task edit <id> -s Done
backlog board                                    # interactive kanban
backlog browser                                  # web UI
backlog doc create "<title>"                      # → docs/doc-N - Title.md
backlog decision create "<title>"                 # → decisions/decision-N - Title.md
backlog config                                   # view/edit config.yml
```
