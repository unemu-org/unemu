# Dev setup

Toolchain and local-environment setup. Stack-specific toolchain pinning (Nix flake, mise, asdf, rustup, nvm, uv, ...) is added at bootstrap.

The repo ships a `flake.nix` whose devShell always includes **`backlog`** (the Backlog.md CLI),
`pre-commit`, and `git`; stack packages are appended at bootstrap. Enter it with `nix develop`, or
let direnv auto-load it (`.envrc` is already `use flake`).

## direnv (optional but recommended)

`.envrc` ships in the repo. Allow it once per clone:

```sh
direnv allow
```

If you use a Nix flake (add at bootstrap), `.envrc` typically contains `use flake` and direnv auto-loads the devShell on `cd`.

## Pre-commit

```sh
pre-commit install
```

Hooks come from `.pre-commit-config.yaml`. The template ships generic hooks (whitespace, EOF, YAML/JSON checks, markdownlint, shellcheck, gitleaks, GPG UID guard). Add language-specific hooks (formatter, linter, typechecker) at bootstrap or later.

Run all hooks on demand:

```sh
pre-commit run --all-files
pre-commit run --all-files --hook-stage manual   # includes manual-staged hooks
```

## GPG signing

The `gpg-uid-guard` pre-commit hook (always active) refuses to sign when `user.email` has no matching UID on `user.signingkey`. Fix path if it fails:

```sh
git config user.email <your-email>
git config user.signingkey <key-id>
# or attach a matching UID to the key with `gpg --edit-key <key>`
```

## Stack-specific toolchain (Rust)

The `flake.nix` devShell provides the Rust toolchain (`rustc`, `cargo`, `clippy`,
`rustfmt`, `rust-analyzer`), plus `nasm` + `binutils` (guest test-ROM builds),
`qemu` (optional differential oracle), and `jq` (fetching the SingleStepTests
corpus). Enter with `nix develop` or let direnv auto-load it.

### CPU core: x86jit

The CPU is an external **git** dependency pinned to an exact revision of x86jit `main` (not
vendored, never a bare `path` dependency in `Cargo.toml` — that breaks `nix build` / CI). For
development, override it onto a local checkout with a `[patch]` in `.cargo/config.toml`, which is
gitignored:

```toml
[patch."<the git URL x86jit-core is pinned to in Cargo.toml>"]
x86jit-core = { path = "<your x86jit checkout>/x86jit-core" }
```

Cargo resolves the override without fetching the pinned URL, so this also works on a machine that
cannot reach it. Check the checkout out at the revision in `Cargo.lock`: goldens are only valid
against the pinned CPU revision.

The pinned HTTPS URL and x86jit's oracles submodule are anonymously fetchable.
No credentials or SSH URL rewrite are needed:

```sh
cargo fetch --locked
```

After one successful fetch the checkout is cached and builds work offline. Moving the pin
(edit the `rev` in `Cargo.toml`, then update `Cargo.lock`) is its own bead: the Definition of Done
plus the slow goldens. See decision-13.

## CI

`.forgejo/workflows/ci.yml` runs on pushes to `main` and via `workflow_dispatch`.
The Forgejo runner's `ubuntu-latest` label uses a `node:20-bookworm` container on
x86-64 Linux without systemd. The workflow installs Nix using pinned installer
v3.24.1 with `--init none` and the build sandbox disabled, then uses the flake devShell.
Cargo's registry, git database and `target/` are cached using `flake.lock` and
`Cargo.lock`; CI never needs dependency credentials.

The gates check the devShell, formatting, publication safety, Clippy, workspace
tests, fixture builds and slow goldens. Fixtures are built by the three
`vendor/{kiselev-bios,freedos,test386}/fetch.sh` scripts and the devShell's tools,
including `nasm` and `mtools` 4.0.49. The scripts fetch only missing pinned commits
or release archives and check every output against its pinned SHA-256 hash.
Retain their gitignored source clones and FreeDOS `build/*.zip` archives for
offline rebuilds. FreeDOS fixes its serial, label and timestamps using mtools'
`SOURCE_DATE_EPOCH` support; see `vendor/freedos/README.md`. Portable size checks
and `sha256sum`/`shasum -a 256` also allow running the scripts outside the devShell
with equivalent tools. Each gate still runs after
earlier failures. The final commit status, `ci / x86_64`, names failed steps.

Run a gate from the repository root with `bash scripts/ci-gate.sh <step>`;
see [`commands.md`](commands.md#ci) for the full sequence. The script enters
`nix develop --no-update-lock-file` for every gate except the publication guard,
which needs no devShell. Cargo builds/tests use `--locked`. Combined output is
saved to `ci-logs/<step>.log` (gitignored); set `CI_LOG_DIR` to change the directory.
The script preserves failures through `tee` so it can be used as a local gate too.

## beads (`bd`)

Open work is tracked with [beads](https://github.com/steveyegge/beads) (`bd`). It is **not** in the
devShell: x86jit's database was created by `bd` 1.3.1 and the pinned nixpkgs carries an older one;
use the same host-installed `bd` ≥ 1.3.1 for both projects (e.g. `brew install beads`, or a release
binary from the upstream repository) and check with `bd --version`.

On a fresh clone the issue database is not in git (`.beads/embeddeddolt/` is ignored). Once
`origin` carries `refs/dolt/data`, recreate it with `bd bootstrap` (non-destructive; see
`bd bootstrap --help`). Use `bootstrap`, not `bd init`, in a clone that already has `.beads/`:
`init` mints a new project identity. `bd init`, and the first `bd dolt push --yes` (which adopts
`origin` and writes `sync.remote` to `.beads/config.yaml`), each make a git commit on their own —
even with `--skip-hooks` — so after any bd setup command check `git log -1` and `git status`.

Until a remote exists the database lives on one machine only; `bd backup` makes a Dolt-native
backup.

### MSRV

Track x86jit's MSRV (the `rust-version` in its workspace `Cargo.toml`). The Nix `rustc` should satisfy it; if you
pin a channel later, add a `rust-toolchain.toml` and record the decision.
