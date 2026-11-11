#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Run one Forgejo CI gate from the repository root, keeping its status and log.
set -euo pipefail

usage() {
    echo 'Usage: bash scripts/ci-gate.sh {devshell|fmt|guard|clippy|deny|tests|fixtures|goldens}' >&2
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
            cargo test --locked --workspace
            cargo test --locked --release -p devices
        ')
        ;;
    fixtures)
        gate_command+=(bash -euc '
            bash vendor/kiselev-bios/fetch.sh
            bash vendor/freedos/fetch.sh
            bash vendor/test386/fetch.sh
        ')
        ;;
    goldens) gate_command+=(env UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --locked --workspace -- --ignored) ;;
    *) usage ;;
esac

log_dir=${CI_LOG_DIR:-ci-logs}
mkdir -p "$log_dir"
# pipefail keeps a failed gate red even when tee successfully writes its log.
"${gate_command[@]}" 2>&1 | tee "$log_dir/$step.log"
