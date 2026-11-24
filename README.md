<img src="assets/brand/unemupc-logo-512.png" width="128" alt="unemu logo">

# unemu

The **unemu application**: a desktop app and command line for unemu emulator
cores. This repository owns the application, its machine and media library,
OCI registry integration, recipes and host interface. It compiles in the
[PC core](https://github.com/unemu-org/unemupc) and the
[NES core](https://github.com/unemu-org/unemunes), each pinned to an exact git
revision in `Cargo.toml` and `Cargo.lock`.

The neutral session `machine-view` and synthetic `unemu-test-core` come from
[unemu-org/unemu-host](https://github.com/unemu-org/unemu-host), version 0.0.4,
pinned to `de86256cfd9d88aeb48c0d5480fa2838f0c152b8` in `Cargo.toml` and
`Cargo.lock`. Both use contract 0.6, pinned with `unemu-core-files` to
`a4ec90098e7ca0bc7ce60b2d20af43f140eb3960` (tag `v0.6.0`). They are not
workspace crates. Land their changes in unemu-host first, then re-pin this
workspace and run its gate, as for a core change.

The PC's CPU, devices, machine configuration and emulation tests live in
`unemupc`. Its CPU dependency and hardware development follow that
repository's rules. This repository does not copy the core's code.

## Build and run

```sh
nix develop
cargo build --locked -p window --bin unemu
cargo run --locked -p window --bin unemu -- --about
cargo test --locked --workspace
```

Running `unemu` with no command opens the desktop. Command words provide
headless machine execution and image, media, firmware and registry management;
`unemu --help` lists them. Never launch the desktop during automated checks.

Without Nix, use the Rust version named by `rust-version` in `Cargo.toml` and
a C compiler. Linux builds need `pkg-config` and ALSA development files
(`libasound2-dev` on Ubuntu). Installed Linux binaries need the ALSA runtime
(`libasound2` on Ubuntu 22.04), including for headless commands. The desktop
needs X11 or Wayland with OpenGL.

## Working with the PC core

```sh
bash scripts/core-checkout.sh           # pinned checkout and fixtures, using the core's flake
bash scripts/core-checkout.sh --print-root
bash scripts/core-patch.sh ../unemupc   # temporary Cargo override for a local core checkout
bash scripts/core-patch.sh --remove     # remove the override
```

`scripts/core-patch.sh` manages a gitignored override; do not replace the
committed git pins with path dependencies. Fixture tests use
`UNEMUPC_FIXTURE_ROOT` to locate the core's fixture tree. The front end owns
its FreeDOS 1.4 Floppy Edition and recipe fixtures. Do not commit built media.
Release builds require a tagged PC core release matching the locked package
version. Development builds with a local core override name that core's
repository as a local checkout in the source notice. Each core's packages must
all use the same git revision or all use a local checkout. Release-profile
builds require both cores' exact git sources in `Cargo.lock`; restore the pinned
lock after local override work before building a release.

## Downloads and licences

Front-end [releases](https://github.com/unemu-org/unemu/releases) ship
`unemu-<version>-<target>.tar.gz` for Linux x86-64 and ARM64 (glibc 2.35 or
newer), and macOS on Apple silicon and Intel.
Each archive holds `unemu`, `LICENSE`, `README.md`, `THIRD-PARTY-LICENSES.md`
and `notices/`, with `SHA256SUMS` alongside. The standalone `unemupc` program
comes from the PC core's own releases.

The [setup-unemu action](.github/actions/setup-unemu/action.yml) installs
`unemu` from this repository's releases and checks the archive checksum; its
optional commit input checks the build's source commit too. Pin the action
to a full commit when using it in another workflow.

[AGPL-3.0-or-later](LICENSE). `unemu --about` and `--version` name the front
end's source and each compiled-in core's locked source revision.
`scripts/third-party-licenses.sh` generates the full third-party texts,
including Nuked-OPL3 (LGPL-2.1-or-later) and the Ultimate Oldschool PC Font
Pack (CC BY-SA 4.0) from the PC core, plus the
[desktop font notices](crates/ui-kit/assets/fonts/egui/NOTICE.txt),
[DejaVu subset notice](crates/ui-kit/assets/fonts/dejavu/NOTICE.md) and
[CC0 logo notice](assets/brand/README.md).

## Development

Open work uses beads with prefix **`ufe`**; the PC core retains **`upc`**.
Knowledge and decisions live under [`backlog/`](backlog/). See
[AGENTS.md](AGENTS.md) for working rules,
[conventions](backlog/docs/conventions.md),
[commands](backlog/docs/commands.md) and
[the split design](backlog/docs/design/repo-split.md).

Everything committed is English and safe to publish. Never commit proprietary
firmware, guest software, manuals or screenshots, or private host names,
paths or personal setup details. Commits require the maintainer's explicit
request; publishing a repository is the maintainer's step.
