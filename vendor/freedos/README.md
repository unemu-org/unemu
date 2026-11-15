# vendor/freedos — bootable FreeDOS floppy fixtures

`fetch.sh` builds three **bootable FAT12 FreeDOS floppy images** in one run:

- `freedos.img`: 1.44 MB, 1,474,560 bytes (2880 sectors), used by the existing
  open-BIOS boot and DOS session goldens.
- `freedos-1200.img`: 1.2 MB, 1,228,800 bytes (2400 sectors), used by the
  user-supplied IBM 5170 BIOS boot measurement. The 1985-06-10 BIOS supports
  360K, 1.2M and 720K drive types, so this measurement uses a 1.2M drive.
- `freedos-tools-1200.img`: 1.2 MB, 1,228,800 bytes (2400 sectors), an open
  installation and utility floppy for the AT. It extends `freedos-1200.img`
  with the programs below and `README.TXT`. All tools fit; none is omitted.

All are read-only boot fixtures. Images and downloaded sources are not
committed; pinned FreeDOS inputs rebuild them reproducibly, like
`vendor/kiselev-bios` and `vendor/test386`.

## What it produces

The two minimal boot floppies contain the same files in the root:

- **`KERNEL.SYS`** — the FreeDOS kernel, **8086 build** (`ke2044_86f16`, FAT12/16,
  no FAT32). The default installer kernel is 386-only and would fault on our
  real-mode 286-class core; the 8086 build boots in pure real mode without
  A20/HMA.
- **`COMMAND.COM`** — FreeCOM, **8086 build** (`com086`).
- A **FreeDOS FAT12 boot sector**, assembled from the kernel source
  (`boot/boot.asm`, `-dISFAT12`) and installed over the formatted image's boot
  code while preserving the geometry-specific BPB `mformat -f 1440` or
  `mformat -f 1200` wrote.

No `FDCONFIG.SYS`/`AUTOEXEC.BAT` is placed: with none present the kernel loads
`COMMAND.COM` as the default shell. FreeCOM asks for the date and time;
two Enter keypresses accept them and reach the DOS prompt.

The tools floppy also contains the following programs. Versions and licence
labels come from `APPINFO/*.LSM` in each package (kernel metadata is
`docs/fdkernel.lsm`). The FreeDOS project and each program's authors supply
these freely redistributable tools; this repository does not author them.

| Program | Package version | Licence | Compatibility evidence in the pinned archive |
|---------|-----------------|---------|----------------------------------------------|
| `KERNEL.SYS` | 2044 FAT12/FAT16 | GPL v2 or later | `docs/readme.txt`: 8086 FAT16 binaries |
| `COMMAND.COM` | FreeCOM 0.86 (`com086`) | GPL v2 or later | The existing pinned 8086 shell fixture |
| `SYS.COM` | Kernel 2044 | GPL v2 or later | Same `ke2044_86f16.zip`; `docs/readme.txt` describes 8086 binaries |
| `FDISK.EXE` | 1.3.4a | GPL v2 | Included `SOURCE/FDISK/SOURCES.ZIP`, `FDISK/MAKEFILE`: `-1` selects 80186/286 instructions; 186/286 or later, not 8086 |
| `FORMAT.EXE` | 0.92 | GNU GPL (v2 text in `DOC/FORMAT/LICENSE.TXT`) | Included `SOURCE/FORMAT/SOURCES.ZIP`, `makefile`: `-0` (8086) and UPX `--8086` |
| `XCOPY.EXE` | 1.7 | GPL v2 | Included `SOURCE/XCOPY/SOURCES.ZIP`, `makefile`: `-0` (8086) and UPX `--8086` |
| `EDIT.EXE` | 0.9b | GPL v2 | LSM: DOS/Borland C; included `SOURCE/EDIT/SOURCES.ZIP`, `EDIT.MAK`: 16-bit Borland build, optional `-1` targets 186/286, UPX `--8086` |
| `MEM.EXE` | 1.12 | GPL v2 | Included `SOURCE/MEM/SOURCES.ZIP`, `MEM/MKFILES/GENERIC.MAK`: UPX `--8086`; 16-bit small-model compiler settings in `WATCOM.MAK` |
| `CTMOUSE.EXE` | 2.1b4 | GPL v2 | Included `SOURCE/CTMOUSE/SOURCES.ZIP`, `CTMOUSE.ASM`: `.286`; `DOC/CTMOUSE/HISTORY.TXT` records 286/386 support; 286 or later, not 8086 |

`EDIT.HLP` accompanies EDIT under its licence. The LSMs mostly declare DOS or
FreeDOS without a minimum CPU, so the more specific history, technical notes
and bundled build instructions above establish the per-program CPU requirements.
FDISK requires 186/286 or later, not 8086; CTMOUSE requires 286 or later.
The complete tools set therefore requires a 286. CTMOUSE TECHNOTE item 5
describes the older 2.0 implementation and does not establish 2.1b4 compatibility.
These are compatibility evidence, not a claim that every optional tool has
been exercised end to end in the emulator. No included tool requires a 386.
Full licence texts and sources remain in the pinned upstream archives under
`DOC/` and `SOURCE/` (the kernel uses `docs/`).

## Pinned versions (provenance)

| Component | Source | Pin |
|-----------|--------|-----|
| Kernel (8086) `KERNEL.SYS` | `github.com/FDOS/kernel` release | tag `ke2044`, asset `ke2044_86f16.zip` |
| Shell (8086) `COMMAND.COM` | `github.com/FDOS/freecom` release | tag `com086`, asset `command.zip` |
| Boot sector `boot/boot.asm` | `github.com/FDOS/kernel` source | commit `8535ddb7a783f94b3aae844c8fe7f5ccce9037eb` (tag `ke2044`) |

Archive SHA-256 sums are checked in `fetch.sh`; the assembled boot sector is
size-checked (512 bytes). Each image is checked for its size, the `0x55AA`
signature and SHA-256:

| Image | Bytes | SHA-256 |
|-------|-------|---------|
| `freedos.img` | 1,474,560 | `b7b088333f7f30584d768941fecea8288b63f2d61e13ec6c9472439eda66e51f` |
| `freedos-1200.img` | 1,228,800 | `4ee55e9c44d553041b9bbde99cacae31472f12debf6c9b9c09c5b9343c435d7e` |
| `freedos-tools-1200.img` | 1,228,800 | `8d91283a1c0d6fc5b5fdb5e40db5b66dd9fabbf014d37262b7892f3b150b6f63` |

**MEASURED:** `fetch.sh` reproduces all three pinned image hashes from the pinned archives and source with mtools 4.0.49, whose version appears in the boot-sector OEM name.

The six utility archives come from the FreeDOS 1.3 repository at
`https://www.ibiblio.org/pub/micro/pc-stuff/freedos/files/repositories/1.3/base/<name>.zip`.
`fetch.sh` pins each archive's full SHA-256, including FORMAT 0.92 and XCOPY 1.7
as supplied in that repository; the repository name alone is not a version pin.

**MEASURED:** Two offline tools builds with mtools 4.0.49 compare byte for byte,
produce the pinned SHA-256, and leave 854,016 bytes free. The system files and
loader code come from the verified 1.2 MB boot fixture; only
the tools volume identity fields and filesystem contents change.

A hash mismatch fails the build and reports both hashes. Check sources and tools;
changing a pinned output hash requires deliberately rebuilding and reviewing the
goldens' expectations.

The minimal boot floppies use label `FREEDOS` and serial `2895-9853`.
The tools floppy uses label `FDTOOLS` and serial `2895-9854`, so DOS can
distinguish it from the boot floppy when switching media in the same drive.
`SOURCE_DATE_EPOCH=315532800` and `TZ=UTC` fix all mtools
volume-label and file creation, access and modification timestamps at the FAT
epoch, 1980-01-01 00:00:00 UTC. `mcopy` deliberately omits `-m`, so archive and
host file mtimes do not enter the image. Use mtools with `SOURCE_DATE_EPOCH`
support (the devShell provides 4.0.49); an incompatible build fails the image hash
check.

## License

FreeDOS kernel and FreeCOM are **GNU GPL v2 or later**, freely redistributable.
This project only *reads* the image at runtime (a boot fixture); it does not link
or incorporate FreeDOS code. Upstream license text ships inside each archive
(`docs/` in the kernel zip, sources in the FreeCOM zip). Anyone passing the
built image on takes on the applicable GPL source obligations: provide the
corresponding source as required by each package's licence. The pinned utility
archives ship it in `SOURCE/*/SOURCES.ZIP`; retain those archives and the pinned
kernel/FreeCOM sources with any redistribution.

## FreeDOS 1.4 Floppy Edition

`fetch-floppy-1.4.sh` fetches the official **FreeDOS 1.4 Floppy Edition**,
released by the FreeDOS Project on 2025-04-02, and the corresponding source
of every program on its disks, for the library's installation sets. Nothing
downloaded is ever executed: every file is checked against a pinned SHA-256,
the disk images are copied out of the edition unmodified, and the sources are
packed into one deterministic tar. Outputs land in `floppy-1.4/` and
downloads in `build/floppy-1.4/`, both gitignored. When every output already
matches its pin the script stops at once, without the network (CI caches
`floppy-1.4/` and runs the script as the last step of the fixtures lane,
`scripts/ci-gate.sh`). A cold run downloads about 330 MB (the edition, 23 MB,
and the release LiveCD, 294 MB, for the sources) plus four shallow git
fetches; the LiveCD zip and its ISO are deleted once the package zips verify,
which stay cached. A download is renamed into the cache only after its hash
matches, and a bad response from the release directory falls back to the
mirror. `pc_image::template::MEDIA` describes the sets and `image template
build` imports them (commands.md, Official template images; machine-images
§5.5).

**The edition.** `FD14-FloppyEdition.zip` comes from
`https://download.freedos.org/1.4/`, falling back to the ibiblio mirror
`https://www.ibiblio.org/pub/micro/pc-stuff/freedos/files/distributions/1.4/`;
its SHA-256 `45b1fa7c52dd996c3bfa5e352ffcd410781b952a6ad629f15a4c9ec4bbaefc5a`
is the one the release's own `verify.txt` lists. The script prints its member
list and refuses any other: three directories of disk images, one per floppy
format, and `readme.txt`. Per its readme, `x86BOOT.img` is the boot disk with
the floppy installer and `x86DSK??.img` hold the FreeDOS packages; their
packages sit in a Slicer archive (`freedos.saf`, `freedos.001`…) that the
installer unpacks. The edition contains no source code; the readme points to
the other release media for it.

| Disk image | Format | Bytes | SHA-256 |
|---|---|---|---|
| `120m/x86BOOT.img` | 1.2M | 1,228,800 | `03df6088be016e57a6c44275f5bb9ab0244db71de1360957fd76ba83243b6a77` |
| `120m/x86DSK01.img` | 1.2M | 1,228,800 | `83505b32f4165f393e76d5de3e59610bf49ffad6c349fd68a2f4795e0d5153af` |
| `120m/x86DSK02.img` | 1.2M | 1,228,800 | `05303837dbd336c3fdc20fe55ea3e2778de467ecf2a68dbce905c721b0ddb94c` |
| `120m/x86DSK03.img` | 1.2M | 1,228,800 | `f82ebe5153a2fb5a37a1569c8927447c086b2d06c77c979bea4ad65b14b4b111` |
| `120m/x86DSK04.img` | 1.2M | 1,228,800 | `ddbbeb52f90c86ef4952a6c4e8b923577071712427a9cbb1578964fa8c9c7487` |
| `120m/x86DSK05.img` | 1.2M | 1,228,800 | `e1687f7672e707cc7e70ac4b11a4a0bb6a4e37a05f1d93914d6867d481338768` |
| `120m/x86DSK06.img` | 1.2M | 1,228,800 | `57b2e375740c061438f8e1446afd202a718278f3c20713c444454cf379afc913` |
| `120m/x86DSK07.img` | 1.2M | 1,228,800 | `b5b7cc6dc0f85ad9c7a961311c3ccf2d811ae0dd37104f44bee8c31a2bc06fe3` |
| `144m/x86BOOT.img` | 1.44M | 1,474,560 | `552f7cbb0625960c050a3e682a4d2121cc2ffdfa9dc9d592ccd49edf179333dc` |
| `144m/x86DSK01.img` | 1.44M | 1,474,560 | `b9dd5315f50a8b770b3d2ecb623b18ed067660e22f421504d199254f7f653516` |
| `144m/x86DSK02.img` | 1.44M | 1,474,560 | `4a951f15da8dd98e54ea84f07fe2a7802e914aa8e66942f646d2e11ca3943e59` |
| `144m/x86DSK03.img` | 1.44M | 1,474,560 | `fbfd172e1ac4bc4abb3f656cae94e2727d2b48b4d02cef0362564048f83f35da` |
| `144m/x86DSK04.img` | 1.44M | 1,474,560 | `894d49c4dfdd294e33878ca317e19a0322f267f639b247a6b2a669799b4720f8` |
| `144m/x86DSK05.img` | 1.44M | 1,474,560 | `0362e084497c5c995ccab1523c4e39f94b7139f988ffa3a4f7e871452c352f3c` |
| `144m/x86DSK06.img` | 1.44M | 1,474,560 | `7a93d1d4e47f5edd5d0a9f17d57c3417ae135052db9449a67aa374a3b4316d80` |
| `720k/x86BOOT.img` | 720K | 737,280 | `cbca90627b944319f02043bc41f1715ff5366b23e5e8e75aa3ca23bb564e0789` |
| `720k/x86DSK01.img` | 720K | 737,280 | `02650e6d952aec1b7465574cc8ddb0f29058444029050f06ab29094589b2b889` |
| `720k/x86DSK02.img` | 720K | 737,280 | `ad2c055b22312f217a76bf0af25be6bd09b5e208b416d4d51cd7cc8bf9d0cd57` |
| `720k/x86DSK03.img` | 720K | 737,280 | `49d79c2a4d36bb026008f31ebf8e7597330a04a0f396111862e6d36ba2398613` |
| `720k/x86DSK04.img` | 720K | 737,280 | `17f756003d3436b5371ab3108c37a73e78eb028ed89a75b067abde3f5ceb7e62` |
| `720k/x86DSK05.img` | 720K | 737,280 | `c3aeb780e1a2cabc5c50e30d2078741a3026da9085ef052d4edaa42ee2b9685c` |
| `720k/x86DSK06.img` | 720K | 737,280 | `d54ce27af491a15128d61f6ee9581cec92c4e823e59e79bb8875147494a0123f` |
| `720k/x86DSK07.img` | 720K | 737,280 | `ea342ba1cb40ba253540d4a935b2ae2834d96d0a35e943f32caaa38684601425` |
| `720k/x86DSK08.img` | 720K | 737,280 | `69ee59879182cdad0bf1a03b318a30dd69c11cd9c4985d875e4bf1587c922fec` |
| `720k/x86DSK09.img` | 720K | 737,280 | `f344088ceb48f05c718d0158a902e584c1f66a7e0d8d67ab2adf9069e122051e` |
| `720k/x86DSK10.img` | 720K | 737,280 | `de31af59860e1f59a67a2036113a0af4db8f72bebb4812fe5aa291c5faf3cce0` |
| `720k/x86DSK11.img` | 720K | 737,280 | `70e82a287d4d89b40c975d6a62700633cdd6dac0d55da53953edfc37c69a6c0b` |

The library offers one set per format, the disks in installer order, labelled
"Disk N of M - boot disk and installer (x86BOOT.img)" and "Disk N of M -
packages (x86DSKnn.img)": `freedos-floppy-1.44m` (7 disks),
`freedos-floppy-1.2m` (8) and `freedos-floppy-720k` (12).

**Extras of every set**:

| Extra | DOS name | Kind | From | SHA-256 |
|---|---|---|---|---|
| `LICENSES.TXT` | `LICENSES.TXT` | document | `floppy-1.4-licenses.txt` of this repository (CC0-1.0), CRLF line ends: the licence text | `35d9478a2f5b3e54ded8d20b1b23feec10b0abd2e1e6715c77ddf314e7bf1a7f` |
| `freedos-1.4-floppy-src.tar` | `FD14SRC.TAR` | archive | built by the script (below): the source archive | `dfa7006e742cb9676a0a6401ce64214089923f055f4a7ba8f36960115b5362b6` |

The release documents (`readme.txt` of the edition; `announce.html`,
`changes.log`, `report.html` and `verify.txt` of the release directory) state
no licence of their own, so the open sets do not carry them; the hashes above
come from `verify.txt`. The edition ships no separately licensed manual. Its
documentation travels inside the disks and the source tar: every package's
`DOC/` files under the package's licence, and the FreeDOS HTML Help
(`htmlhelp`) under the GNU FDL.

**Sources.** The edition's packages are the release's packages without their
`SOURCE/` trees (the release report lists 69 for the Floppy-Only Edition;
FDI-x86's `SETTINGS/X86_ALL.LST` and `X86_FDI.LST` at its `FreeDOS-v1.4` tag
name the same, plus FDBANNER on the boot disk). The online 1.4 repository has
moved on since the release, so the script takes the packages, with their
sources, from the release LiveCD: `FD14-LiveCD.zip` (SHA-256
`2020ff6bb681967fd6eff8f51ad2e5cd5ab4421165948cef4246e4f7fcaf6339`, listed in
`verify.txt`), its `FD14LIVE.iso` (SHA-256
`c48a9dcf4b8e22f44e268a9879745f0bd88c061195ac584e6ef2deb0477f81fb`), read by
`iso-extract.py`, a small ISO 9660 reader (ECMA-119) that copies named files
to paths given on its command line, bounds every record and extent, and
executes nothing. FDINST is part of the fdnpkg package (its source is in
fdnpkg's `SOURCES.ZIP`); FDNET's `DHCP.EXE` is mTCP's, byte for byte, so the
mTCP package comes too. FDBANNER is not on the LiveCD; the repository's
`util/fdbanner.zip` holds the same `FDBANNER.COM` and its source.

**MEASURED:** every program file on the 1.44M boot disk except the installer's
own files is byte-identical to the file of the same name in these packages
(`KERNEL.SYS` is the kernel package's `KERNL86.SYS`). The installer files come
from the tags below, with the release builder's placeholders (`$OS_NAME$`,
`$OS_VERSION$`, `$OS_VOLUME$`, `$AUTOEXEC$`, `$CONFIG$`) filled in, and
`freedos/version.fdi` is written by the builder. The Slicer disks were not
unpacked here; their contents follow from the report and the package lists.

The installer and the build environment, at the `FreeDOS-v1.4` tag each
project set for this release:

| Part | Repository | Commit | Licence |
|---|---|---|---|
| Floppy installer (`setup.bat`, `fdauto.bat`, `fdconfig.sys`, `fdwrapup.bat`) | `github.com/shidel/FDI-x86` | `2ba66516bce2703a1b33e2c2190c7cebf339089f` | GPL-1.0-or-later |
| Configuration templates (`freedos/configs/*`) | `github.com/shidel/FDI` | `23979ab50c9418052a84155795aa378b88bb73ab` | GPL-1.0-or-later |
| Installer translations and fonts (`freedos/nls/setup.*`, `cp*.v8f` with their `.FNT` sources) | `github.com/shidel/fd-nls` (`LICENSE`, `README.md`, `fdi/language`, `fdi-x86` only) | `079e0a38eef71d39559768f263ecd025257409da` | GPL-2.0-or-later (README: "version 2 or later") |
| Release build environment | `gitlab.com/FreeDOS/OS/builder` | `08f1920798d01276aca2576c74db7c1dcfd2870e` | MIT |

`freedos-1.4-floppy-src.tar` holds `LICENSES.TXT`, `packages/<group>/<name>.zip`
(exactly the packages below, unmodified), `installer/FDI-x86/`,
`installer/FDI/`, `installer/fd-nls/` (`git archive` of the pinned commits)
and `builder/`, in a ustar archive with sorted members, owner 0, fixed modes
and every mtime at the release date, 2025-04-02 00:00 UTC (22 MB).
**MEASURED:** repeated runs give the pinned SHA-256, and a cold run from
fresh downloads gave outputs byte-identical to a warm run's.

| Package | Version | LSM Copying-policy | SPDX used | In the source tar | SHA-256 of the package |
|---|---|---|---|---|---|
| `fdimples` | 0.11.9 | GNU General Public License, Version 2 | GPL-2.0-only | `packages/apps/fdimples.zip` | `3259e011cad448a20863c1ebdab2fb1d509b829765efe0d1ad5bc2cac3857fcf` |
| `gzip` | 1.2.4a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/archiver/gzip.zip` | `a21e7ffa045ee5c19be7e49757bfe03828295a1a3ab67bf54f2bf7a2182de8c5` |
| `slicer` | 0.24 (BETA) | Simplified (2-Clause) BSD License | BSD-2-Clause | `packages/archiver/slicer.zip` | `48598beb09e72548710b68e4699b51c16694a90801764328f12b5cb1cb2962a2` |
| `unzip` | 6.00a | Open Source, see license | Info-ZIP | `packages/archiver/unzip.zip` | `ce22cde8ee2185f7e9312a61413ce2fe7879df9aebbf1cf04f0c4181456e4c17` |
| `zip` | 3.0 | Open Source, see license. | Info-ZIP | `packages/archiver/zip.zip` | `e139f09786aa77678d06c6a266e324bd74bd0a82ee4ebd95da85c5c2576dc4dc` |
| `append` | 5.0-0.7 | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/append.zip` | `5bbd29b12309cc6966796f90a2580714ea902eb19e744a7d2905bb5b2a9144c6` |
| `assign` | 1.4a | GNU General Public License, Version 2 | GPL-1.0-or-later | `packages/base/assign.zip` | `6e79e0fc10d18d5153f7c42b5cbbbfe0c64575c0b79f398dd66b6383b07beb49` |
| `attrib` | 2.1a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/attrib.zip` | `b5a77bd0c20a821bf0085dfed7cff408a8dd72603506c0abda1e2c54c6303964` |
| `chkdsk` | beta 0.9.2 (rev A) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/chkdsk.zip` | `42a7120307e6604147ee663b3cba2b346bdb1305961eed0357ec6130cd23ba4e` |
| `choice` | 4.4a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/choice.zip` | `fae8902974b165dbaaa47c87ad171d0a073f94f0610a71ebec0c5f808a68079b` |
| `comp` | 1.04a | MIT license | MIT | `packages/base/comp.zip` | `3687e25eb78f1164a66e4f7328bc37952c402fba3ba888df64ad59f323114d96` |
| `cpidos` | 3.0a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/cpidos.zip` | `3a3daed5de2daf7ed39d0f3f8bb8b96ff23a79e2cda5622a1d8796d197261e7c` |
| `ctmouse` | 2.1b4 | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/ctmouse.zip` | `dd378ddfe5b6948d3073c12acaf0ca31bf60d0898f1cd708b6858083a14a41c7` |
| `debug` | 2.50 | MIT License | MIT | `packages/base/debug.zip` | `0ceabfacdad7066d159419bedd2a5299d583fb04ba0540d8a89df3e5563a09f7` |
| `defrag` | 1.3.2a | GNU General Public License, version 2 | GPL-2.0-or-later | `packages/base/defrag.zip` | `a2af8b47652616da9a33aee6e61193fdff729616120f594067a4c6faf95763de` |
| `deltree` | 1.02g.mrlg (rev A) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/deltree.zip` | `6f603150a3d9004314dcfaba6c14f6c80e21dbfd2ef88aef602162bb2a4def3d` |
| `devload` | 3.25a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/devload.zip` | `8ed0fcd7ac0d66d9f6b351c5b5cf4c69026f60033bcb8b2552c3ae23c95c99d9` |
| `diskcomp` | 06jun2003 (rev A) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/diskcomp.zip` | `e4f266f1e4e1d4dd8b90530ed9f911fcddb131d6d6008a4b524402561a56ac78` |
| `diskcopy` | beta 0.95 (rev A) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/diskcopy.zip` | `23eb5cbce5aa4ad767516d2ce259b76cfe3c82818783d5f7bb84745847fbc558` |
| `display` | 0.13d | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/display.zip` | `f05a0130309c2e52620aa14f04e9c499c4f8c04a1e58266e9fa9baa873ee024e` |
| `edit` | 0.9b | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/edit.zip` | `d794ffb6c2a2d04f96a6354ac0eded9c05f5592e7e856c3c28b48e4ba1cb030e` |
| `edlin` | 2.24 | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/edlin.zip` | `f8729654f985160867d0af99790515366b9067af6c83ff64e0d830533da92b5a` |
| `exe2bin` | 1.5a | Sybase Open Watcom Public License, Version 1.0 | Watcom-1.0 | `packages/base/exe2bin.zip` | `a2e3cb2b29abf32a92b5a7fbe65f508b8f30df41141ac9eeb75c19b1bcc003b8` |
| `fc` | 3.03a | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/fc.zip` | `5e1811bea9d60ffb34cef8e5851bc907cd57b130a37812480f5e759bcd94147d` |
| `fdapm` | 2009sep11 (rev A) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/fdapm.zip` | `bdda9abb631447508a659f1aec970079fe72b60a01c4e194255ffb34bfe77288` |
| `fdhelper` | 1.3.6 | GNU General Public License, Version 2 or later | GPL-2.0-or-later | `packages/base/fdhelper.zip` | `5180fa61c70ccf9084504100728d79e1aeb6d0103d4787b9e8f1cedb5e1e12ec` |
| `fdisk` | 1.4.3 | GNU General Public License, version 2 | GPL-2.0-only | `packages/base/fdisk.zip` | `1f705c34586e3468769e0baf59342891ac4c2329bbe87a140fcb4902381ab10d` |
| `fdxms` | 0.94.Bananas (rev A) | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/fdxms.zip` | `5fc8a472944211cfd6b456bc9753a061d6c969aaa16eda9541f983a19594d500` |
| `fdxms286` | 0.03.Temperaments (rev A) | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/fdxms286.zip` | `5586614769c542f75254a1caa562643089a17f6501e2507bfa8f488918fa8779` |
| `find` | 3.0b LFN (rev A) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/find.zip` | `71115327e2a05fe723f5a22ef2505067f9176ddcc714a1e2856b8fafb97afe1b` |
| `format` | 0.92a (pre-release) | GNU GPL | GPL-2.0-only | `packages/base/format.zip` | `80a7f0616fbe07ceb74c074bab2e79da880e6dd24cc588a43463a7182a312eae` |
| `freecom` | 0.86a | GNU General Public License, Version 2 (GPL) | GPL-2.0-or-later | `packages/base/freecom.zip` | `2529cf15c2ee7d7030ed99a6a88df2cf5eef87b9fe10f1c0fb643c38ea6aaa8e` |
| `graphics` | 2008-07-14a | GNU General Public License | GPL-2.0-only | `packages/base/graphics.zip` | `c9a305e20cf78d684d4e6d30caa5cbe290296edc92917257119aa58be5b9209c` |
| `himemx` | 3.38 | General Public License, Artistic license and Public Domain | (GPL-1.0-or-later OR Artistic-1.0) AND public domain | `packages/base/himemx.zip` | `5be13efe2c86e784c546c2a79d7f1811d512d90d99a779c9ea204b4f4829615b` |
| `htmlhelp` | 1.1.0 | Express+GNU Free Documentation License | GFDL-1.1-or-later AND Express Software Public License 1.0 (viewer; MPL-1.1-derived copyleft, source in the package) | `packages/base/htmlhelp.zip` | `e420ca8223bfaf8ae2e64a8f17d582b0ee3f9c32b361bb8b25065b7a95735a3d` |
| `jemm` | 5.85 | Artistic License | Artistic-1.0 | `packages/base/jemm.zip` | `c571724c2025cc493ba3b194ec68d29a2a9913a5e9b844828d3cdb4c21548994` |
| `kernel` | 2043 | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/kernel.zip` | `38ce3c63e399c8f18ab6230d8988a5d1a1aa9be4e109d15c6f4842b5e8fe61e6` |
| `keyb` | 2.11 | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/keyb.zip` | `9fd172421cfd6f30bf45cd9e46e45c78e40439209104c4e81498e327904e759e` |
| `keyb_lay` | 3.1a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/keyb_lay.zip` | `c20e750bd97eb85a1210500ab7f756f38a2070359eb53791a6a5979070f30bf6` |
| `label` | 1.5 | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/label.zip` | `40713091ecc0de5d3f053876e6b6e61ba5cd22e4276ad1dd7581108d5d136d90` |
| `lbacache` | 2009feb06 | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/lbacache.zip` | `b688c237ed99f71b0f23ded2abe2f63278456461e5bc9c6e3e42e465481b7a17` |
| `mem` | 1.12 | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/mem.zip` | `05608dc39dad9e89f4b783aae710b2ebeedbf0ffa5453d45d9484445fd752799` |
| `mirror` | 0.2a | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/mirror.zip` | `728f2fb4410c3029ecfe374817bc1961589b53ddafd5aeb53259bc455dd22421` |
| `mkeyb` | 0.52 (fork) | GNU General Public License, with exceptions | GPL-2.0-or-later (GPL with an exe-packer exception; DOS-C files 2 or later) | `packages/base/mkeyb.zip` | `872b427c818b9fe286c0019aa49c3733c53aabeccb30f88cdc8e3f1eae6d139b` |
| `mode` | 2015-11-25a | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/mode.zip` | `b4df1f5616afab1a7bf8774a1331914677fa7bd2ac3fd8d9dcde461c1601c492` |
| `more` | 4.3a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/more.zip` | `6c2861a0a4fdbbca726e677cb45cc7761d4ba7f6926c3e7c6903e03723ef5869` |
| `move` | 3.5 | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/move.zip` | `0aeca7a82f1ea367e779ff736526316bf11c5cc7374e94c5de49bd72983e25d4` |
| `nansi` | 4.0d (2007may26) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/nansi.zip` | `be9e1179efe1ce5c4473d0a6f7d917566fdb7f8d60b63a846521ce32d14b2cf6` |
| `nlsfunc` | 0.4a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/nlsfunc.zip` | `e8abc552205408422008d3c9b775e87e945f09f2717779d0a62d84ede59061aa` |
| `print` | 1.02.ea (rev A) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/print.zip` | `ab10f50918c6936517044d0afd3cee0ba1a2676c0ffe0c6658a25a7f32c046db` |
| `recover` | 0.1 (BETA) (rev A) | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/recover.zip` | `9c980817629463d865cee65f0fb22a9ac575da4049ec224ed026d428c4e09578` |
| `replace` | 1.2a | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/replace.zip` | `3eb27db2fa1da396502f367aa4a35bcb54b6d5dabb341b5895fd20a4db365f5e` |
| `share` | 08/2006a | GNU General Public License | GPL-1.0-or-later | `packages/base/share.zip` | `be39883e7dda38f9cafcb9fc2bf37647d8adea1605838417bbea3e9efd979fc7` |
| `shsucdx` | 3.09 | (none) | freeware with source ("Freeware.") | `packages/base/shsucdx.zip` | `411eacf4f4b64cace0031462004f6018352de5de3509698734a2cb0f967de05d` |
| `sort` | 1.5.1a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/sort.zip` | `3f99a877e86a37a627bfa95906e9774447b0eb43a7174992723f37c0eea40aaf` |
| `swsubst` | 3.2a | GNU General Public License, Version 2 | GPL-1.0-or-later | `packages/base/swsubst.zip` | `aaa196da6a88ee8e2aa4de0ed7696d4ebc6f209eb5fcc9b9b8f035f8c26f9192` |
| `tree` | 3.7.3 | GPL (using public domain findfile by Jeremy Davis and optional use of LGPL catgets by Jim Hall) | GPL-2.0-only AND LGPL-2.1-or-later AND public domain (findfile) | `packages/base/tree.zip` | `bb56442d17ec06b75ad15d99ca77dad879f9ad06539f3a3d9653902d8ba09da5` |
| `undelete` | 2008a | GNU General Public License, Version 2 | GPL-2.0-or-later | `packages/base/undelete.zip` | `9e02ad4b204c832d4a142036601757ae741689c3de5a919c2070fccf1c43b80f` |
| `unformat` | 0.8a | GNU General Public License | GPL-1.0-or-later | `packages/base/unformat.zip` | `46fefe1b3c37c1b4ce84e300be06cc7e25035c1578ea5c5fc836b7e1c04c5f22` |
| `xcopy` | 1.8b | GNU General Public License, Version 2 | GPL-2.0-only | `packages/base/xcopy.zip` | `aa8ab3742dd0dd1b31ff991b3ed9bdf65f4f11f0da2c6a22b321c3d1e58f995b` |
| `shsufdrv` | 1.02a | Freeware / source code available | freeware with source ("Freeware."); ZLIBCDRD.LIB: Zlib | `packages/drivers/shsufdrv.zip` | `adb35e7f33683f787b8f1da32e964c75195dbf74d088c5c2197d1a7738233a94` |
| `udvd2` | 2022-12-12 | Free w/sources | free with sources (LSM) | `packages/drivers/udvd2.zip` | `c90d6611bb32e3dd3b7362db2b0a680665c6efcb9d0490dd2bbb8cb187fbe3a7` |
| `fdnet` | 2025-03-17 | Various open source licenses, see LICENSE file | GPL-1.0-or-later AND GPL-3.0-or-later (DHCP, mTCP) AND GPL-1.0-only (Crynwr drivers) AND public domain | `packages/net/fdnet.zip` | `fa4d04aab2977c23845d5db779af068a2d2dde6789e0b4fdf34cd042ec8b5d2f` |
| `fdnpkg` | 0.99.7a | MIT | MIT (also FDINST) | `packages/net/fdnpkg.zip` | `38aeb7f2154cf4a159d7249e1704316922efedb112479dcab39fb85594a548a1` |
| `mtcp` | 2024-10-20 (FreeDOS) | GNU General Public License, Version 3 | GPL-3.0-or-later | `packages/net/mtcp.zip` | `79439cba08ae1e44339c0c97a9247494dbe6173d3c5b3397dc68b6354dbf4827` |
| `callver` | 2007-08-19a | Public Domain | public domain | `packages/tools/callver.zip` | `327d2a0ade12911cbdf4a42e9d700789e6847dfbe9732b5b57ba2ea6a5d86ea3` |
| `cwsdpmi` | 7a | GNU General Public License, Version 2 | GPL-2.0-only | `packages/tools/cwsdpmi.zip` | `4c0d6aee9b14609f2bb9b620d2245ec50c5c866a018860beb7e99d03ccad2219` |
| `pkgtools` | 2021-12-31 | Mozilla Public License Version 2.0 | MPL-2.0 | `packages/tools/pkgtools.zip` | `1ae990094f184fa1726642d6d578e2e68423ed896e75bb68e452563b5a89b335` |
| `v8power` | 24.09.30 | GNU General Public License, Version 2 | GPL (any) OR BSD-3-Clause OR MIT; MIT used | `packages/tools/v8power.zip` | `f1378905558e07c9379bd0461043d869880440cc7a20429ce4472e12d36ac7e0` |
| `fdbanner` | 1.1 | MIT License | MIT | `packages/util/fdbanner.zip` | `51d4ae13a3e1bb06eb05cc8b3e9ce8406349eebe806fd68fd9397cf10e2591c2` |

The licence column is each package's LSM `Copying-policy`; the SPDX column is
read from the program's own notices, the source headers of its main program
and its licence files, and from the LSM only where those name no version
(fc, fdxms, fdxms286). Where a package's notices differ, the narrowest grant
is listed (FDISK: README "version 2 or later", `MAIN.C` "Version 2"). A GPL
notice without a version reads GPL-1.0-or-later. The LGPL-2.0-or-later IO95
and SUPPL libraries inside diskcopy, find and freecom are redistributed under
LGPL-2.1-or-later, which their grant allows. `LICENSES.TXT` gives every
program's licence with the notice it rests on, and quotes the terms
`LicenseRef-FreeDOS-other` names: the public-domain parts, the freeware
SHSUCDX and SHSUFDRV ("Freeware."), UDVD2 ("Free w/sources") and the HTML Help
viewer's Express Software Public License 1.0, an MPL-1.1-derived copyleft
whose source is in its package. The conjunction is the statement every layer
of the sets carries (256 bytes, the statement limit):
`GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND
GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND
Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND
LicenseRef-FreeDOS-other`. A test checks that the code states the same
conjunction as `floppy-1.4-licenses.txt`.

**Two parts read closely.**

- **cpidos** (GPL-2.0-or-later, its README: "version 2.0 or later"): its 18
  code-page fonts ship as `CPI/*.CPX`. `SOURCE/CPIDOS/SOURCES.TXT` says they
  are "UPX-compressed CPI files", to be uncompressed and then "modified using
  free utilities created by Kosta Kostis": the CPI file is the form the
  author edits, and each CPX carries it losslessly, as a compressed archive
  carries a source. **MEASURED (read-only):** every one of the 18 files
  starts with a DOS COM stub (`81 fc`) and holds the `UPX!` header at offset
  0x23-0x24 and the CPI `FONT` header at 0x3C-0x3D. `upx -d` restores the CPI
  byte for byte; the script runs nothing on the files.
- **ZLIBCDRD.LIB**, the zlib 1.2.3 library SHSURDRV (in shsufdrv, freeware)
  links, modified for it: the zlib licence requires no source, only that an
  altered version be plainly marked, which `DOC/SHSUFDRV/ZLIBCDRD.TXT` does
  with a diff against zlib 1.2.3. `Zlib` is in the conjunction. SHSURDRV's
  own source is in the package.

## Usage

```sh
vendor/freedos/fetch.sh             # builds all three images
vendor/freedos/fetch-floppy-1.4.sh  # the FreeDOS 1.4 Floppy Edition sets (separate)
```

Requires `git`, `nasm`, `unzip`, `curl`, and mtools (`mformat`/`mcopy`/`mdir`/`mlabel`) —
all in the flake devShell. SHA-256 uses `sha256sum` or macOS's `shasum -a 256`;
size and boot-signature checks also use portable utilities. The images, archives
and source clone are gitignored. Once `build/ke2044_86f16.zip`,
`build/command.zip` and the pinned commit in `build/kernel-src/` are cached,
rebuilding needs no network. Cached archives are still hash-checked; missing
archives or commits are fetched as before.

For a tools-only offline build, first provide the already pinned
`freedos-1200.img` and a directory containing `ke2044_86f16.zip` plus
`fdisk.zip`, `format.zip`, `xcopy.zip`, `edit.zip`, `mem.zip`, `ctmouse.zip`:

```sh
FREEDOS_PACKAGE_DIR=/path/to/cached-packages vendor/freedos/fetch.sh
```

When set, this variable disables downloads and source-clone operations and
builds only `freedos-tools-1200.img`. It checks the boot fixture and every
archive before copying any tools. Missing inputs fail; it never silently
falls back to the network. Without the variable, the original source build
runs first, followed by the tools build. All three outputs retain the same
FAT timestamp rules and fixed per-volume serials.
