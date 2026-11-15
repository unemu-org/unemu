#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build a 1.44M installation floppy from the unmodified shareware release of
# Commander Keen 4, "Secret of the Oracle", version 1.4 (id Software,
# distributed by Apogee). The archive and the image are third-party shareware:
# fetched or supplied locally, never committed. See README.md for the source,
# every pinned hash and the distribution terms.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ARCHIVE="4keen.zip"
ARCHIVE_SHA256="b024f2fb48fbf360c6fac2932acd72e4585595cae41a6223c2607bc42c111421"
# Apogee's file area as mirrored by FUNET since 1995; the fallback is the
# byte-identical copy on a 1994 shareware CD-ROM indexed by DiscMaster.
ARCHIVE_URLS=(
	"https://ftp.funet.fi/pub/msdos/games/apogee/4keen.zip"
	"https://discmaster.textfiles.com/file/20928/400%20MB%20Of%20Top%20PC%20Shareware.iso/games/apogee/4keen.zip"
)
FILES=(
	"install.bat b79322a82348ad1bd98e135c4402de8f04021031a727e2ae4d3849b4a93b833f"
	"deice.exe 4f77c1fee9c6e579ee325e83b0fc9d79254bb4837be950d06059260395fb4569"
	"galaxy.dat 0815e45e2fd8068501725b1b2e6accde6c60e09897a371fdc946c4d7e85f4cd4"
	"galaxy.1 d6784de99672f5ab16d0d91e78b7ead91bdce7511092e091605acceb6adf72fe"
	"file_id.diz de1d9d2e597fdb5f03d8bc0aa41d46c6436cb699ac82262940d5f45dd756a337"
)
# This release has no VENDOR.DOC; README.DOC carries its only shareware terms.
EXTRAS=(
	"galaxy.1 README.DOC 58ed579502a7c8e802381225e93ba0b2f5860908c7f4d234b5897385afb447e3"
	"galaxy.1 ORDER.FRM 1ff74f2750659268e138d0ea0e17d9fc43274ca5872a4dbdb2ad62988b9aaa76"
)
VOLUME_LABEL="KEEN4SW14"
VOLUME_SERIAL="0x19920201"
IMAGE="keen4-sw14-1440.img"
IMAGE_SHA256="c34c25fae130a4b432ae6ae1fcaea1eb8f4763c3e184785a2f551d14c47a4578"

# shellcheck source-path=SCRIPTDIR source=../common.sh
. "$HERE/../common.sh"
