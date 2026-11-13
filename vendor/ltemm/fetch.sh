#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Fetch a local-only fixture; neither archive nor driver may be redistributed.
set -euo pipefail

if command -v sha256sum >/dev/null 2>&1; then
    SHA256=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
    SHA256=(shasum -a 256)
else
    echo "missing SHA-256 tool: install sha256sum or shasum (or use nix develop)" >&2
    exit 1
fi
check_sha256() {
    local file="$1" expected="$2" actual
    actual="$("${SHA256[@]}" "$file" | awk '{print $1}')"
    if [ "$actual" != "$expected" ]; then
        echo "SHA-256 mismatch for $file: expected $expected, got $actual" >&2
        return 1
    fi
}
ARCHIVE_SHA256="0bcbcac26dd21da6125da79d74107905989c092f1639416ba90219001602383a"
OUTPUT_SHA256="457c9a849f87749a067d4ef23477106de4abd575eb4d5cd0ff8755aeaa80956d"
UPSTREAM="https://www.lo-tech.co.uk/downloads/2MB-EMS-Board/LTEMM-r01.zip"
FALLBACK="https://web.archive.org/web/20260820161725/https://www.lo-tech.co.uk/downloads/2MB-EMS-Board/LTEMM-r01.zip"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d "$HERE/.extract.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
ARCHIVE="${LTEMM_ARCHIVE:-$WORK/LTEMM-r01.zip}"
if [ -z "${LTEMM_ARCHIVE:-}" ]; then
    if ! curl -fsSL -o "$ARCHIVE" "$UPSTREAM" || ! check_sha256 "$ARCHIVE" "$ARCHIVE_SHA256"; then
        curl -fsSL -o "$ARCHIVE" "$FALLBACK"
        check_sha256 "$ARCHIVE" "$ARCHIVE_SHA256"
    fi
else
    check_sha256 "$ARCHIVE" "$ARCHIVE_SHA256"
fi
unzip -p "$ARCHIVE" LTEMM.EXE > "$WORK/LTEMM.EXE"
check_sha256 "$WORK/LTEMM.EXE" "$OUTPUT_SHA256"
mv "$WORK/LTEMM.EXE" "$HERE/LTEMM.EXE"
echo "verified local fixture: vendor/ltemm/LTEMM.EXE"
