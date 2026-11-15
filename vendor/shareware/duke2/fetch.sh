#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build a 1.44M installation floppy from the unmodified shareware release of
# Duke Nukem II, episode 1, version 1.0 (Apogee). The archive and the image
# are third-party shareware: fetched or supplied locally, never committed.
# See README.md for the source, every pinned hash and the distribution terms.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ARCHIVE="4duke.zip"
ARCHIVE_SHA256="584c43f625e4998b623914aeb4968ff6cc3a0f114938bc4cf43f5ffb2705dd08"
# Apogee's file area as mirrored by FUNET since 1995; the fallback is the
# byte-identical copy on a 1994 shareware CD-ROM indexed by DiscMaster.
ARCHIVE_URLS=(
	"https://ftp.funet.fi/pub/msdos/games/apogee/4duke.zip"
	"https://discmaster.textfiles.com/file/20928/400%20MB%20Of%20Top%20PC%20Shareware.iso/games/apogee/dukene2/4duke.zip"
)
FILES=(
	"install.bat 5834d9d3dae6cda7056919ad5dd0a40fd790d9484832c366a4b955ac7c9cded1"
	"deice.exe 4f77c1fee9c6e579ee325e83b0fc9d79254bb4837be950d06059260395fb4569"
	"dn2.dat 101f8007e4459b75cbf7b3f318eb1b2916d656f60a6ccaedb4c68197e2ec3e39"
	"dn2.1 fa8170193314f1ab7cfa47f40703624414f9126495336b56c6c5a4012b94219c"
	"file_id.diz 6d9fbdbe6108984c480a01cd9337586b0f360497206eb28b6bdca646f0a139cd"
)
EXTRAS=(
	"dn2.1 LICENSE.DOC 2ebda15b03e566b5589081f33f79023fab99287cd711baacba6e51289c7537af"
	"dn2.1 ORDER.FRM f494e98017d6e59dc3dd7c0bd7e5c4d1f95fd552f29304871136b2019c24e612"
)
VOLUME_LABEL="DUKE2SW10"
VOLUME_SERIAL="0x19931201"
IMAGE="duke2-sw10-1440.img"
IMAGE_SHA256="00250a06e6797f8d9ffcd8dd41af82e00aa53043ad7ffd8e2c5ac90bd48fd554"

# shellcheck source-path=SCRIPTDIR source=../common.sh
. "$HERE/../common.sh"
