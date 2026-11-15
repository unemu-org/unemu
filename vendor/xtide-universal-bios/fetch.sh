#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Prepare the pinned XTIDE Universal BIOS AT and configured XT+ option ROMs.
# The upstream binary is shorter than its declared ROM size and has no valid
# checksum. Keep its bytes intact, pad with zeros, and checksum the last byte.
# Also write the corresponding source, the pinned Subversion revision of the
# project's trunk plus a NOTICE, as a deterministic tar archive, after checking
# that it assembles to both downloads.
# Requires curl and Subversion (only for a missing cache; without svn on PATH
# Nix supplies it from this repository's pinned nixpkgs), GNU tar, nasm, make
# and standard shell tools. The flake devShell provides all of them except
# Subversion, and the pinned CA roots that the downloads are verified against.
# Firmware bytes, cached downloads and the source export are never committed.
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
for tool in nasm make; do
	if ! command -v "$tool" >/dev/null 2>&1; then
		echo "missing $tool: run this script inside nix develop" >&2
		exit 1
	fi
done

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
		elif [ "${3:-output}" = source ]; then
			echo "Remove the cached source export $SOURCE_CACHE and export it again; then check source-notice.txt and the GNU tar version before updating the source archive hash." >&2
		elif [ "${3:-output}" = rebuild ]; then
			echo "The r$SOURCE_REVISION source no longer assembles to the pinned download; check the cached export $SOURCE_CACHE and the nasm and make versions." >&2
		else
			echo "Check the pinned sources and toolchain; deliberately rebuild the goldens' expectations before updating the expected output hash." >&2
		fi
		exit 1
	fi
}

URL="https://www.xtideuniversalbios.org/binaries/r638/ide_at.bin"
UPSTREAM_SHA256="6d5007bca3be0bffeb210969a299b67199368b4845f554b5a00715e2718dfea8"
OUTPUT_SHA256="6409de912eab620ea144b79e723bed478814047dc2766fc19fd6e2e3690662d0"
XTP_UPSTREAM_SHA256="338f70795a7c881a839ce25365678db0f2a356f2f255b7493a07f58f3ac65ec4"
XTP_OUTPUT_SHA256="54975501e6887c836b8d57e7aad78421e745a9b71e250f94d5d1f9631eab1513"
# The download centre builds every trunk commit; r638's trunk assembles to
# both downloads byte for byte (README.md), so its export is their source.
SOURCE_URL="https://www.xtideuniversalbios.org/svn/xtideuniversalbios/trunk"
SOURCE_REVISION="638"
# r638's commit time, 2026-06-09 09:36:36 UTC (svn info); an export has no
# revision metadata of its own. The download centre's builds embed that date
# through NASM's __DATE__.
SOURCE_EPOCH="1780997796"
SOURCE_DATE="2026-06-09"
SOURCE_ARCHIVE_SHA256="7bbd5de269f4e1d52ae30b52531fcda842709b9ae49166ac249f9b37187b91e8"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$HERE/build"
RAW="$WORK/ide_at.bin"
OUT="$HERE/ide_at-r638-8k.bin"
SOURCE_NAME="xtide-universal-bios-r$SOURCE_REVISION-src"
SOURCE_CACHE="$WORK/trunk-r$SOURCE_REVISION"
SOURCE_OUT="$HERE/$SOURCE_NAME.tar"
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

RAW="$WORK/ide_xtp.bin"
OUT="$HERE/ide_xtp-r638-ata1f0-8k.bin"
if [ ! -f "$RAW" ]; then
	curl -fsSL -o "$RAW" "https://www.xtideuniversalbios.org/binaries/r638/ide_xtp.bin"
fi
check_sha256 "$RAW" "$XTP_UPSTREAM_SHA256" archive
RAW_SIZE="$(wc -c < "$RAW" | tr -d '[:space:]')"
HEADER="$(od -An -tx1 -N3 "$RAW" | tr -d ' \n')"
if [ "$RAW_SIZE" -ne 6629 ] || [ "$HEADER" != "55aa10" ]; then
	echo "unexpected XT+ image: expected 6629 bytes and header 55aa10" >&2
	exit 1
fi
TMP="$(mktemp "$WORK/ide_xtp-r638-ata1f0-8k.XXXXXX")"
dd if=/dev/zero of="$TMP" bs=8192 count=1 2>/dev/null
dd if="$RAW" of="$TMP" conv=notrunc 2>/dev/null
# ROMVARS.bBootDrv selects A: because this profile's hard disk is not bootable.
printf '\000' | dd of="$TMP" bs=1 seek=73 count=1 conv=notrunc 2>/dev/null
# ROMVARS.ideVars0: command base 1F0h, control base 3F0h (register 6 is
# 3F6h), DEVICE_16BIT_ATA=0, IRQ=0. XT+ has no MODULE_IRQ, so keep polling.
printf '\360\001\360\003\000\000' \
	| dd of="$TMP" bs=1 seek=78 count=6 conv=notrunc 2>/dev/null
SUM="$(byte_sum "$TMP")"
CHECKSUM=$(( (256 - SUM) % 256 ))
printf '%b' "\\$(printf '%03o' "$CHECKSUM")" \
	| dd of="$TMP" bs=1 seek=8191 count=1 conv=notrunc 2>/dev/null
SIZE="$(wc -c < "$TMP" | tr -d '[:space:]')"
if [ "$SIZE" -ne 8192 ] || [ "$(byte_sum "$TMP")" -ne 0 ]; then
	echo "invalid XT+ option ROM: expected 8192 bytes with byte sum 0 mod 256" >&2
	exit 1
fi
check_sha256 "$TMP" "$XTP_OUTPUT_SHA256"
mv "$TMP" "$OUT"
echo "built $OUT ($SIZE bytes) from r638 ide_xtp.bin; ATA 1F0h, polling, boot A:, byte sum 0"

SOURCE_DIR="$(mktemp -d "$WORK/source.XXXXXX")"
trap 'rm -rf "$SOURCE_DIR"' EXIT
svn_run() {
	if command -v svn >/dev/null 2>&1; then
		svn "$@"
	elif command -v nix >/dev/null 2>&1; then
		nix --extra-experimental-features "nix-command flakes" shell --no-update-lock-file \
			--inputs-from "$HERE/../.." nixpkgs#subversion --command svn "$@"
	else
		echo "missing svn: install Subversion, or Nix to supply it" >&2
		exit 1
	fi
}
# A revision's export never changes, so a cached one keeps rebuilds offline;
# the archive pin catches a changed cache.
if [ ! -d "$SOURCE_CACHE" ]; then
	echo "exporting XTIDE Universal BIOS r$SOURCE_REVISION source from $SOURCE_URL (needs the network once; cached in $SOURCE_CACHE)"
	# r638 sets no eol-style, keyword or externals properties; the flags keep
	# a host's Subversion configuration from changing the exported bytes.
	svn_run export --quiet --non-interactive --ignore-externals --native-eol LF \
		-r "$SOURCE_REVISION" "$SOURCE_URL" "$SOURCE_DIR/trunk"
	mv -T "$SOURCE_DIR/trunk" "$SOURCE_CACHE"
fi
STAGE="$SOURCE_DIR/$SOURCE_NAME"
mkdir -p "$STAGE"
cp -R "$SOURCE_CACHE" "$STAGE/trunk"
cp "$HERE/source-notice.txt" "$STAGE/NOTICE"
# The archived source must still assemble to both downloads. Build in a copy
# so no build output reaches the archive.
cp -R "$STAGE/trunk" "$SOURCE_DIR/check"
make -C "$SOURCE_DIR/check/XTIDE_Universal_BIOS" at xtplus \
	AS="nasm -D__DATE__='\"$SOURCE_DATE\"'" > /dev/null
check_sha256 "$SOURCE_DIR/check/XTIDE_Universal_BIOS/Build/ide_at.bin" "$UPSTREAM_SHA256" rebuild
check_sha256 "$SOURCE_DIR/check/XTIDE_Universal_BIOS/Build/ide_xtp.bin" "$XTP_UPSTREAM_SHA256" rebuild
# Member order, timestamps, owners and modes are fixed so the archive has the
# same bytes on every host; ustar stores no atime or ctime.
TEMPORARY_ARCHIVE="$SOURCE_DIR/source.tar"
LC_ALL=C tar --create --file="$TEMPORARY_ARCHIVE" --directory="$SOURCE_DIR" \
	--format=ustar --sort=name --hard-dereference --mtime="@$SOURCE_EPOCH" \
	--owner=0 --group=0 --numeric-owner --mode='a=rX,u+w,ug-s' "$SOURCE_NAME"
check_sha256 "$TEMPORARY_ARCHIVE" "$SOURCE_ARCHIVE_SHA256" source
mv "$TEMPORARY_ARCHIVE" "$SOURCE_OUT"
echo "wrote $SOURCE_OUT (source of both r638 images, GPL-2.0-only; rebuilds both downloads)"
