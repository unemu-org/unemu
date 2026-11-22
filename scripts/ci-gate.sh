#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Run one CI gate from the repository root, keeping its status and log.
set -euo pipefail

usage() {
    echo 'Usage: bash scripts/ci-gate.sh {devshell|fmt|guard|clippy|deny|tests|fixtures|goldens|recipe-conformance|user-roms}' >&2
    exit 2
}

[[ $# -eq 1 ]] || usage
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
step=$1
case "$step" in
    fixtures|goldens|recipe-conformance|user-roms)
        if override=$(bash scripts/core-patch.sh --active); then
            echo "ci gate: $override overrides the core; fixture lanes require the pinned graph; use bead-gate.sh for the patched checkout" >&2
            exit 1
        fi
        ;;
esac
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
case "$step" in
    devshell) lane_command=(rustc --version) ;;
    fmt) lane_command=(cargo fmt --check) ;;
    guard)
        # Check every guard even after a failure so the log names every finding.
        lane_command=(bash -uc '
            status=0
            # A cold CI cache needs the locked sources before the offline check.
            if [[ ${CI_GATE_OFFLINE:-0} == 1 ]]; then
                cargo fetch --locked --offline || status=1
            else
                cargo fetch --locked || status=1
            fi
            bash scripts/publication-guard-test.sh && bash scripts/publication-guard.sh || status=1
            bash scripts/crate-boundaries.sh --self-test || status=1
            bash scripts/crate-boundaries.sh || status=1
            bash scripts/core-patch.sh --self-test || status=1
            bash scripts/core-checkout.sh --self-test || status=1
            bash scripts/uid-guard.sh --self-test || status=1
            exit "$status"
        ')
        ;;
    clippy)
        lane_command=(bash -euc '
            cargo clippy --locked --all-targets --all-features -- -D warnings
            cargo clippy --locked -p window --bin unemu -- -D warnings
        ')
        ;;
    deny) lane_command=(cargo deny --all-features check advisories licenses bans sources) ;;
    tests) lane_command=(cargo test --locked --profile test --workspace) ;;
    recipe-conformance)
        lane_command=(bash -euc '
            fixture=$(bash scripts/core-checkout.sh --print-root)
            export "$fixture"
            cargo run --locked -p window -- recipe conformance \
                crates/pc-image/data/recipes \
                backlog/docs/design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml \
                --library vendor/freedos --library vendor \
                --library "$UNEMUPC_FIXTURE_ROOT/vendor"
        ')
        ;;
    fixtures)
        # The core owns the build tools and pins of its fixtures. Keep the
        # large FreeDOS edition download last, as in the original lane: an
        # unavailable mirror must not prevent the other fixtures being built.
        # Each header names the script if the lane stops at a network failure.
        lane_command=(bash -euc '
            echo "== fixtures: scripts/core-checkout.sh"
            bash scripts/core-checkout.sh
            echo "== fixtures: vendor/kenslab/fetch-labfull.sh"
            bash vendor/kenslab/fetch-labfull.sh
            echo "== fixtures: vendor/freedos/fetch-floppy-1.4.sh"
            bash vendor/freedos/fetch-floppy-1.4.sh
        ')
        ;;
    # Tests before fixture provisioning may skip fixture reads. Goldens must
    # use the verified checkout and require every fixture.
    goldens)
        lane_command=(bash -euc '
            fixture=$(bash scripts/core-checkout.sh --print-root) || {
                echo "goldens: run bash scripts/core-checkout.sh to provision the pinned core fixtures" >&2
                exit 1
            }
            export "$fixture"
            UNEMUPC_REQUIRE_FIXTURES=1 cargo test --locked --profile test --workspace -- --include-ignored --skip user_rom_
        ')
        ;;
    # Local opt-in lane: CI never has user-supplied firmware.
    user-roms)
        lane_command=(bash -euc '
            fixture=$(bash scripts/core-checkout.sh --print-root)
            export "$fixture"
            cargo test --locked --profile test --workspace --no-fail-fast -- --ignored user_rom_
        ')
        ;;
    *) usage ;;
esac

# A supervisor can supply an already loaded devShell and a shared cargo wrapper.
gate_command=()
if [[ ${CI_GATE_IN_DEV_SHELL:-0} != 1 ]]; then
    gate_command=(nix develop --no-update-lock-file --command)
fi
gate_command+=(bash -euc '
    cargo() {
        local cargo_cmd
        read -r -a cargo_cmd <<< "${AFFECTED_CARGO:-cargo}"
        command "${cargo_cmd[@]}" "$@"
    }
    export -f cargo
    "$@"
' -- "${lane_command[@]}")
log_dir=${CI_LOG_DIR:-ci-logs}
mkdir -p "$log_dir"
# pipefail keeps a failed gate red even when tee successfully writes its log.
"${gate_command[@]}" 2>&1 | tee "$log_dir/$step.log"
