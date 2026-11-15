# vendor/shareware/wolf3d — Wolfenstein 3-D shareware installation floppy

`fetch.sh` builds `wolf3d-sw14-1440.img`, a 1.44M FAT12 installation floppy
holding the unmodified shareware release of **Wolfenstein 3-D, episode 1,
version 1.4** (id Software, 1992, published by Apogee Software Productions),
and copies its vendor texts to `extras/`.

The archive, the image and the extras are third-party shareware. They are
never committed, and fetching does not add any redistribution right. The
maintainer approved publishing this release on 2026-11-15 (decision-26,
amendment of that date), so its media set is imported with the licence
statement `--open --license LicenseRef-apogee-vendor-v11.01.92
--license-text VENDOR.DOC` (Usage below): the set is open under Apogee's
`VENDOR.DOC` terms (revision `[V.11.01.92]`), that text travels in the set
as its terms text, and the push guard lets the set into `unemu/`. Imported
without the statement the set stays `user-supplied` in the user's private
store (decision-26 §8). The terms are summarised below, not interpreted; the
full texts are never in the repository, only in `extras/` and in the set.

## Usage

```sh
vendor/shareware/wolf3d/fetch.sh                                    # download once, then offline
SHAREWARE_ARCHIVE=/path/to/1wolf14.zip vendor/shareware/wolf3d/fetch.sh  # never uses the network
cargo run --offline -p cli -- image media import vendor/shareware/wolf3d/wolf3d-sw14-1440.img --title "Wolfenstein 3-D: Episode 1 (shareware 1.4)" --tag wolf3d-shareware:1.4 --label '1=Disk 1 of 1 - Shareware v1.4 installation' --extra vendor/shareware/wolf3d/extras/VENDOR.DOC:VENDOR.DOC --extra vendor/shareware/wolf3d/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1992 --genre "first-person shooter" --platform DOS --open --license LicenseRef-apogee-vendor-v11.01.92 --license-text VENDOR.DOC --store /path/to/store --create
```

The import also records the set-level metadata: publisher Apogee Software
(`VENDOR.DOC` asks commercial distributors to name Apogee in every
description), developer id Software,
release year 1992, genre first-person shooter and software platform DOS. The
archive's files are dated 1993-01-01 (version 1.4); the year is the game's
first release, and `VENDOR.DOC` itself is revision `[V.11.01.92]`.

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
| Archive | `1wolf14.zip`, 728,992 bytes |
| SHA-256 | `c3d355ed9f887a94239140070b15d4042ec68a4a456171c2853edc04727b1171` |
| Primary | <https://ftp.funet.fi/pub/msdos/games/apogee/1wolf14.zip> |
| Fallback | <https://discmaster.textfiles.com/file/20928/400%20MB%20Of%20Top%20PC%20Shareware.iso/games/apogee/wolf3d/1wolf14.zip> |

The primary is FUNET's mirror of Apogee's shareware file area; its index
(`00-index.txt`, listing dated 19 Nov 1995) describes the file as "Castle
Wolfenstein 3D, with texture-mapped VGA graphics. Requires 286 or better.
Version 1.4." The fallback is the copy on the 1994 CD-ROM *400 MB of Top PC
Shareware*, indexed by DiscMaster; both were downloaded on 2026-11-15 and are
byte-identical. `WOLF.DAT` names the release: "Wolfenstein 3-D Version 1.4",
"Shareware Episode - BBS & Online Edition".

Other editions were not chosen: the later 3D Realms file area carries a
repack with a newer installer (`INSTALL.EXE`, `W3DSW14.SHR`; 856,401 bytes,
SHA-256 `cb2a2ef7ecef14152c65ff93cc3b84fbd3e8eb0c5c1de41a6fc8cdef559451a8`),
and the December 1992 upload kept in the gamers.org Wolfenstein archive
(747,094 bytes, SHA-256
`7755d3e6cc8897e9628fa1c13fe24d9657d901af2c3793ae258381431dcc69f2`) contains a
`CHKLIST.CPS` added after release by a virus scanner.

## Contents

The floppy holds the six archive members, in archive order. `INSTALL.BAT`
runs `DEICE.EXE`, which installs `WOLF.1` (a self-extracting zip) into
`\WOLF3D` on a chosen drive.

| Archive member | Bytes | SHA-256 |
| --- | ---: | --- |
| `INSTALL.BAT` | 77 | `c7184ead2efaa119849128a67ad3a1ad12c0a0d53135402e6db4e2ff409c4e8c` |
| `DEICE.EXE` | 17,303 | `4f77c1fee9c6e579ee325e83b0fc9d79254bb4837be950d06059260395fb4569` |
| `WOLF.DAT` | 277 | `7b1488120dbfa9a2c2899626bfc0f16432ec1827e73afa473e4eb25501c4b2bc` |
| `WOLF.1` | 709,658 | `ce042aeb2a6b32f7651b318d78203376baf72151e6f95d3cc3da212f693959e7` |
| `VENDOR.DOC` | 3,936 | `a2ab674de6d2e5ed75d5e67f592d1cc0357b201979457c4b144e78db91df3b30` |
| `FILE_ID.DIZ` | 350 | `840242ab486718872c865f4feed86b7bd874bf0d7e339977956e347dac97dfca` |

Files inside `WOLF.1`, for reference (the installer writes them; the floppy
keeps the payload as distributed):

| Payload member | Bytes | SHA-256 |
| --- | ---: | --- |
| `WOLF3D.EXE` | 109,959 | `3c9df119dc85ee49294275374652f22c691c4e6ea017dbba6b6d6d960a84891f` |
| `AUDIOHED.WL1` | 1,156 | `39351624ae6f8eef4b873e060c1a6f3e5ee7e81c4939c485275a89b145336338` |
| `AUDIOT.WL1` | 132,613 | `1e2c9ae30398a14c61a4ddd39aabaa0dbcc984cc4924a3e51df75574c259cfb9` |
| `GAMEMAPS.WL1` | 27,425 | `a6a6654b342f2c027bcb22bfce0a41f9fc0063b775e9e4da0c771970e53e11aa` |
| `MAPHEAD.WL1` | 402 | `3458f661c9b875bca99ea22a7267771fad8f2a33c699ea732cf7c3322909bf8c` |
| `VGADICT.WL1` | 1,024 | `59878fec65f033b00dbb1240317d1793f5858213dc8013b4d68f9ac8b45b0c80` |
| `VGAGRAPH.WL1` | 326,568 | `d5176f843c53415132db199c19f38591eaf3cedd35a4f7eb2698c1865d83030d` |
| `VGAHEAD.WL1` | 471 | `f4cc800dc8444373092d4eaa5d6ab59d63d23a510a9d38b73ca9b3dbb700d18b` |
| `VSWAP.WL1` | 742,912 | `698f217257e2cbb951a4d110ba09140291f38d0121b3784d1d6be59c03a6b47b` |
| `CATALOG.EXE` | 28,715 | `9c15f5a2d382418362c8fc08504facad1b017a43660dc43807bd537e82ce1fa2` |
| `ORDER.FRM` | 3,321 | `1ff74f2750659268e138d0ea0e17d9fc43274ca5872a4dbdb2ad62988b9aaa76` |

Outputs:

| Output | SHA-256 |
| --- | --- |
| `wolf3d-sw14-1440.img` (1,474,560 bytes) | `3d0ae4d9b0916b43dba1d02f427a2e2f9469bc55abbd5d105fa33369d44e8c70` |
| `extras/VENDOR.DOC` | `a2ab674de6d2e5ed75d5e67f592d1cc0357b201979457c4b144e78db91df3b30` |
| `extras/ORDER.FRM` | `1ff74f2750659268e138d0ea0e17d9fc43274ca5872a4dbdb2ad62988b9aaa76` |

The image is a standard 1.44M FAT12 volume (512-byte sectors, one sector per
cluster, two 9-sector FATs, 224 root entries, media byte F0h), label
`WOLF3DSW14`, serial `1993-0101`. Every file keeps the archive's DOS date and
time (1993-01-01 13:40; the publisher's time of day encodes the version); the
volume label uses 1980-01-01 00:00. The volume sector is the project's own
non-bootable stub, as in `vendor/hdd`: jump `EB 3C 90`, OEM text
`"UNEMUPC "`, drive 00h, extended-BPB signature 29h, and at 3Eh `INT 18h`
then `HLT` in a loop. Unused sectors are zero.

## Terms

`VENDOR.DOC` (revision `[V.11.01.92]`; on the floppy as distributed and in
`extras/VENDOR.DOC`, SHA-256
`a2ab674de6d2e5ed75d5e67f592d1cc0357b201979457c4b144e78db91df3b30`) holds the
complete text. In summary:

- **Scope.** It states the only terms on which Apogee permits *commercial*
  distribution of version 1.4 and its related files, the document included.
- **Definitions.** Commercial distribution is distribution for money or other
  consideration: tangible media, retail, mail order, catalogues, advertising
  or trade shows. It excludes distribution by not-for-profit organisations, by
  user groups to their members, and electronically by BBS; the document calls
  those, and free distribution, non-commercial:

  > Distribution is "non-commercial" when for free, or by any not-for-profit
  > corporation or tax exempt organization, or by hobby, user or computer
  > interest group to its members, or by any BBS.

- **A (ownership).** Apogee keeps the copyright, the distribution right and
  the trademarks, except as it licenses them.
- **B (conditions for commercial distributors).** [1] market it as shareware
  ("try before you buy"); [2] sell only the current version; [3] copy only
  from Apogee's masters; [4] distribute the complete program with all related
  files in the format as released; [5] name "Apogee" in every catalogue or
  promotional description.
- **C (scope of the right).** The right is personal; it includes no right to
  "sublicense or otherwise cause any copying or distribution of the Program
  by anyone else without our consent in writing", to rent or lease it, or to
  sell it as part of a hardware or software package.
- **D, F, H.** No warranty; either side may end the licence on 30 days'
  written notice, with a sell-off period; equitable remedies, indemnity for
  the distributor's copying, and courts in Dallas, Texas.

It sets no terms for non-commercial distribution beyond defining it.
`ORDER.FRM` is Apogee's order form and price list for the registered
episodes; it contains no distribution terms.
