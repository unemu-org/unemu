# vendor/shareware/keen4 — Commander Keen 4 shareware installation floppy

`fetch.sh` builds `keen4-sw14-1440.img`, a 1.44M FAT12 installation floppy
holding the unmodified shareware release of **Commander Keen 4, "Secret of
the Oracle", version 1.4** (id Software, 1991, distributed by Apogee
Software Productions), and copies its shareware texts to `extras/`.

The archive, the image and the extras are third-party shareware. They are
never committed, and fetching does not add any redistribution right. The
maintainer approved publishing this release on 2026-11-15 (decision-26,
amendment of that date), so its media set is imported with the licence
statement `--open --license LicenseRef-apogee-readme-keen4-v1.4
--license-text README.DOC` (Usage below): the set is open under the
release's `README.DOC`, its only text about distribution (it has no revision
mark, so the identifier names the release), that text travels in the set as
its terms text, and the push guard lets the set into `unemu/`. Imported
without the statement the set stays `user-supplied` in the user's private
store (decision-26 §8). The terms are summarised below, not interpreted; the
full texts are never in the repository, only in `extras/` and in the set.

## Usage

```sh
vendor/shareware/keen4/fetch.sh                                  # download once, then offline
SHAREWARE_ARCHIVE=/path/to/4keen.zip vendor/shareware/keen4/fetch.sh   # never uses the network
cargo run --offline -p cli -- image media import vendor/shareware/keen4/keen4-sw14-1440.img --title "Commander Keen 4: Secret of the Oracle (shareware 1.4)" --tag keen4-shareware:1.4 --label '1=Disk 1 of 1 - Shareware v1.4 installation' --extra vendor/shareware/keen4/extras/README.DOC:README.DOC --extra vendor/shareware/keen4/extras/ORDER.FRM:ORDER.FRM --open --license LicenseRef-apogee-readme-keen4-v1.4 --license-text README.DOC --store /path/to/store --create
```

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
| Archive | `4keen.zip`, 648,640 bytes |
| SHA-256 | `b024f2fb48fbf360c6fac2932acd72e4585595cae41a6223c2607bc42c111421` |
| Primary | <https://ftp.funet.fi/pub/msdos/games/apogee/4keen.zip> |
| Fallback | <https://discmaster.textfiles.com/file/20928/400%20MB%20Of%20Top%20PC%20Shareware.iso/games/apogee/4keen.zip> |

The primary is FUNET's mirror of Apogee's shareware file area; its index
(`00-index.txt`, listing dated 19 Nov 1995) describes the file as "Commander
Keen part 4, "Secret of the Oracle", the first of two parts to "Goodbye,
Galaxy!"". The fallback is the copy on the 1994 CD-ROM *400 MB of Top PC
Shareware*, indexed by DiscMaster; both were downloaded on 2026-11-15 and are
byte-identical. `GALAXY.DAT` names the release: "Shareware Episode Version
1.4 - BBS Version".

Other editions were not chosen: the later 3D Realms file area carries a
repack with a newer installer (`INSTALL.EXE`, `CK4SW14.SHR`; 729,683 bytes,
SHA-256 `f7c63feb8bbec817646d95aa26313df1654c01eda8f7ccc5cfd0c0bc0b5e305e`),
and a December 1992 edition split into ARJ volumes survives only as
third-party CD-ROM copies that differ in size.

## Contents

The floppy holds the five archive members, in archive order, under upper-case
DOS names. `INSTALL.BAT` runs `DEICE.EXE`, which installs `GALAXY.1` (a
self-extracting zip) into `\KEEN` on a chosen drive.

| Archive member | Bytes | SHA-256 |
| --- | ---: | --- |
| `install.bat` | 81 | `b79322a82348ad1bd98e135c4402de8f04021031a727e2ae4d3849b4a93b833f` |
| `deice.exe` | 17,303 | `4f77c1fee9c6e579ee325e83b0fc9d79254bb4837be950d06059260395fb4569` |
| `galaxy.dat` | 280 | `0815e45e2fd8068501725b1b2e6accde6c60e09897a371fdc946c4d7e85f4cd4` |
| `galaxy.1` | 630,898 | `d6784de99672f5ab16d0d91e78b7ead91bdce7511092e091605acceb6adf72fe` |
| `file_id.diz` | 344 | `de1d9d2e597fdb5f03d8bc0aa41d46c6436cb699ac82262940d5f45dd756a337` |

Files inside `GALAXY.1`, for reference (the installer writes them; the floppy
keeps the payload as distributed):

| Payload member | Bytes | SHA-256 |
| --- | ---: | --- |
| `KEEN4E.EXE` | 105,108 | `bdbb46746e02c0dddcba9b24fa464c26cc7e3514a3b3931267c8c17b9fb9c405` |
| `EGAGRAPH.CK4` | 520,581 | `9805168fb910385ad40455541cbcfa1aaa61f9d81b2504274ee0ca0ec4b71550` |
| `AUDIO.CK4` | 33,325 | `66dcf6070a4bc5c8933815b15dab7145ddcccb959f8f43af2a0449b085a04366` |
| `GAMEMAPS.CK4` | 99,040 | `01186f8cb99257b69a4f475186deb9ba79b872ed52378131c79c21f6344753ea` |
| `README.DOC` | 1,360 | `58ed579502a7c8e802381225e93ba0b2f5860908c7f4d234b5897385afb447e3` |
| `CATALOG.EXE` | 28,715 | `9c15f5a2d382418362c8fc08504facad1b017a43660dc43807bd537e82ce1fa2` |
| `ORDER.FRM` | 3,321 | `1ff74f2750659268e138d0ea0e17d9fc43274ca5872a4dbdb2ad62988b9aaa76` |

Outputs:

| Output | SHA-256 |
| --- | --- |
| `keen4-sw14-1440.img` (1,474,560 bytes) | `c34c25fae130a4b432ae6ae1fcaea1eb8f4763c3e184785a2f551d14c47a4578` |
| `extras/README.DOC` | `58ed579502a7c8e802381225e93ba0b2f5860908c7f4d234b5897385afb447e3` |
| `extras/ORDER.FRM` | `1ff74f2750659268e138d0ea0e17d9fc43274ca5872a4dbdb2ad62988b9aaa76` |

The image is a standard 1.44M FAT12 volume (512-byte sectors, one sector per
cluster, two 9-sector FATs, 224 root entries, media byte F0h), label
`KEEN4SW14`, serial `1992-0201`. Every file keeps the archive's DOS date and
time (1992-02-01 13:40; the publisher's time of day encodes the version); the
volume label uses 1980-01-01 00:00. The volume sector is the project's own
non-bootable stub, as in `vendor/hdd`: jump `EB 3C 90`, OEM text
`"UNEMUPC "`, drive 00h, extended-BPB signature 29h, and at 3Eh `INT 18h`
then `HLT` in a loop. Unused sectors are zero.

## Terms

This release carries no `VENDOR.DOC` or licence file. `README.DOC` (inside
`GALAXY.1`; written to `extras/README.DOC`, SHA-256
`58ed579502a7c8e802381225e93ba0b2f5860908c7f4d234b5897385afb447e3`) is its
only text about distribution. It names id Software as copyright holder and
Apogee as distributor, gives a compatibility hint for Super VGA cards, and
points to the in-game F1 screens for help and ordering information. It grants
no licence and sets no conditions; its one sentence on shareware asks for
payment:

> Please do not forget that this game is distributed through shareware and
> as such we, our dealers and distributors rely on your honesty and financial
> support in order that we can continue to provide the quality products and
> exceptional value you have come to expect from Id Software, Inc. and Apogee
> Software Productions.

The in-game F1 text was not read. Wolfenstein 3-D's `VENDOR.DOC` defines its
"Program" as that game only, so it does not cover this one. `ORDER.FRM` is
Apogee's order form and price list for the registered episodes; it contains
no distribution terms.
