#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build the pinned release of the open AT BIOS (unemu-at-bios, the unemu fork
# of Sergey Kiselev's 8088_bios) with Nix, check its SHA-256 pin and copy the
# ROM here (see README.md). The ROM is never committed. The script also writes
# its corresponding source, the pinned tree Nix built from plus a NOTICE, as a
# deterministic tar archive, and checks that the archived tree builds the same
# ROM.
#
# Requires the host's Nix with flakes, git and GNU tar (both also in the flake
# devShell).
# The default source is the fork's public repository,
# https://github.com/unemu-org/at-bios (a GitHub fork of skiselev/8088_bios),
# fetched anonymously over HTTPS.
#
# Environment overrides, in order of precedence:
#   UNEMU_AT_BIOS_FLAKE     complete flake reference to build, without the
#                           #output suffix (a mirror, or a path to a work tree)
#   UNEMU_AT_BIOS_CHECKOUT  path to a local clone; builds the pinned revision
#                           from its object store, never its working tree
# Option: --print-flake prints the reference it would build, then exits.
set -euo pipefail

if ! command -v nix >/dev/null 2>&1; then
	echo "missing nix: install Nix with flakes; this script builds the AT BIOS with nix build" >&2
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
		elif [ "${3:-output}" = rebuild ]; then
			echo "The archived source no longer builds the shipped ROM; check the left-out paths before writing the archive." >&2
		else
			echo "Check the flake reference and the pinned release; deliberately rebuild the goldens' expectations before updating the expected output hash." >&2
		fi
		exit 1
	fi
}

# Pinned release. The tag names it; the revision is what Nix checks out, so a
# moved tag cannot change the sources. The output hashes are the final guard.
# The fork's flake assembles src/bios.asm with nasm from its nixpkgs pin (the
# one this repository's devShell uses), with the MACHINE_UNEMU_AT
# configuration, and checks the same ROM hash itself.
TAG="v1.0.2-unemu.3"
REVISION="4548c1769aed67323979d5fa8eff52eed266b64b"
ROM_SHA256="6938dc8b665fa120f33775163ab6bacc559389525db87adedf734416f9ef2442"
SOURCE_ARCHIVE_SHA256="b928cee48fb7be8bf3da0808ed31a2ad283ae4d57e73241e59a61b1ddde932ee"
QUERY="ref=refs/tags/$TAG&rev=$REVISION"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROM_OUT="$HERE/bios-unemu-at.bin"
SOURCE_NAME="unemu-at-bios-${TAG#v}-src"
SOURCE_OUT="$HERE/$SOURCE_NAME.tar"

# The fork's public repository; UNEMU_AT_BIOS_FLAKE names a mirror instead.
REPOSITORY="https://github.com/unemu-org/at-bios"

if [ -n "${UNEMU_AT_BIOS_FLAKE:-}" ]; then
	FLAKE="$UNEMU_AT_BIOS_FLAKE"
	SOURCE="UNEMU_AT_BIOS_FLAKE"
elif [ -n "${UNEMU_AT_BIOS_CHECKOUT:-}" ]; then
	CHECKOUT="$(cd "$UNEMU_AT_BIOS_CHECKOUT" && pwd)"
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
if [ "$SOURCE" = UNEMU_AT_BIOS_FLAKE ]; then
	echo "building unemu-at-bios from UNEMU_AT_BIOS_FLAKE; the output pins still apply"
else
	echo "building unemu-at-bios $TAG ($REVISION) from the $SOURCE"
fi
# An unreachable or renamed repository must fail, not wait for a password
# prompt; credential helpers still work.
export GIT_TERMINAL_PROMPT="${GIT_TERMINAL_PROMPT:-0}"
# --no-link keeps result symlinks out of both repositories.
ROM_STORE="$("${NIX[@]}" build --no-link --print-out-paths "$FLAKE#unemu-at")" || {
	echo "cannot build unemu-at-bios #unemu-at from the $SOURCE." >&2
	if [ "$SOURCE" = "public repository" ]; then
		echo "The default source is $REPOSITORY, read anonymously over HTTPS; check the network and that it still carries tag $TAG at revision $REVISION." >&2
	fi
	echo "Set UNEMU_AT_BIOS_FLAKE to another flake reference (a mirror that carries the pinned revision, or a work tree), or UNEMU_AT_BIOS_CHECKOUT to a local clone; see vendor/unemu-at-bios/README.md." >&2
	exit 1
}

mkdir -p "$HERE/build"
WORK="$(mktemp -d "$HERE/build/fetch.XXXXXX")"
# The source copy below comes from the read-only store; make it removable.
trap 'chmod -R u+w "$WORK" 2>/dev/null || true; rm -rf "$WORK"' EXIT

# The raw 32 KiB image maps at F000:8000 (physical F8000h-FFFFFh); the CPU
# reset entry F000:FFF0 is offset 7FF0h, a far jump (EAh) into POST.
cp "$ROM_STORE/bios-unemu-at.bin" "$WORK/bios-unemu-at.bin"
SIZE="$(wc -c < "$WORK/bios-unemu-at.bin" | tr -d '[:space:]')"
RESET="$(od -An -tx1 -j 32752 -N1 "$WORK/bios-unemu-at.bin" | tr -d ' \n')"
if [ "$SIZE" -ne 32768 ] || [ "$RESET" != "ea" ]; then
	echo "invalid system BIOS: expected 32768 bytes and a far jump at offset 7FF0h" >&2
	exit 1
fi
check_sha256 "$WORK/bios-unemu-at.bin" "$ROM_SHA256"
# Store copies are read-only; the fixture stays replaceable by the next run.
chmod 0644 "$WORK/bios-unemu-at.bin"
mv "$WORK/bios-unemu-at.bin" "$ROM_OUT"
echo "built $ROM_OUT ($SIZE bytes)"

# The archive holds the tree Nix built the ROM from. Its commit time
# (lastModified) becomes every member's timestamp.
SOURCE_INFO="$(UNEMU_AT_BIOS_FLAKE_REF="$FLAKE" "${NIX[@]}" eval --raw --impure --expr \
	'let flake = builtins.getFlake (builtins.getEnv "UNEMU_AT_BIOS_FLAKE_REF"); in "${toString flake.lastModified} ${flake.outPath}"')"
SOURCE_EPOCH="${SOURCE_INFO%% *}"
SOURCE_TREE="${SOURCE_INFO#* }"
STAGE="$WORK/source/$SOURCE_NAME"
mkdir -p "$STAGE"
cp -R "$SOURCE_TREE" "$STAGE/unemu-at-bios"
chmod -R u+w "$STAGE"
# Upstream's prebuilt images are build outputs, not source, and the ROM does
# not contain them: binaries/ holds other boards' BIOS images, xtide_r625/
# XTIDE Universal BIOS r625 binaries whose source the archive does not carry.
rm -rf "$STAGE/unemu-at-bios/binaries" "$STAGE/unemu-at-bios/xtide_r625"
cp "$HERE/source-notice.txt" "$STAGE/NOTICE"
# The archived tree, a flake of its own, must still build the shipped ROM.
# Nix copies it to the store as it is; the fork's flake.lock pins nasm.
REBUILT="$("${NIX[@]}" build --no-link --print-out-paths --no-write-lock-file \
	"path:$STAGE/unemu-at-bios#unemu-at")" || {
	echo "the archived unemu-at-bios tree does not build #unemu-at" >&2
	exit 1
}
check_sha256 "$REBUILT/bios-unemu-at.bin" "$ROM_SHA256" rebuild
# Member order, timestamps, owners and modes are fixed so the archive has the
# same bytes on every host; ustar stores no atime or ctime.
TEMPORARY_ARCHIVE="$WORK/source.tar"
LC_ALL=C tar --create --file="$TEMPORARY_ARCHIVE" --directory="$WORK/source" \
	--format=ustar --sort=name --hard-dereference --mtime="@$SOURCE_EPOCH" \
	--owner=0 --group=0 --numeric-owner --mode='a=rX,u+w,ug-s' "$SOURCE_NAME"
check_sha256 "$TEMPORARY_ARCHIVE" "$SOURCE_ARCHIVE_SHA256" source
mv "$TEMPORARY_ARCHIVE" "$SOURCE_OUT"
echo "wrote $SOURCE_OUT (source of the ROM, GPL-3.0-or-later)"
