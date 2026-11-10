#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Prepare the pinned XTIDE Universal BIOS AT option ROM (see README.md).
# The upstream binary is shorter than its declared ROM size and has no valid
# checksum. Keep its bytes intact, pad with zeros, and checksum the last byte.
# Requires curl (only for a missing cache) and standard shell tools from the
# flake devShell. Firmware bytes and cached downloads are never committed.
set -euo pipefail

# Both GNU/Linux and macOS provide one of these SHA-256 tools.
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
		if [ "${3:-output}" = archive ]; then
			echo "Remove the cached archive and fetch the pinned release again; do not change its expected hash." >&2
		else
			echo "Check the pinned sources and toolchain; deliberately rebuild the goldens' expectations before updating the expected output hash." >&2
		fi
		exit 1
	fi
}

URL="https://www.xtideuniversalbios.org/binaries/r638/ide_at.bin"
UPSTREAM_SHA256="6d5007bca3be0bffeb210969a299b67199368b4845f554b5a00715e2718dfea8"
OUTPUT_SHA256="6409de912eab620ea144b79e723bed478814047dc2766fc19fd6e2e3690662d0"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$HERE/build"
RAW="$WORK/ide_at.bin"
OUT="$HERE/ide_at-r638-8k.bin"
mkdir -p "$WORK"

if [ ! -f "$RAW" ]; then
	curl -fsSL -o "$RAW" "$URL"
fi
check_sha256 "$RAW" "$UPSTREAM_SHA256" archive

RAW_SIZE="$(wc -c < "$RAW" | tr -d '[:space:]')"
if [ "$RAW_SIZE" -ne 6646 ]; then
	echo "unexpected upstream size: $RAW_SIZE bytes (expected 6646)" >&2
	exit 1
fi
HEADER="$(od -An -tx1 -N3 "$RAW" | tr -d ' \n')"
if [ "$HEADER" != "55aa10" ]; then
	echo "unexpected option-ROM header: $HEADER (expected 55aa10, length 16 x 512 bytes)" >&2
	exit 1
fi

TMP="$(mktemp "$WORK/ide_at-r638-8k.XXXXXX")"
trap 'rm -f "$TMP"' EXIT
# Zero padding leaves the downloaded payload intact and gives the checksum a
# neutral starting value; the last byte belongs to the checksum, not the pad.
dd if=/dev/zero of="$TMP" bs=8192 count=1 2>/dev/null
dd if="$RAW" of="$TMP" conv=notrunc 2>/dev/null
byte_sum() {
	od -An -v -tu1 "$1" | awk '{for (i = 1; i <= NF; i++) sum += $i} END {print sum % 256}'
}
SUM="$(byte_sum "$TMP")"
CHECKSUM=$(( (256 - SUM) % 256 ))
printf '%b' "\\$(printf '%03o' "$CHECKSUM")" \
	| dd of="$TMP" bs=1 seek=8191 count=1 conv=notrunc 2>/dev/null

SIZE="$(wc -c < "$TMP" | tr -d '[:space:]')"
if [ "$SIZE" -ne 8192 ] || [ "$(byte_sum "$TMP")" -ne 0 ]; then
	echo "invalid option ROM: expected 8192 bytes with byte sum 0 mod 256" >&2
	exit 1
fi
check_sha256 "$TMP" "$OUTPUT_SHA256"
mv "$TMP" "$OUT"
echo "built $OUT ($SIZE bytes) from r638 ide_at.bin; header 55aa10, byte sum 0"
