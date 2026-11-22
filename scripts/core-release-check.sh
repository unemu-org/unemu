#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# A released front end must name a released, version-matching PC core.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
exec python3 -I "$root/scripts/core-release-check.py" "$root/Cargo.lock" "${1:-}"
