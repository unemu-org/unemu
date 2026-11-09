#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Fetch and build the Xi8088 configuration of Sergey Kiselev's 8088_bios, an
# open-source PC-class BIOS (see README.md). We use it as the boot firmware for
# the AT-style real-mode machine. The ROM is not committed — it is rebuilt from a
# pinned upstream commit so the boot is reproducible against a known BIOS.
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

# Pinned upstream revision. A golden boot trace is only valid against a known
# BIOS revision, so this is hard-coded rather than tracking a moving branch.
REPO="https://github.com/skiselev/8088_bios"
COMMIT="a4af458a72bc33c4be066b33bd76e8af2b24111c"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="$HERE/8088_bios"                  # gitignored clone
OUT="$HERE/bios-xi8088.bin"                # gitignored build product

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

# Build the Xi8088 configuration (MACHINE_XI8088): a full AT-style config with two
# cascaded 8259 PICs, 8042 keyboard controller, MC146818 RTC, and floppy INT 13h.
# The result is the raw 32 KiB BIOS that maps read-only at physical 0xF8000
# (F000:8000); its reset vector at offset 0x7FF0 (F000:FFF0) far-jumps to POST.
# This is the image the emulator loads — the *.rom flash images the Makefile also
# produces are 128 KiB programmer images for physical flash chips, not used here.
nasm -i"$SRC_DIR/src/" -DMACHINE_XI8088 -O9 -f bin -o "$OUT" \
	-l "$SRC_DIR/src/bios-xi8088.lst" "$SRC_DIR/src/bios.asm" -w-all

SIZE="$(wc -c < "$OUT" | tr -d '[:space:]')"
if [ "$SIZE" -ne 32768 ]; then
	echo "unexpected bios-xi8088.bin size: $SIZE bytes (expected 32768)" >&2
	exit 1
fi
check_sha256 "$OUT" "1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8"
echo "built $OUT ($SIZE bytes) from $COMMIT"
