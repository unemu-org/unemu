#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Fetch and build test386.bin, a third-party 386 CPU-test ROM used here as
# read-only golden test data (see README.md). The image is not committed — it is
# rebuilt from a pinned upstream commit so the smoke test is reproducible.
#
# Requires `git` and `nasm` on PATH (both are in the flake devShell).
set -euo pipefail

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
git -C "$SRC_DIR" fetch --depth 1 origin "$COMMIT" 2>/dev/null || git -C "$SRC_DIR" fetch origin
git -C "$SRC_DIR" checkout --quiet "$COMMIT"

# Default configuration builds a 64 KiB image whose reset vector at offset 0xFFF0
# far-jumps to the POST entry, exactly the layout the runner loads at 0xF0000.
nasm -i"$SRC_DIR/src/" -f bin "$SRC_DIR/src/test386.asm" -w-all -o "$OUT"

SIZE="$(stat -c %s "$OUT")"
if [ "$SIZE" -ne 65536 ]; then
	echo "unexpected test386.bin size: $SIZE bytes (expected 65536)" >&2
	exit 1
fi
echo "built $OUT ($SIZE bytes) from $COMMIT"
