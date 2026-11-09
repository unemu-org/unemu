#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Fetch and build test386.bin, a third-party 386 CPU-test ROM used here as
# read-only golden test data (see README.md). The image is not committed — it is
# rebuilt from a pinned upstream commit so the smoke test is reproducible.
#
# Requires `git` and `nasm` on PATH (both are in the flake devShell).
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
		echo "Check the pinned sources and toolchain; deliberately rebuild the goldens' expectations before updating the expected output hash." >&2
		exit 1
	fi
}

# Pinned upstream revision. test386.bin is only valid golden data against a known
# source revision, so this is hard-coded rather than tracking a moving branch.
REPO="https://github.com/barotto/test386.asm"
COMMIT="cfd052d1e64d5375dea5a681c1eadeed64ceda2c"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="$HERE/test386.asm"        # gitignored clone
OUT="$HERE/test386.bin"            # gitignored build product

if [ ! -d "$SRC_DIR/.git" ]; then
	git clone "$REPO" "$SRC_DIR"
fi
# A cached commit is sufficient for offline builds; fetch only missing objects.
if ! git -C "$SRC_DIR" cat-file -e "$COMMIT^{commit}" 2>/dev/null; then
	git -C "$SRC_DIR" fetch --depth 1 origin "$COMMIT" 2>/dev/null \
		|| git -C "$SRC_DIR" fetch origin
fi
if [ "$(git -C "$SRC_DIR" rev-parse HEAD)" != "$COMMIT" ]; then
	git -C "$SRC_DIR" checkout --quiet "$COMMIT"
fi

# Default configuration builds a 64 KiB image whose reset vector at offset 0xFFF0
# far-jumps to the POST entry, exactly the layout the runner loads at 0xF0000.
nasm -i"$SRC_DIR/src/" -f bin "$SRC_DIR/src/test386.asm" -w-all -o "$OUT"

SIZE="$(wc -c < "$OUT" | tr -d '[:space:]')"
if [ "$SIZE" -ne 65536 ]; then
	echo "unexpected test386.bin size: $SIZE bytes (expected 65536)" >&2
	exit 1
fi
check_sha256 "$OUT" "40cbbe1be2235cb0e43e28d8ae57dd6a4d3200ad6870d3b2ba31330e6760e7b8"
echo "built $OUT ($SIZE bytes) from $COMMIT"
