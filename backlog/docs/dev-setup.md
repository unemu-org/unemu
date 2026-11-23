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
`rustfmt`, `rust-analyzer`, `rust-src`) as the latest stable release from the
`rust-overlay` input — upstream binaries, so a Rust update never rebuilds the
rest of nixpkgs — plus `jq` and the image and recipe fixture tools. Enter with `nix develop` or let direnv auto-load it.

The fixture scripts under `vendor/` take their tools from the same devShell:
`mtools` 4.0.49, `dosfstools`, `curl` and Info-ZIP `unzip` 6.00, so macOS and
Linux run the same versions. The devShell also points `NIX_SSL_CERT_FILE` and
`CURL_CA_BUNDLE` at the pinned nixpkgs CA bundle, so downloads verify against
the same roots everywhere rather than the host's bundle (an old container bundle
could not verify a FreeDOS mirror's current root). `SSL_CERT_FILE` stays unset:
the registry client's native-roots loader would read that file instead of the
macOS keychain. A host `SSL_CERT_FILE` still applies to tools that read it.

### The PC core: unemupc

unemu maintains host tools and recipe fixtures; [unemupc](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/dev-setup.md)
maintains PC hardware, the transitive x86jit pin and hardware fixture toolchains.
All 13 PC-core git dependencies in `Cargo.toml` name the same exact revision,
`fc9c652e6876e7f0d189bf11d178f303ada19b13`, also recorded in `Cargo.lock`.

```sh
bash scripts/core-patch.sh ../unemupc          # gitignored local override
bash scripts/core-patch.sh --remove
bash scripts/core-checkout.sh                 # pinned checkout and fixtures
bash scripts/core-checkout.sh --print-root
```

`UNEMUPC_FIXTURE_ROOT` is the absolute directory holding the verified pinned
core checkout and its built fixtures. Front-end tests obtain core fixture paths
through `pc_harness::test_support::root()`; missing required fixtures fail with
`UNEMUPC_REQUIRE_FIXTURES=1`. Built files under `.core/` and `vendor/` stay
untracked. The front end owns the FreeDOS 1.4 and recipe fixture scripts.
Never commit a bare path dependency. Land changes in the core before moving
all PC pins together and rerunning the front-end gate. The NES core is likewise
pinned, to `ba0ece5d1bdad3d4ec813d5a795a54175118a46b`.

### Host libraries: unemu-host

The neutral session `machine-view` and synthetic `unemu-test-core` come from
[unemu-org/unemu-host](https://github.com/unemu-org/unemu-host), version 0.0.4,
pinned to `1692a1fcfd6c9900d3074786a54f51c9de3b30f0` in `Cargo.toml` and
`Cargo.lock`. They are not workspace crates. Land their changes in unemu-host
first, then re-pin this workspace and run its gate, as for a core change.

For a local checkout, add both entries to the gitignored `.cargo/config.toml`
(alongside a core patch if needed):

```toml
[patch."https://github.com/unemu-org/unemu-host"]
machine-view = { path = "../unemu-host/crates/machine-view" }
unemu-test-core = { path = "../unemu-host/crates/test-core" }
```

Paths are relative to the workspace root. Remove this patch section after local
work and restore the pinned lock before a release. `scripts/core-patch.sh`
manages only the PC-core section; apply it before adding a host override,
because it refuses to replace an unmarked configuration. Keep a host section
separate from its generated file when switching or removing core overrides:
the helper replaces or removes the whole marked file. The affected-test planner
selects the full golden tier for any local patch. The boundary checker still
requires the neutral host libraries and desktop crates to be PC-free, and the
resolved graph to contain exactly one `unemu-core-api`.

### The contract: unemu-core-api

The contract between front ends and emulator cores, `unemu-core-api`, lives in its own repository,
[unemu-org/unemu-core-api](https://github.com/unemu-org/unemu-core-api) (`MIT OR Apache-2.0`), and
is pinned with `unemu-core-files` to contract 0.5 at
`316dd2a1f429cc3ef37a2bed203e89b2160079ff` (tag `v0.5.0`): a git dependency
at the exact commit of a release tag, never a bare `path` dependency. To work against a local checkout, add an override in the
gitignored `.cargo/config.toml`:

```toml
[patch."https://github.com/unemu-org/unemu-core-api"]
unemu-core-api = { path = "<your unemu-core-api checkout>" }
```

The package is at the repository's root. The override applies to the whole graph, so a pinned core
that names the same URL builds against the checkout too. A build must hold exactly one copy of the
contract; `scripts/crate-boundaries.sh` fails otherwise and names each copy's source. Moving the pin
is its own bead: the contract and every pinned core move in one commit (core contract §13.1).

## CI

GitHub Actions runs `.github/workflows/ci.yml` on pushes to `main`, on pull
requests and via `workflow_dispatch`, on GitHub-hosted `ubuntu-latest`: an x86-64
Linux VM with systemd. The job first removes preinstalled toolchains it never uses
(Android SDK, .NET, Haskell, CodeQL, Swift), because the Nix store, the `target/`
directory, the caches and the fixture downloads outgrow the runner's free disk.
It installs Nix with `cachix/install-nix-action` (multi-user daemon, build sandbox
on), then uses the flake devShell. Every action is pinned to a full commit SHA,
with its release in a comment. The workflow has read-only repository permissions
and no secrets, and keeps the job token out of `.git/config` and `nix.conf`, so
pull requests from forks run the same gates. Cargo's registry, git database and
`target/` are cached using `flake.lock` and `Cargo.lock`; CI never needs
dependency credentials. Read runs with `gh run list --repo unemu-org/unemu`
and `gh run view <run-id> --repo unemu-org/unemu`; see
[`commands.md`](commands.md#ci).

The gates check formatting, publication safety, crate boundaries, Clippy,
workspace tests, dependency policy, fixtures and slow goldens through
`scripts/ci-gate.sh`. Core fixtures are built in the pinned checkout's own
shell; recipe fixtures use this repository's tools. See [commands](commands.md#ci).

## beads (`bd`)

Open work is tracked with [beads](https://github.com/steveyegge/beads) (`bd`). It is **not** in the
devShell: x86jit's database was created by `bd` 1.3.1 and the pinned nixpkgs carries an older one;
use the same host-installed `bd` ≥ 1.3.1 for all repositories (e.g. `brew install beads`, or a release
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

The workspace follows the latest stable Rust: `rust-version` in
`[workspace.package]` equals the devShell's `rustc`. To move to a newer release,
run `nix flake update rust-overlay`, set `rust-version` to the new `rustc
--version`, fix any new clippy lints, and run the full gate including the slow
goldens. The toolchain must also satisfy both pinned cores and the contract.
