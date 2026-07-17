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
git -C "$SRC_DIR" fetch --depth 1 origin "$COMMIT" 2>/dev/null || git -C "$SRC_DIR" fetch origin
git -C "$SRC_DIR" checkout --quiet "$COMMIT"

# Build the Xi8088 configuration (MACHINE_XI8088): a full AT-style config with two
# cascaded 8259 PICs, 8042 keyboard controller, MC146818 RTC, and floppy INT 13h.
# The result is the raw 32 KiB BIOS that maps read-only at physical 0xF8000
# (F000:8000); its reset vector at offset 0x7FF0 (F000:FFF0) far-jumps to POST.
# This is the image the emulator loads — the *.rom flash images the Makefile also
# produces are 128 KiB programmer images for physical flash chips, not used here.
nasm -i"$SRC_DIR/src/" -DMACHINE_XI8088 -O9 -f bin -o "$OUT" \
	-l "$SRC_DIR/src/bios-xi8088.lst" "$SRC_DIR/src/bios.asm" -w-all

SIZE="$(stat -c %s "$OUT")"
if [ "$SIZE" -ne 32768 ]; then
	echo "unexpected bios-xi8088.bin size: $SIZE bytes (expected 32768)" >&2
	exit 1
fi
echo "built $OUT ($SIZE bytes) from $COMMIT"
echo "sha256: $(sha256sum "$OUT" | cut -d' ' -f1)"
