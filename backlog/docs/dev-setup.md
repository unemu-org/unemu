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

> **Known defect (bead `upc-f7j.1`).** The pin still names a deleted branch at a host-local URL,
> so a fresh checkout does not build without the override above, and x86jit's current `main` has
> a changed `Exit` API. The bead re-pins and adapts the backend.

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
