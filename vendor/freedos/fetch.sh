#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build a bootable 1.44 MB FreeDOS floppy image (8086 real-mode build) used here
# as read-only boot fixture (see README.md). Nothing is committed — the archives
# are fetched from pinned FreeDOS releases and the image is rebuilt from them, so
# the boot is reproducible against known kernel / shell / boot-sector revisions.
#
# The image is FAT12 (1,474,560 bytes) with a FreeDOS FAT12 boot sector, the 8086
# KERNEL.SYS, and the 8086 FreeCOM COMMAND.COM in the root. FreeDOS is GPLv2+ and
# redistributable; this project only reads the image, never links its code.
#
# Requires `git`, `nasm`, `unzip`, `curl`, and mtools (`mformat`/`mcopy`/`mdir`)
# on PATH — all in the flake devShell.
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

# Pinned upstream revisions. A boot is only reproducible against known versions,
# so these are hard-coded rather than tracking moving branches.
#
# 8086 kernel build (FAT12/16, no FAT32, real-mode 8086 — the default installer
# kernel is 386-only and would fault on our real-mode core).
KERNEL_TAG="ke2044"
KERNEL_ZIP="ke2044_86f16.zip"
KERNEL_ZIP_SHA256="367c170c14412fc256b118fcee4906c2ae724c35f28d1496fa6beed1d78237eb"
# 8086 FreeCOM shell build.
FREECOM_TAG="com086"
FREECOM_ZIP="command.zip"
FREECOM_ZIP_SHA256="b0cbfc65f58a2f0f2b88f172bf2a99a306b761ed5b10edcc668f9f047cf42f68"
# Kernel source at the ke2044 tag, for assembling the FAT12 boot sector from
# boot/boot.asm (the release archives ship SYS.COM but no standalone boot binary).
KERNEL_REPO="https://github.com/FDOS/kernel"
KERNEL_COMMIT="8535ddb7a783f94b3aae844c8fe7f5ccce9037eb"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$HERE/build"                     # gitignored scratch (downloads + clone)
SRC_DIR="$WORK/kernel-src"             # gitignored clone (boot-sector source)
OUT="$HERE/freedos.img"                # gitignored build product

mkdir -p "$WORK"

fetch_release() {
	# $1 repo (kernel|freecom), $2 tag, $3 asset, $4 sha256
	local dest="$WORK/$3"
	if [ ! -f "$dest" ]; then
		curl -fsSL -o "$dest" \
			"https://github.com/FDOS/${1}/releases/download/${2}/${3}"
	fi
	check_sha256 "$dest" "$4" archive
}

fetch_release "kernel" "$KERNEL_TAG" "$KERNEL_ZIP" "$KERNEL_ZIP_SHA256"
fetch_release "freecom" "$FREECOM_TAG" "$FREECOM_ZIP" "$FREECOM_ZIP_SHA256"

# --- assemble the FAT12 boot sector from pinned kernel source ---------------
if [ ! -d "$SRC_DIR/.git" ]; then
	git clone "$KERNEL_REPO" "$SRC_DIR"
fi
# A cached commit is sufficient for offline builds; fetch only missing objects.
if ! git -C "$SRC_DIR" cat-file -e "$KERNEL_COMMIT^{commit}" 2>/dev/null; then
	git -C "$SRC_DIR" fetch --depth 1 origin "$KERNEL_COMMIT" 2>/dev/null \
		|| git -C "$SRC_DIR" fetch origin
fi
if [ "$(git -C "$SRC_DIR" rev-parse HEAD)" != "$KERNEL_COMMIT" ]; then
	git -C "$SRC_DIR" checkout --quiet "$KERNEL_COMMIT"
fi
# boot.asm selects FAT12 with -dISFAT12; it includes only the local magic.mac and
# has its own `org`, so it assembles standalone into a 512-byte sector image.
nasm -dISFAT12 -f bin -I"$SRC_DIR/boot/" "$SRC_DIR/boot/boot.asm" -o "$WORK/fat12com.bin"
BOOT_SIZE="$(wc -c < "$WORK/fat12com.bin" | tr -d '[:space:]')"
if [ "$BOOT_SIZE" -ne 512 ]; then
	echo "unexpected boot-sector size: $BOOT_SIZE bytes (expected 512)" >&2
	exit 1
fi

# --- lay down the FAT12 filesystem ------------------------------------------
unzip -o -j "$WORK/$KERNEL_ZIP" bin/kernel.sys -d "$WORK/extract" >/dev/null
unzip -o -j "$WORK/$FREECOM_ZIP" BIN/COMMAND.COM -d "$WORK/extract" >/dev/null

export MTOOLS_SKIP_CHECK=1
# mtools uses this epoch for volume-label, creation, access and modification
# timestamps. Do not use mcopy -m: it would preserve archive/host file mtimes.
# UTC avoids host timezone differences; the FAT epoch is representable everywhere.
export SOURCE_DATE_EPOCH=315532800 TZ=UTC
rm -f "$OUT"
# 2880 sectors * 512 = 1,474,560 bytes (standard 1.44 MB), formatted FAT12. This
# writes a correct 1.44 MB BPB and a (non-FreeDOS) boot stub we replace below.
dd if=/dev/zero of="$OUT" bs=512 count=2880 2>/dev/null
mformat -i "$OUT" -f 1440 -N 0x28959853 -v FREEDOS ::
mcopy -i "$OUT" "$WORK/extract/kernel.sys" ::KERNEL.SYS
mcopy -i "$OUT" "$WORK/extract/COMMAND.COM" ::COMMAND.COM

# --- install the FreeDOS boot code, preserving the formatted BPB ------------
# A DOS boot sector is: jump + OEM name + BPB (offsets 0x0B..0x3D) + boot code
# (0x3E..0x1FD) + 0x55AA signature. mformat wrote a valid 1.44 MB BPB and the
# signature; overwrite only the code region [0x3E, 0x1FE) with FreeDOS's, so the
# loader runs against the real geometry the filesystem was formatted with.
dd if="$WORK/fat12com.bin" of="$OUT" bs=1 skip=62 seek=62 count=448 conv=notrunc 2>/dev/null

# --- verify -----------------------------------------------------------------
SIZE="$(wc -c < "$OUT" | tr -d '[:space:]')"
if [ "$SIZE" -ne 1474560 ]; then
	echo "unexpected image size: $SIZE bytes (expected 1474560)" >&2
	exit 1
fi
SIG="$(dd if="$OUT" bs=1 skip=510 count=2 2>/dev/null | od -An -tx1 | tr -d ' \n')"
if [ "$SIG" != "55aa" ]; then
	echo "boot signature missing: got 0x$SIG (expected 0x55aa)" >&2
	exit 1
fi
check_sha256 "$OUT" "b7b088333f7f30584d768941fecea8288b63f2d61e13ec6c9472439eda66e51f"
echo "built $OUT ($SIZE bytes), boot signature 0x$SIG"
mdir -i "$OUT" ::
