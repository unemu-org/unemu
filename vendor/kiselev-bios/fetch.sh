#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Fetch and build the Xi8088 configuration of Sergey Kiselev's 8088_bios, an
# open-source PC-class BIOS (see README.md). We use it as the boot firmware for
# the AT-style real-mode machine. The ROM is not committed — it is rebuilt from a
# pinned upstream commit so the boot is reproducible against a known BIOS.
# The script builds the original image and an AT reset variant, then writes
# the source of both as a deterministic tar archive (upstream at the pinned
# commit with our patch applied, the patch and a NOTICE).
#
# Requires git, nasm, patch and GNU tar on PATH (all are in the flake devShell).
set -euo pipefail

# BSD tar (the macOS default) cannot fix member order, owners and modes, and
# GNU tar would take extra options from TAR_OPTIONS.
unset TAR_OPTIONS
case "$(tar --version 2>/dev/null)" in
	*"GNU tar"*) ;;
	*)
		echo "missing GNU tar: run this script inside nix develop" >&2
		exit 1
		;;
esac

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
		if [ "${3:-output}" = source ]; then
			echo "Check the pinned commit, at-reset.patch, source-notice.txt and the GNU tar version before updating the source archive hash." >&2
		elif [ "${3:-output}" = rebuild ]; then
			echo "The archived source no longer builds the shipped AT reset image; check the export and patch steps before writing the archive." >&2
		else
			echo "Check the pinned sources and toolchain; deliberately rebuild the goldens' expectations before updating the expected output hash." >&2
		fi
		exit 1
	fi
}

# Pinned upstream revision. A golden boot trace is only valid against a known
# BIOS revision, so this is hard-coded rather than tracking a moving branch.
REPO="https://github.com/skiselev/8088_bios"
COMMIT="a4af458a72bc33c4be066b33bd76e8af2b24111c"
OUTPUT_SHA256="1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8"
ATRESET_OUTPUT_SHA256="61589573c9809273198b137d9419c87242b0a5f63c2ad834a9565950c3d6becc"
SOURCE_ARCHIVE_SHA256="ced63707661747fc045d1e6e565a5cc9be73f95a9c944accedd77b19467fb5f9"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="$HERE/8088_bios"                  # gitignored clone
OUT="$HERE/bios-xi8088.bin"                # gitignored build product
ATRESET_OUT="$HERE/bios-xi8088-atreset.bin"
SOURCE_NAME="kiselev-bios-${COMMIT:0:7}-atreset-src"
SOURCE_OUT="$HERE/$SOURCE_NAME.tar"

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
SOURCE_DIR="$(mktemp -d "$HERE/build/source.XXXXXX")"
trap 'rm -rf "$BUILD_DIR" "$SOURCE_DIR"' EXIT
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

# The archive carries the source of both images: the patched tree builds the
# AT reset image, and reversing the patch restores the original. Upstream's
# prebuilt images (binaries/, and XTIDE Universal BIOS r625 binaries in
# xtide_r625/) are build outputs that neither image uses, so they stay out.
# Line-ending and attribute settings are fixed so a host's git configuration
# cannot change the exported bytes.
STAGE="$SOURCE_DIR/$SOURCE_NAME"
mkdir -p "$STAGE/8088_bios"
git -C "$SRC_DIR" -c core.autocrlf=false -c core.eol=lf -c core.attributesFile=/dev/null \
	archive "$COMMIT" -- . ':(exclude)binaries' ':(exclude)xtide_r625' \
	| tar -x -C "$STAGE/8088_bios"
patch --batch --forward --fuzz=0 --no-backup-if-mismatch -p1 -d "$STAGE/8088_bios" \
	< "$HERE/at-reset.patch"
cp "$HERE/at-reset.patch" "$STAGE/at-reset.patch"
cp "$HERE/source-notice.txt" "$STAGE/NOTICE"
# The archived tree must still build the shipped AT reset image; the output
# goes outside the tree so it is not archived.
nasm -i"$STAGE/8088_bios/src/" -DMACHINE_XI8088 -O9 -f bin -w-all \
	-o "$SOURCE_DIR/atreset-check.bin" "$STAGE/8088_bios/src/bios.asm"
check_sha256 "$SOURCE_DIR/atreset-check.bin" "$ATRESET_OUTPUT_SHA256" rebuild
# A configured signature check would print into the format output.
SOURCE_EPOCH="$(git -C "$SRC_DIR" -c log.showSignature=false \
	log -1 --no-show-signature --format=%ct "$COMMIT")"
# Member order, timestamps (the commit time), owners and modes are fixed so
# the archive has the same bytes on every host; ustar stores no atime or ctime.
TEMPORARY_ARCHIVE="$SOURCE_DIR/source.tar"
LC_ALL=C tar --create --file="$TEMPORARY_ARCHIVE" --directory="$SOURCE_DIR" \
	--format=ustar --sort=name --hard-dereference --mtime="@$SOURCE_EPOCH" \
	--owner=0 --group=0 --numeric-owner --mode='a=rX,u+w,ug-s' "$SOURCE_NAME"
check_sha256 "$TEMPORARY_ARCHIVE" "$SOURCE_ARCHIVE_SHA256" source
mv "$TEMPORARY_ARCHIVE" "$SOURCE_OUT"
echo "wrote $SOURCE_OUT (source of both images, GPL-3.0-or-later)"
