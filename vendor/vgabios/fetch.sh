#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build the pinned release of the open VGA BIOS (the unemu fork of the LGPL
# VGABIOS) and its INT 10h probe floppy with Nix, check both SHA-256 pins and
# copy the two files here (see README.md). Neither file is ever committed.
# It also writes their corresponding source, the pinned tree Nix built from
# plus a NOTICE, as a deterministic tar archive.
#
# Requires the host's Nix with flakes, git and GNU tar (both also in the flake
# devShell).
# The default source is the fork's public repository,
# https://github.com/unemu-org/vgabios (a GitHub fork of bochs-emu/VGABIOS),
# fetched anonymously over HTTPS.
#
# Environment overrides, in order of precedence:
#   UNEMU_VGABIOS_FLAKE     complete flake reference to build, without the
#                           #output suffix (a mirror, or a path to a work tree)
#   UNEMU_VGABIOS_CHECKOUT  path to a local clone; builds the pinned revision
#                           from its object store, never its working tree
# Option: --print-flake prints the reference it would build, then exits.
set -euo pipefail

if ! command -v nix >/dev/null 2>&1; then
	echo "missing nix: install Nix with flakes; this script builds the VGA BIOS with nix build" >&2
	exit 1
fi
if ! command -v git >/dev/null 2>&1; then
	echo "missing git: run this script inside nix develop, or install git" >&2
	exit 1
fi
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
			echo "Check the flake reference, the pinned release, source-notice.txt and the GNU tar version before updating the source archive hash." >&2
		else
			echo "Check the flake reference and the pinned release; deliberately rebuild the goldens' expectations before updating the expected output hash." >&2
		fi
		exit 1
	fi
}

# Pinned release. The tag names it; the revision is what Nix checks out, so a
# moved tag cannot change the sources. The output hashes are the final guard.
# The flake preprocesses with unwrapped gcc 14.3.0 from its nixpkgs pin on
# every system, without platform macros or system headers, then runs dev86
# 1.0.1 bcc/as86; it also fixes the intermediate file names, which bcc's
# output depends on. The release is based on upstream 0.9d and adds the
# teletype BEL tone, so both outputs differ from the 0.8a-based releases
# before it. The fork's CI checks both hashes on x86_64-linux with nix flake
# check.
TAG="v0.9d-unemu.1"
REVISION="40522278d5b703e637271696783721f4b5ea541f"
ROM_SHA256="b867f3846f44be63b1b10febc1ce79c9435f5d1233c67a0b63164ab48359a582"
PROBE_SHA256="7c928cec8e49c04c91d1cb7194b5f508db3d86ac89be9867a7290eb04c11f1b4"
SOURCE_ARCHIVE_SHA256="0f0890b6fc95156177c37725329ca8111b889abce33de73aeeaf2fc99ce86c48"
QUERY="ref=refs/tags/$TAG&rev=$REVISION"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROM_OUT="$HERE/vgabios-std.bin"
PROBE_OUT="$HERE/int10-probe-1200.img"
SOURCE_NAME="vgabios-${TAG#v}-src"
SOURCE_OUT="$HERE/$SOURCE_NAME.tar"

# The fork's public repository; UNEMU_VGABIOS_FLAKE names a mirror instead.
REPOSITORY="https://github.com/unemu-org/vgabios"

if [ -n "${UNEMU_VGABIOS_FLAKE:-}" ]; then
	FLAKE="$UNEMU_VGABIOS_FLAKE"
	SOURCE="UNEMU_VGABIOS_FLAKE"
elif [ -n "${UNEMU_VGABIOS_CHECKOUT:-}" ]; then
	CHECKOUT="$(cd "$UNEMU_VGABIOS_CHECKOUT" && pwd)"
	FLAKE="git+file://$CHECKOUT?$QUERY"
	SOURCE="local checkout"
else
	FLAKE="git+$REPOSITORY?$QUERY"
	SOURCE="public repository"
fi

if [ "${1:-}" = "--print-flake" ]; then
	echo "$FLAKE"
	exit 0
fi

NIX=(nix --extra-experimental-features "nix-command flakes")
if [ "$SOURCE" = UNEMU_VGABIOS_FLAKE ]; then
	echo "building vgabios from UNEMU_VGABIOS_FLAKE; the output pins still apply"
else
	echo "building vgabios $TAG ($REVISION) from the $SOURCE"
fi
# An unreachable or renamed repository must fail, not wait for a password
# prompt; credential helpers still work.
export GIT_TERMINAL_PROMPT="${GIT_TERMINAL_PROMPT:-0}"
build() {
	# --no-link keeps result symlinks out of both repositories.
	"${NIX[@]}" build --no-link --print-out-paths "$FLAKE#$1" || {
		echo "cannot build vgabios #$1 from the $SOURCE." >&2
		if [ "$SOURCE" = "public repository" ]; then
			echo "The default source is $REPOSITORY, read anonymously over HTTPS; check the network and that it still carries tag $TAG at revision $REVISION." >&2
		fi
		echo "Set UNEMU_VGABIOS_FLAKE to another flake reference (a mirror that carries the pinned revision, or a work tree), or UNEMU_VGABIOS_CHECKOUT to a local clone; see vendor/vgabios/README.md." >&2
		exit 1
	}
}
ROM_STORE="$(build std)"
PROBE_STORE="$(build int10-probe)"

mkdir -p "$HERE/build"
WORK="$(mktemp -d "$HERE/build/fetch.XXXXXX")"
# The source copy below comes from the read-only store; make it removable.
trap 'chmod -R u+w "$WORK" 2>/dev/null || true; rm -rf "$WORK"' EXIT

byte_sum() {
	od -An -v -tu1 "$1" | awk '{for (i = 1; i <= NF; i++) sum += $i} END {print sum % 256}'
}

cp "$ROM_STORE/vgabios-std.bin" "$WORK/vgabios-std.bin"
SIZE="$(wc -c < "$WORK/vgabios-std.bin" | tr -d '[:space:]')"
HEADER="$(od -An -tx1 -N3 "$WORK/vgabios-std.bin" | tr -d ' \n')"
# IBM PC AT Technical Reference, "Adapters with System-Accessible ROM Modules":
# 55AAh, a length byte in 512-byte units (40h: 32 KiB) and a zero byte sum.
if [ "$SIZE" -ne 32768 ] || [ "$HEADER" != "55aa40" ] || [ "$(byte_sum "$WORK/vgabios-std.bin")" -ne 0 ]; then
	echo "invalid video ROM: expected 32768 bytes, header 55aa40 and byte sum 0" >&2
	exit 1
fi
check_sha256 "$WORK/vgabios-std.bin" "$ROM_SHA256"

cp "$PROBE_STORE/int10-probe-1200.img" "$WORK/int10-probe-1200.img"
PROBE_SIZE="$(wc -c < "$WORK/int10-probe-1200.img" | tr -d '[:space:]')"
if [ "$PROBE_SIZE" -ne 1228800 ]; then
	echo "unexpected probe floppy size: $PROBE_SIZE bytes (expected 1228800)" >&2
	exit 1
fi
check_sha256 "$WORK/int10-probe-1200.img" "$PROBE_SHA256"

# Store copies are read-only; the fixtures stay replaceable by the next run.
chmod 0644 "$WORK/vgabios-std.bin" "$WORK/int10-probe-1200.img"
mv "$WORK/vgabios-std.bin" "$ROM_OUT"
mv "$WORK/int10-probe-1200.img" "$PROBE_OUT"
echo "built $ROM_OUT ($SIZE bytes, header 55aa40, byte sum 0)"
echo "built $PROBE_OUT ($PROBE_SIZE bytes)"

# The archive holds the tree Nix built both files from. Its commit time
# (lastModified) becomes every member's timestamp.
SOURCE_INFO="$(VGABIOS_FLAKE_REF="$FLAKE" "${NIX[@]}" eval --raw --impure --expr \
	'let flake = builtins.getFlake (builtins.getEnv "VGABIOS_FLAKE_REF"); in "${toString flake.lastModified} ${flake.outPath}"')"
SOURCE_EPOCH="${SOURCE_INFO%% *}"
SOURCE_TREE="${SOURCE_INFO#* }"
STAGE="$WORK/source/$SOURCE_NAME"
mkdir -p "$STAGE"
cp -R "$SOURCE_TREE" "$STAGE/vgabios"
chmod -R u+w "$STAGE"
# The issue tracker's settings build nothing and name a developer's own sync
# remote, an address the published archive must not carry. The fork stopped
# tracking them before v0.8a-unemu.3; the removal stays as a guard.
rm -f "$STAGE/vgabios/.beads/config.yaml"
cp "$HERE/source-notice.txt" "$STAGE/NOTICE"
# Member order, timestamps, owners and modes are fixed so the archive has the
# same bytes on every host; ustar stores no atime or ctime.
TEMPORARY_ARCHIVE="$WORK/source.tar"
LC_ALL=C tar --create --file="$TEMPORARY_ARCHIVE" --directory="$WORK/source" \
	--format=ustar --sort=name --hard-dereference --mtime="@$SOURCE_EPOCH" \
	--owner=0 --group=0 --numeric-owner --mode='a=rX,u+w,ug-s' "$SOURCE_NAME"
check_sha256 "$TEMPORARY_ARCHIVE" "$SOURCE_ARCHIVE_SHA256" source
mv "$TEMPORARY_ARCHIVE" "$SOURCE_OUT"
echo "wrote $SOURCE_OUT (source of both files, LGPL-2.1-or-later)"
