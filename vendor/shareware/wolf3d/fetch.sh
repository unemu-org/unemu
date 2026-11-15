#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build a 1.44M installation floppy from the unmodified shareware release of
# Wolfenstein 3-D, episode 1, version 1.4 (id Software, published by Apogee).
# The archive and the image are third-party shareware: fetched or supplied
# locally, never committed. See README.md for the source, every pinned hash
# and the distribution terms.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ARCHIVE="1wolf14.zip"
ARCHIVE_SHA256="c3d355ed9f887a94239140070b15d4042ec68a4a456171c2853edc04727b1171"
# Apogee's file area as mirrored by FUNET since 1995; the fallback is the
# byte-identical copy on a 1994 shareware CD-ROM indexed by DiscMaster.
ARCHIVE_URLS=(
	"https://ftp.funet.fi/pub/msdos/games/apogee/1wolf14.zip"
	"https://discmaster.textfiles.com/file/20928/400%20MB%20Of%20Top%20PC%20Shareware.iso/games/apogee/wolf3d/1wolf14.zip"
)
FILES=(
	"INSTALL.BAT c7184ead2efaa119849128a67ad3a1ad12c0a0d53135402e6db4e2ff409c4e8c"
	"DEICE.EXE 4f77c1fee9c6e579ee325e83b0fc9d79254bb4837be950d06059260395fb4569"
	"WOLF.DAT 7b1488120dbfa9a2c2899626bfc0f16432ec1827e73afa473e4eb25501c4b2bc"
	"WOLF.1 ce042aeb2a6b32f7651b318d78203376baf72151e6f95d3cc3da212f693959e7"
	"VENDOR.DOC a2ab674de6d2e5ed75d5e67f592d1cc0357b201979457c4b144e78db91df3b30"
	"FILE_ID.DIZ 840242ab486718872c865f4feed86b7bd874bf0d7e339977956e347dac97dfca"
)
EXTRAS=(
	"- VENDOR.DOC a2ab674de6d2e5ed75d5e67f592d1cc0357b201979457c4b144e78db91df3b30"
	"WOLF.1 ORDER.FRM 1ff74f2750659268e138d0ea0e17d9fc43274ca5872a4dbdb2ad62988b9aaa76"
)
VOLUME_LABEL="WOLF3DSW14"
VOLUME_SERIAL="0x19930101"
IMAGE="wolf3d-sw14-1440.img"
IMAGE_SHA256="3d0ae4d9b0916b43dba1d02f427a2e2f9469bc55abbd5d105fa33369d44e8c70"

# shellcheck source-path=SCRIPTDIR source=../common.sh
. "$HERE/../common.sh"
