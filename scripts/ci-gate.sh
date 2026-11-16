#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Run one Forgejo CI gate from the repository root, keeping its status and log.
set -euo pipefail

usage() {
    echo 'Usage: bash scripts/ci-gate.sh {devshell|fmt|guard|clippy|deny|tests|fixtures|goldens|user-roms}' >&2
    exit 2
}

[[ $# -eq 1 ]] || usage
step=$1
gate_command=(nix develop --no-update-lock-file --command)
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
case "$step" in
    devshell) gate_command+=(rustc --version) ;;
    fmt) gate_command+=(cargo fmt --check) ;;
    guard) gate_command=(bash scripts/publication-guard.sh) ;;
    clippy)
        gate_command+=(bash -euc '
            cargo clippy --locked --all-targets --all-features -- -D warnings
            cargo clippy --locked -p cli --bin unemupc98 -- -D warnings
        ')
        ;;
    deny) gate_command+=(cargo deny --all-features check advisories licenses bans sources) ;;
    tests)
        # Release builds keep a few behaviours that debug builds assert against
        # (devices ignore an earlier clock); run the device tests in both.
        gate_command+=(bash -euc '
            cargo test --locked --profile test --workspace
            cargo test --locked --release -p devices
        ')
        ;;
    fixtures)
        # The goldens gate requires every fixture its tests read. vgabios and
        # unemu-at-bios are built by Nix from their public GitHub repositories
        # over anonymous HTTPS (UNEMU_VGABIOS_FLAKE / UNEMU_VGABIOS_CHECKOUT and
        # UNEMU_AT_BIOS_FLAKE / UNEMU_AT_BIOS_CHECKOUT override the source).
        # The Kiselev, XTIDE, vgabios and unemu-at-bios
        # scripts also write the pinned source archives of their open
        # images. XTIDE runs after them: it exports its
        # source over the network with Subversion, which Nix supplies, so an
        # unreachable server cannot keep the other fixtures from being built.
        # The FreeDOS 1.4 Floppy Edition runs last for the same reason: its
        # first run downloads about 330 MB from FreeDOS, GitHub and GitLab,
        # and with the workflow's cache of its outputs it only re-checks pins.
        # The lane stops at the first failing script; the last header names it.
        gate_command+=(bash -euc '
            echo "== fixtures: vendor/kiselev-bios/fetch.sh"
            bash vendor/kiselev-bios/fetch.sh
            echo "== fixtures: vendor/freedos/fetch.sh"
            bash vendor/freedos/fetch.sh
            echo "== fixtures: vendor/test386/fetch.sh"
            bash vendor/test386/fetch.sh
            echo "== fixtures: vendor/hdd/build.sh"
            bash vendor/hdd/build.sh
            echo "== fixtures: vendor/vgabios/fetch.sh"
            bash vendor/vgabios/fetch.sh
            echo "== fixtures: vendor/unemu-at-bios/fetch.sh"
            bash vendor/unemu-at-bios/fetch.sh
            echo "== fixtures: vendor/at-pm-test/build.sh"
            bash vendor/at-pm-test/build.sh
            echo "== fixtures: vendor/xtide-universal-bios/fetch.sh"
            bash vendor/xtide-universal-bios/fetch.sh
            echo "== fixtures: vendor/freedos/fetch-floppy-1.4.sh"
            bash vendor/freedos/fetch-floppy-1.4.sh
        ')
        ;;
    # The tests lane runs before the fixtures exist, so tests that read a
    # fixture skip there. This lane runs them again with every fixture
    # present, next to the slow ignored goldens.
    goldens) gate_command+=(env UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --locked --profile test --workspace -- --include-ignored) ;;
    # Local opt-in lane: CI never calls it and never has user-supplied firmware.
    user-roms) gate_command+=(cargo test --locked --profile test --workspace --no-fail-fast -- --ignored user_rom_) ;;
    *) usage ;;
esac

log_dir=${CI_LOG_DIR:-ci-logs}
mkdir -p "$log_dir"
# pipefail keeps a failed gate red even when tee successfully writes its log.
"${gate_command[@]}" 2>&1 | tee "$log_dir/$step.log"
