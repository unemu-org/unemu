# vendor/shareware/duke2 — Duke Nukem II shareware installation floppy

`fetch.sh` builds `duke2-sw10-1440.img`, a 1.44M FAT12 installation floppy
holding the unmodified shareware release of **Duke Nukem II, episode 1,
version 1.0** (Apogee Software, Todd Replogle, 1993), and copies its licence
texts to `extras/`.

The archive, the image and the extras are third-party shareware. They are
never committed, and fetching does not add any redistribution right. The
maintainer approved publishing this release on 2026-11-15 (decision-26,
amendment of that date), so its media set is imported with the licence
statement `--open --license LicenseRef-apogee-license-v12.01.93
--license-text LICENSE.DOC` (Usage below): the set is open under Apogee's
`LICENSE.DOC` terms (revision `[V.12.01.93]`), that text travels in the set
as its terms text, and the push guard lets the set into `unemu/`. Imported
without the statement the set stays `user-supplied` in the user's private
store (decision-26 §8). The terms are summarised below, not interpreted; the
full texts are never in the repository, only in `extras/` and in the set.

## Usage

```sh
vendor/shareware/duke2/fetch.sh                                  # download once, then offline
SHAREWARE_ARCHIVE=/path/to/4duke.zip vendor/shareware/duke2/fetch.sh   # never uses the network
cargo run --offline -p cli -- image media import vendor/shareware/duke2/duke2-sw10-1440.img --title "Duke Nukem II: Episode 1 (shareware 1.0)" --tag duke2-shareware:1.0 --label '1=Disk 1 of 1 - Shareware v1.0 installation' --extra vendor/shareware/duke2/extras/LICENSE.DOC:LICENSE.DOC --extra vendor/shareware/duke2/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "Apogee Software" --year 1993 --genre platformer --platform DOS --open --license LicenseRef-apogee-license-v12.01.93 --license-text LICENSE.DOC --store /path/to/store --create
```

The import also records the set-level metadata: publisher and developer
Apogee Software (`LICENSE.DOC`: "Copyright 1993 Apogee Software, Todd
Replogle"; it asks commercial distributors to put "Published by Apogee" or
"An Apogee Game" in every description), release year 1993, genre platformer
and software platform DOS.

Run it inside `nix develop`, which provides mtools 4.0.49, the version that
produced the pinned image; unzip and curl (download only) come from the
host. It also needs Bash, awk, and `sha256sum` or `shasum`. The pinned hash
was measured on macOS (arm64) with Info-ZIP UnZip 6.00; a Linux build has
not been measured. The verified archive is cached in `build/`; a
cached or supplied archive with another hash is refused. The script checks
the archive's member list, every member's SHA-256, each extra's SHA-256 and
the image's SHA-256 before it replaces any output. Nothing from the archive
is ever run.

## Source

| | |
| --- | --- |
| Archive | `4duke.zip`, 1,117,465 bytes |
| SHA-256 | `584c43f625e4998b623914aeb4968ff6cc3a0f114938bc4cf43f5ffb2705dd08` |
| Primary | <https://ftp.funet.fi/pub/msdos/games/apogee/4duke.zip> |
| Fallback | <https://discmaster.textfiles.com/file/20928/400%20MB%20Of%20Top%20PC%20Shareware.iso/games/apogee/dukene2/4duke.zip> |

The primary is FUNET's mirror of Apogee's shareware file area; its index
(`00-index.txt`, listing dated 19 Nov 1995) describes the file as "Duke Nukem
II. Continuing adventures of Duke Nukem. Requires VGA, 286+." The fallback is
the copy on the 1994 CD-ROM *400 MB of Top PC Shareware*, indexed by
DiscMaster; both were downloaded on 2026-11-15 and are byte-identical.
`DN2.DAT` names the release: "Shareware Episode - BBS & Online Edition".

The later 3D Realms file area carries a repack with a newer installer
(`INSTALL.EXE`, `DN2SW10.SHR`; 1,146,465 bytes, SHA-256
`c94479ace93a8923615ddebea96bcd18242a3a31f54cdcbc84e02ca0ab685829`); it was
not chosen.

## Contents

The floppy holds the five archive members, in archive order, under upper-case
DOS names. `INSTALL.BAT` runs `DEICE.EXE`, which installs `DN2.1` (a
self-extracting zip) into `\DUKE2` on a chosen drive.

| Archive member | Bytes | SHA-256 |
| --- | ---: | --- |
| `install.bat` | 75 | `5834d9d3dae6cda7056919ad5dd0a40fd790d9484832c366a4b955ac7c9cded1` |
| `deice.exe` | 17,303 | `4f77c1fee9c6e579ee325e83b0fc9d79254bb4837be950d06059260395fb4569` |
| `dn2.dat` | 317 | `101f8007e4459b75cbf7b3f318eb1b2916d656f60a6ccaedb4c68197e2ec3e39` |
| `dn2.1` | 1,103,054 | `fa8170193314f1ab7cfa47f40703624414f9126495336b56c6c5a4012b94219c` |
| `file_id.diz` | 540 | `6d9fbdbe6108984c480a01cd9337586b0f360497206eb28b6bdca646f0a139cd` |

Files inside `DN2.1`, for reference (the installer writes them; the floppy
keeps the payload as distributed):

| Payload member | Bytes | SHA-256 |
| --- | ---: | --- |
| `NUKEM2.EXE` | 58,852 | `e31bfb7f1cf0cb9df0bd89bdeb3e9b3efd6541fe847f32366a403ed739596f1c` |
| `NUKEM2.CMP` | 3,075,327 | `3083e92df83476eab94b496f2561fbf3d1e057fddd127cd072ca0a10a8aa4431` |
| `NUKEM2.F1` | 98,960 | `2cb7839e8e7766e9fd44f58042d3de511889c9e6e31c87a8cffeb392f80c68db` |
| `NUKEM2.F2` | 38,622 | `2a2094ea73974923b711cdc70675443b7def731068a76210be8337e1308e1415` |
| `NUKEM2.F3` | 52,427 | `ed06be886a6a311c2fd3c890d2b3d6adacbf710b5c0e5d4d0b503038aad11343` |
| `NUKEM2.F4` | 240,848 | `d628e685d04d84839c86ec7f6802451c4efe62cf748918ecc4e5460df7396bc1` |
| `NUKEM2.F5` | 33,911 | `fdad9a21240db893a3ce098c96de2ce80ee389b7ebb7193b5587f0611ea7aec3` |
| `DN2-HELP.EXE` | 10,795 | `440e075acc395cd1ace2951a4208b4a67dff7c07b0438e202f0824247224723f` |
| `LICENSE.DOC` | 6,602 | `2ebda15b03e566b5589081f33f79023fab99287cd711baacba6e51289c7537af` |
| `CATALOG.EXE` | 28,859 | `32f1a07dbb49417caf0f3113ba84fe449cb4320b201fa40923d5c101a8dcfc83` |
| `ORDER.FRM` | 5,984 | `f494e98017d6e59dc3dd7c0bd7e5c4d1f95fd552f29304871136b2019c24e612` |

Outputs:

| Output | SHA-256 |
| --- | --- |
| `duke2-sw10-1440.img` (1,474,560 bytes) | `00250a06e6797f8d9ffcd8dd41af82e00aa53043ad7ffd8e2c5ac90bd48fd554` |
| `extras/LICENSE.DOC` | `2ebda15b03e566b5589081f33f79023fab99287cd711baacba6e51289c7537af` |
| `extras/ORDER.FRM` | `f494e98017d6e59dc3dd7c0bd7e5c4d1f95fd552f29304871136b2019c24e612` |

The image is a standard 1.44M FAT12 volume (512-byte sectors, one sector per
cluster, two 9-sector FATs, 224 root entries, media byte F0h), label
`DUKE2SW10`, serial `1993-1201`. Every file keeps the archive's DOS date and
time (1993-12-01 13:00; the publisher's time of day encodes the version); the
volume label uses 1980-01-01 00:00. The volume sector is the project's own
non-bootable stub, as in `vendor/hdd`: jump `EB 3C 90`, OEM text
`"UNEMUPC "`, drive 00h, extended-BPB signature 29h, and at 3Eh `INT 18h`
then `HLT` in a loop. Unused sectors are zero.

## Terms

`LICENSE.DOC` (revision `[V.12.01.93]`; inside `DN2.1` on the floppy and in
`extras/LICENSE.DOC`, SHA-256
`2ebda15b03e566b5589081f33f79023fab99287cd711baacba6e51289c7537af`) holds the
complete text. In summary:

- **Scope.** A licence to distribute or sell the shareware version only. It
  states the only terms on which Apogee permits *commercial* distribution, and
  does not apply to the registered episodes, which only authorised dealers
  may distribute with Apogee's written approval.
- **Definitions.** Commercial distribution is distribution for money or other
  consideration: tangible media, retail, mail order, catalogues, advertising
  or trade shows. It excludes distribution by not-for-profit organisations, by
  user groups to their members, and electronically by BBS. "Game software"
  covers the released shareware version and its related files, this document
  included.
- **A (ownership).** Apogee keeps the copyright, the distribution right and
  the trademarks, except as it licenses them.
- **B (conditions for commercial distributors).** [1] put "Published by
  Apogee" or "An Apogee Game" in every catalogue, packaging or promotional
  description; [2] market it as shareware; [3] sell only the current version;
  [4] copy only from Apogee's masters; [5] distribute the complete shareware
  version with all related files in the format as released, naming
  `CATALOG.EXE`, `ORDER.FRM` and `FILE_ID.DIZ`; [6] sign Apogee's retail
  distribution agreement before rack, CD-ROM or other retail sale; [7] do not
  distribute or sell third-party levels, editors or add-ons that change how
  Apogee's games play.
- **C (scope of the right).** The right is personal; it includes no right to
  "sublicense or otherwise cause any copying or distribution of the GAME
  SOFTWARE by anyone else without our consent in writing", to rent or lease
  it, or to sell it as part of a hardware or software package.
- **D, E, F.** No warranty; either side may end the licence on 30 days'
  written notice, with a sell-off period; equitable remedies, indemnity for
  the distributor's copying, and courts in Dallas, Texas.
- **G.** The document itself is Apogee's property; other shareware companies
  may not copy it.
- **Summary of key points.** [1] Apogee decides who may sell and distribute;
  [2] advertising names Apogee; [3] retail sale, bundling and CD-ROM sale need
  Apogee's written permission first; [4] selling by catalogue, advertisement,
  BBS, on-line service or direct mail needs no prior permission; [5] players
  may copy the shareware episode, and only it:

  > Customers and players of Apogee's shareware games may freely copy and
  > pass around our shareware software, without charging a fee.

`ORDER.FRM` is Apogee's order form and price list for the registered
episodes; it contains no distribution terms.
