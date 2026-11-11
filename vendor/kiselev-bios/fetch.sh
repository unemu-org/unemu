#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Fetch and build the Xi8088 configuration of Sergey Kiselev's 8088_bios, an
# open-source PC-class BIOS (see README.md). We use it as the boot firmware for
# the AT-style real-mode machine. The ROM is not committed — it is rebuilt from a
# pinned upstream commit so the boot is reproducible against a known BIOS.
#
# Requires git, nasm, patch and tar on PATH (all are in the flake devShell).
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
OUTPUT_SHA256="1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8"
ATRESET_OUTPUT_SHA256="61589573c9809273198b137d9419c87242b0a5f63c2ad834a9565950c3d6becc"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="$HERE/8088_bios"                  # gitignored clone
OUT="$HERE/bios-xi8088.bin"                # gitignored build product
ATRESET_OUT="$HERE/bios-xi8088-atreset.bin"

if [ ! -d "$SRC_DIR/.git" ]; then
	git clone "$REPO" "$SRC_DIR"
fi
# A cached commit is sufficient for offline builds; fetch only missing objects.
if ! git -C "$SRC_DIR" cat-file -e "$COMMIT^{commit}" 2>/dev/null; then
	git -C "$SRC_DIR" fetch --depth 1 origin "$COMMIT" 2>/dev/null \
		|| git -C "$SRC_DIR" fetch origin
fi
# Export the exact commit without changing a cached checkout or its work files.
mkdir -p "$HERE/build"
BUILD_DIR="$(mktemp -d "$HERE/build/xi8088.XXXXXX")"
trap 'rm -rf "$BUILD_DIR"' EXIT
git -C "$SRC_DIR" archive "$COMMIT" | tar -x -C "$BUILD_DIR"

# Build the Xi8088 configuration (MACHINE_XI8088): a full AT-style config with two
# cascaded 8259 PICs, 8042 keyboard controller, MC146818 RTC, and floppy INT 13h.
# The result is the raw 32 KiB BIOS that maps read-only at physical 0xF8000
# (F000:8000); its reset vector at offset 0x7FF0 (F000:FFF0) far-jumps to POST.
# This is the image the emulator loads — the *.rom flash images the Makefile also
# produces are 128 KiB programmer images for physical flash chips, not used here.
build_image() {
	local output="$1" expected="$2" temporary="$BUILD_DIR/bios.bin" size
	nasm -i"$BUILD_DIR/src/" -DMACHINE_XI8088 -O9 -f bin -o "$temporary" \
		-l "$BUILD_DIR/bios.lst" "$BUILD_DIR/src/bios.asm" -w-all
	size="$(wc -c < "$temporary" | tr -d '[:space:]')"
	if [ "$size" -ne 32768 ]; then
		echo "unexpected BIOS size: $size bytes (expected 32768)" >&2
		exit 1
	fi
	check_sha256 "$temporary" "$expected"
	mv "$temporary" "$output"
	echo "built $output ($size bytes) from $COMMIT"
}

build_image "$OUT" "$OUTPUT_SHA256"
patch --batch --forward --fuzz=0 -p1 -d "$BUILD_DIR" < "$HERE/at-reset.patch"
build_image "$ATRESET_OUT" "$ATRESET_OUTPUT_SHA256"
