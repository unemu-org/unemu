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

### CPU core: x86jit worktree

The CPU is an external pinned **git** dependency (not vendored). For development,
create a git worktree of x86jit and override the dep onto it locally:

```sh
git -C ~/src/x86jit worktree add ../x86jit-pc pc-emulator   # once
```

`Cargo.toml` pins the x86jit git rev; a local `[patch]` (or `.cargo/config.toml`)
points at `~/src/x86jit-pc` for editing. A bare `path` dependency is **not** used
— it breaks `nix build` / CI. See the worktree decision in `backlog/decisions/`.

### MSRV

Track x86jit's MSRV (currently 1.75). The Nix `rustc` should satisfy it; if you
pin a channel later, add a `rust-toolchain.toml` and record the decision.
