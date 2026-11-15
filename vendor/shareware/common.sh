# SPDX-License-Identifier: AGPL-3.0-or-later
# shellcheck shell=bash
#
# Shared steps of the shareware installation-floppy scripts. Each title's
# fetch.sh sets the pins below and sources this file; it is not run directly.
#
# Nothing here is committed or redistributed by the repository. The script
# fetches the publisher's original distribution archive (or reads a copy the
# user supplies), checks its pinned SHA-256, and copies every file of the
# archive, byte for byte and in archive order, onto a fresh 1.44M FAT12
# floppy whose SHA-256 is pinned as well. The vendor and licence texts are
# also written as separate files next to the image, so a media set can carry
# them as extras.
#
# Pins set by fetch.sh before sourcing:
#   HERE            the title directory
#   ARCHIVE         file name of the distribution archive
#   ARCHIVE_SHA256  its SHA-256
#   ARCHIVE_URLS    array of download URLs, tried in order
#   FILES           array of "MEMBER SHA256", the archive's complete member
#                   list in archive order; each goes to the floppy root under
#                   its upper-case DOS name
#   EXTRAS          array of "SOURCE MEMBER SHA256"; SOURCE is "-" for the
#                   archive itself, or the FILES member that is the release's
#                   self-extracting zip payload holding MEMBER
#   VOLUME_LABEL, VOLUME_SERIAL, IMAGE, IMAGE_SHA256
#
# Environment: SHAREWARE_ARCHIVE=/path/to/archive.zip uses that copy and never
# touches the network. Otherwise a verified download is cached in build/ and
# later runs are offline.
#
# Requires Bash, mtools (mformat, mcopy, mdir), unzip, curl (only to
# download), awk, dd, locale, printf, tr, wc, mktemp, sha256sum or shasum,
# and the C.UTF-8 locale. The project devShell provides mtools 4.0.49,
# Info-ZIP UnZip 6.00 and curl with pinned CA roots. The pinned image hashes
# were measured on macOS (arm64) with the host's UnZip 6.00 and reproduce with
# the devShell's; a Linux build has not been measured.

if command -v sha256sum >/dev/null 2>&1; then
	SHA256=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
	SHA256=(shasum -a 256)
else
	echo "missing SHA-256 tool: install sha256sum or shasum (or use nix develop)" >&2
	exit 1
fi

require_tools() {
	local tool hint
	for tool in "$@"; do
		if ! command -v "$tool" >/dev/null 2>&1; then
			case "$tool" in
				mformat | mcopy | mdir) hint="nix develop provides mtools" ;;
				unzip | curl) hint="nix develop provides it" ;;
				*) hint="install it with the host's package manager" ;;
			esac
			echo "missing $tool on PATH: $hint" >&2
			exit 1
		fi
	done
}

sha256_of() {
	"${SHA256[@]}" "$1" | awk '{print $1}'
}

# $3 says what the file is, so the hint names the right remedy.
check_sha256() {
	local file="$1" expected="$2" role="$3" actual
	actual="$(sha256_of "$file")"
	if [ "$actual" = "$expected" ]; then
		return 0
	fi
	echo "SHA-256 mismatch for $file: expected $expected, got $actual" >&2
	case "$role" in
		cached) echo "Remove the cached archive and run again; do not change its expected hash." >&2 ;;
		supplied) echo "SHAREWARE_ARCHIVE must be the original $ARCHIVE named in README.md." >&2 ;;
		member) echo "The archive is not the pinned release; do not change the expected hash." >&2 ;;
		image) echo "Check the toolchain (mtools 4.0.49 and Info-ZIP UnZip 6.00 from nix develop); review the image deliberately before updating its expected hash." >&2 ;;
	esac
	exit 1
}

upper() {
	printf '%s' "$1" | LC_ALL=C tr '[:lower:]' '[:upper:]'
}

# mtools reads ~/.mtoolsrc through HOME even when MTOOLSRC is set (that only
# adds a further file), so mtools runs with an empty home directory. A
# system-wide mtools.conf, if the host has one, is still read.
run_mtools() {
	HOME="$WORK/home" "$@"
}

require_tools awk locale mformat mcopy mdir unzip dd tr wc mktemp

# Keep the caller's environment out of the bytes. The locale is fixed to
# C.UTF-8: under the plain C locale mtools 4.0.49 adds a VFAT long-name entry
# to every file and to the volume label, while any UTF-8 locale gives the
# plain 8.3 entries of the pinned images; upper() maps case byte-wise in C.
# UNZIP, UNZIPOPT, ZIPINFO and ZIPINFOOPT would add default options that
# change names, bytes or timestamps. MTOOLS_SKIP_CHECK and the cleared
# MTOOLS_* overrides keep mtools' layout fixed. The volume label gets the FAT
# epoch from SOURCE_DATE_EPOCH; every file entry, including its creation and
# access dates, gets the archive's DOS date and time, which TZ=UTC makes
# unzip and mcopy convert identically on every host.
export LC_ALL=C.UTF-8
if [ "$(locale charmap 2>/dev/null)" != "UTF-8" ]; then
	echo "the C.UTF-8 locale is not available on this host; mtools would write long-name entries" >&2
	exit 1
fi
unset UNZIP UNZIPOPT ZIPINFO ZIPINFOOPT
export MTOOLS_SKIP_CHECK=1
export SOURCE_DATE_EPOCH=315532800 TZ=UTC
unset MTOOLSRC MTOOLS_DATE_STRING MTOOLS_NFATS MTOOLS_DIR_LEN

mkdir -p "$HERE/build"
WORK="$(mktemp -d "$HERE/build/work.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
mkdir "$WORK/home"
CACHE="$HERE/build/$ARCHIVE"

if [ -n "${SHAREWARE_ARCHIVE:-}" ]; then
	if [ ! -f "$SHAREWARE_ARCHIVE" ]; then
		echo "SHAREWARE_ARCHIVE=$SHAREWARE_ARCHIVE is not a file" >&2
		exit 1
	fi
	check_sha256 "$SHAREWARE_ARCHIVE" "$ARCHIVE_SHA256" supplied
	SOURCE="$SHAREWARE_ARCHIVE"
elif [ -f "$CACHE" ]; then
	check_sha256 "$CACHE" "$ARCHIVE_SHA256" cached
	SOURCE="$CACHE"
else
	require_tools curl
	for url in "${ARCHIVE_URLS[@]}"; do
		rm -f "$WORK/download"
		# -q first, so no ~/.curlrc applies. The limits let a stalled or
		# oversized answer give way to the next source; every archive is
		# well under 2 MiB.
		if ! curl -q --fail --silent --show-error --location --retry 2 \
			--connect-timeout 20 --max-time 300 --max-filesize 2097152 \
			-o "$WORK/download" "$url"; then
			echo "download failed: $url" >&2
			continue
		fi
		got="$(sha256_of "$WORK/download")"
		if [ "$got" != "$ARCHIVE_SHA256" ]; then
			echo "wrong file from $url: $(wc -c < "$WORK/download" | tr -d '[:space:]') bytes, SHA-256 $got; trying the next source" >&2
			continue
		fi
		# Only a verified archive enters the cache.
		mv -f "$WORK/download" "$CACHE"
		break
	done
	if [ ! -f "$CACHE" ]; then
		echo "could not fetch $ARCHIVE with SHA-256 $ARCHIVE_SHA256 from:" >&2
		printf '  %s\n' "${ARCHIVE_URLS[@]}" >&2
		echo "Download it by hand and run again with SHAREWARE_ARCHIVE=/path/to/$ARCHIVE." >&2
		exit 1
	fi
	SOURCE="$CACHE"
fi

# A repacked archive with an added or missing file is not the release, even if
# the files that are present match.
expected_members="$(for line in "${FILES[@]}"; do printf '%s\n' "${line%% *}"; done)"
actual_members="$(unzip -Z1 "$SOURCE")"
if [ "$actual_members" != "$expected_members" ]; then
	echo "unexpected member list in $SOURCE:" >&2
	printf '%s\n' "$actual_members" >&2
	exit 1
fi

mkdir -p "$WORK/files" "$WORK/extras"
for line in "${FILES[@]}"; do
	member="${line%% *}"
	unzip -q -o "$SOURCE" "$member" -d "$WORK/files"
	check_sha256 "$WORK/files/$member" "${line##* }" member
done
for line in "${EXTRAS[@]}"; do
	read -r from member expected <<< "$line"
	if [ "$from" = "-" ]; then
		unzip -q -o "$SOURCE" "$member" -d "$WORK/extras"
	else
		# The payload is a self-extracting zip; unzip reads past its program
		# stub without running anything.
		unzip -q -o "$WORK/files/$from" "$member" -d "$WORK/extras"
	fi
	check_sha256 "$WORK/extras/$member" "$expected" member
done

# Microsoft FAT32 File System Specification, Boot Sector and BPB, BS_jmpBoot:
# a valid jump lets DOS recognize the BPB. The project's own volume sector
# keeps mtools' boot code out of the image. IBM Personal Computer AT Technical
# Reference, BIOS listing, INT 18h: no bootable disk; halt if it returns.
# Drive number 00h (the first floppy) and the extended-BPB signature 29h,
# which enables the serial and label.
dd if=/dev/zero of="$WORK/boot.bin" bs=512 count=1 2>/dev/null
printf '\353\074\220' | dd of="$WORK/boot.bin" bs=1 seek=0 conv=notrunc 2>/dev/null
printf 'UNEMUPC ' | dd of="$WORK/boot.bin" bs=1 seek=3 conv=notrunc 2>/dev/null
printf '\000\000\051' | dd of="$WORK/boot.bin" bs=1 seek=36 conv=notrunc 2>/dev/null
printf '\315\030\364\353\375' | dd of="$WORK/boot.bin" bs=1 seek=62 conv=notrunc 2>/dev/null
printf '\125\252' | dd of="$WORK/boot.bin" bs=1 seek=510 conv=notrunc 2>/dev/null

dd if=/dev/zero of="$WORK/$IMAGE" bs=512 count=2880 2>/dev/null
run_mtools mformat -i "$WORK/$IMAGE" -f 1440 -N "$VOLUME_SERIAL" -v "$VOLUME_LABEL" -B "$WORK/boot.bin" ::
# mcopy -m keeps each file's DOS date and time from the archive: they are part
# of the release as distributed (the publisher's time of day encodes the
# version), and unzip restored them on the extracted files.
for line in "${FILES[@]}"; do
	member="${line%% *}"
	run_mtools mcopy -m -i "$WORK/$IMAGE" "$WORK/files/$member" "::$(upper "$member")"
done

size="$(wc -c < "$WORK/$IMAGE" | tr -d '[:space:]')"
if [ "$size" -ne 1474560 ]; then
	echo "unexpected image size: $size bytes (expected 1474560)" >&2
	exit 1
fi
check_sha256 "$WORK/$IMAGE" "$IMAGE_SHA256" image

mv -f "$WORK/$IMAGE" "$HERE/$IMAGE"
rm -rf "$HERE/extras"
mv "$WORK/extras" "$HERE/extras"
echo "built $HERE/$IMAGE ($size bytes, SHA-256 $IMAGE_SHA256)"
run_mtools mdir -i "$HERE/$IMAGE" ::
echo "extras in $HERE/extras:"
for line in "${EXTRAS[@]}"; do
	read -r from member expected <<< "$line"
	echo "  $member  $expected"
done
