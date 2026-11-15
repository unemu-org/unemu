# XTIDE Universal BIOS option-ROM fixtures

`fetch.sh` prepares the **r638 AT image and configured XT+ image** from the
[XTIDE Universal BIOS binary download centre](https://www.xtideuniversalbios.org/binaries/r638/).
The AT input is
<https://www.xtideuniversalbios.org/binaries/r638/ide_at.bin> (6,646 bytes),
with SHA-256:

```text
6d5007bca3be0bffeb210969a299b67199368b4845f554b5a00715e2718dfea8
```

The download centre's [Readme.txt](https://www.xtideuniversalbios.org/binaries/Readme.txt)
states exactly:

> The XTIDE Universal BIOS project is run under GNU General Public License version 2.

It also says the binaries are built automatically from each commit and names
the XTIDE Universal BIOS Team as copyright holder (2010,2026). The binary
download statement says **version 2**, without an "or later" grant. Accordingly,
this binary's catalogue entry records `GPL-2.0-only`; it does not infer
`GPL-2.0-or-later` from that statement. The broader source-header licence
survey is recorded separately in
[`firmware-and-machine-classes.md` §2](../../backlog/docs/design/firmware-and-machine-classes.md#22-xub-in-detail-read-svn-trunk-sources-measured-r638-ide_atbin).
The firmware is an external read-only fixture; it is not linked into the emulator.

## Rebuild

From the repository root, with the devShell tools on `PATH`:

```sh
vendor/xtide-universal-bios/fetch.sh
```

The script reuses `build/ide_at.bin`, `build/ide_xtp.bin` and the source
export `build/trunk-r638/` when present, so a cached build requires no
network. Otherwise it fetches the pinned URLs with `curl` and exports the
source with Subversion (see [Source archive](#source-archive)). It checks both
upstream SHA-256 hashes on every build.

The raw AT binary starts with `55 AA 10`: the length byte at offset 2 declares
16 × 512 = 8,192 bytes, but the download has only 6,646 bytes and its byte sum
is B0h. It cannot pass the BIOS option-ROM scan in that form. The script keeps
all downloaded bytes intact, fills offsets 6,646–8,190 with **00h**, and writes
the checksum byte **50h at offset 8,191**, the last byte of the declared image.
Zero padding contributes nothing to the sum and avoids changing any upstream
code or settings. The whole 8 KiB image then sums to 0 modulo 256.

The result is `ide_at-r638-8k.bin` (8,192 bytes), SHA-256:

```text
6409de912eab620ea144b79e723bed478814047dc2766fc19fd6e2e3690662d0
```

The script checks the header, size, sum and pinned output hash before moving
a temporary image into place. A changed cache or build recipe fails its hash
check; investigate the change before deliberately updating any pin.
Only `fetch.sh`, `source-notice.txt` and this README are committed in this
directory. The caches, temporary files, output images and the source archive
are gitignored.

## XT+ configured for 16-bit ATA and floppy boot

The second input is r638 `ide_xtp.bin`, 6,629 bytes, from
<https://www.xtideuniversalbios.org/binaries/r638/ide_xtp.bin>, SHA-256:

```text
338f70795a7c881a839ce25365678db0f2a356f2f255b7493a07f58f3ac65ec4
```

The binary download centre's GNU GPL version 2 statement above applies to
both builds. The configured image retains the conservative `GPL-2.0-only`
catalogue licence. No firmware or cached source bytes are committed.

Source references below are relative to the SVN r638 trunk export that
`fetch.sh` caches under `build/trunk-r638/` (the `trunk/` directory of the
source archive). The configuration is derived from that revision:

- `XTIDE_Universal_BIOS/Makefile:116–120` defines XT+ as XT plus `USE_186`,
  without `USE_AT` or `MODULE_IRQ`.
- `XTIDE_Universal_BIOS/Src/Initialization/FloppyDrive.asm:65–69` checks
  whether INT 40h points into ROM instead of calling an unset handler.
  `Src/Initialization/Interrupts.asm:92–103` then copies the old INT 13h
  offset and segment into INT 40h if needed. This works with the Kiselev
  BIOS; the AT build's INT 40h probe assumes an AT system BIOS installed it.
- `XTIDE_Universal_BIOS/Inc/RomVars.inc:53–74` defines IDEVARS: two-byte
  command base at +0, two-byte control-block base at +2, device at +4,
  IRQ at +5, then two six-byte DRVPARAMS records. IDEVARS is 18 bytes.
  `RomVars.inc:141–175` defines the ROMVARS header; its first IDEVARS
  starts at 004Eh after alignment. `DEVICE_16BIT_ATA` is zero (line 113).
- `RomVars.inc:94–95` defines the primary control-block base as 03F0h.
  `Src/Device/IDE/IdeIO.asm:154–157` adds register index 6, defined in
  `Inc/IdeRegisters.inc:43`, to reach device control at 03F6h.
  `Src/Device/IDE/IdePioBlock.asm:217–219` uses `rep insw` with `USE_186`.
- `XTIDE_Universal_BIOS_Configurator_v2/Src/Menupages/IdeControllerMenu.asm:309–339`
  binds these device, port and IRQ offsets to configuration fields.
  Its `WriteDevice` at lines 548–590 selects standard 16-bit ATA and enables
  IRQ only if `MODULE_IRQ` is present. `Src/Menuitem.asm:162–163,258–265`
  addresses the configuration buffer by ROMVARS offset and stores words
  little-endian. The patch uses the same field representation.

The recipe changes only these ROMVARS bytes; all other build defaults remain:

| Offset | Field | Upstream bytes | Configured bytes |
|---|---|---|---|
| 0049h | `ROMVARS.bBootDrv` | `80` | `00` (boot A:) |
| 004Eh–004Fh | `ideVars0.wBasePort` | `00 03` | `F0 01` |
| 0050h–0051h | `ideVars0.wControlBlockPort` | `00 00` | `F0 03` |
| 0052h | `ideVars0.bDevice` | `0E` (XT-CF PIO8) | `00` (16-bit ATA) |
| 0053h | `ideVars0.bIRQ` | `00` | `00` (unchanged: polling) |

Polling preserves the XT+ build's IRQ default because it has no IRQ module;
the emulated card remains wired to IRQ14. The only additional setting is boot
drive 00h: `Src/Main.asm:169–173` defaults to 80h, and
`Src/Menus/HotkeyBar.asm:429` uses this drive for boot selection. The fixture
has zero MBR code and an inactive partition (`vendor/hdd/build.sh:46–49`),
so this profile must boot from its FreeDOS floppy. Controller count, lite
mode, RAMVARS location, translation, block mode, cache policy and every drive
parameter stay at their upstream defaults.

The declared size is 16 × 512 bytes. The script pads offsets 6,629–8,190
with zero and sets byte 8,191 to **77h**, giving byte sum zero. Output
`ide_xtp-r638-ata1f0-8k.bin` is 8,192 bytes, SHA-256:

```text
54975501e6887c836b8d57e7aad78421e745a9b71e250f94d5d1f9631eab1513
```

## Catalogue and placement

`FirmwareEntry::XUB_R638_AT` identifies `xub-r638-at` as an option ROM for
`ibm-5170`, providing `fixed-disk`, requiring a CPU ≥ 80286 and a primary ATA
channel (`ata@1F0/14`), with suggested address C8000h. It remains `Listed`
until a later boot golden executes it. Option-ROM catalogue enforcement and
ATA assembly are separate work; building this fixture does not enable
hard-disk boot in an existing machine profile.

`FirmwareEntry::XUB_R638_XTP_ATA1F0` identifies `xub-r638-xtp-ata1f0` for
`ibm-5170`, providing `fixed-disk`, requiring CPU ≥ 80186 and `ata@1F0/14`.
`machines/at-ide.toml` selects this configured XT+ image; the AT image remains
available for system BIOSes that already initialize INT 40h.

## Source archive

`fetch.sh` also writes `xtide-universal-bios-r638-src.tar`, the corresponding
source of both downloads and therefore of both images. It is what a published
firmware package carries as its `source` layer: the images are GPL, so they may
be published only together with it. Its root directory,
`xtide-universal-bios-r638-src/`, holds:

- `trunk/`: `svn export -r 638` of
  `https://www.xtideuniversalbios.org/svn/xtideuniversalbios/trunk`
  (repository UUID `143130fb-dee8-9d2b-0d25-209ee59ef9c5`), unchanged.
- `NOTICE`: [`source-notice.txt`](source-notice.txt), unchanged. It names the
  revision, the hashes of both downloads and of both shipped 8 KiB images,
  how and when (2026-11-10 and 2026-11-11) the project derives the images,
  including the checksum rule (the last byte makes the byte sum 0 modulo
  256), the licence and the command that rebuilds the downloads.

The archive's licence is **GPL-2.0-only**. Source files with a licence header
grant "version 2 of the License, or (at your option) any later version", but
61 of the 255 source files in the export (`.asm`, `.inc`, `.cpp`, `.h`, `.pl`)
have no header and fall under `license.txt`, which is version 2, and the
download centre's statement above names version 2. So the archive, like the
binaries' catalogue entries, is version 2 only.

How the source revision was matched to the downloads:

- READ: the downloads sit under `binaries/r638/`, and the download centre's
  `Readme.txt` says its binaries are built automatically from each commit.
  `svn info -r 638` of trunk reports last-changed revision 638, committed
  2026-06-09 09:36:36 UTC. Its `XTIDE_Universal_BIOS/Inc/Revision.inc` is
  `638 `, the revision the ROM's sign-on shows.
- MEASURED on 2026-11-15 with NASM 3.01 and GNU make 4.4.1: in the export's
  `XTIDE_Universal_BIOS/`, `make at xtplus AS=nasm` writes `Build/ide_at.bin`
  (6,646 bytes) and `Build/ide_xtp.bin` (6,629 bytes). Each differs from its
  download only in the 10-byte build date at offset 32h, which NASM's
  `__DATE__` fills. With that macro defined as the commit date,
  `make at xtplus AS="nasm -D__DATE__='\"2026-06-09\"'"`, both outputs have
  the pinned download SHA-256 values, byte for byte. The same command in an
  extracted archive gives the same result.

`fetch.sh` repeats that check on every run: it copies the staged `trunk/`,
runs the same `make` command (NASM and make from the devShell) and compares
both outputs with the pinned download hashes before writing the archive, so
the archive always builds what we ship.

r638 sets no `svn:eol-style`, `svn:keywords` or `svn:externals` property (one
`svn:mime-type` only, MEASURED with `svn proplist -R`), so the export is the
repository's bytes; the script still passes `--native-eol LF` and
`--ignore-externals` so a host's Subversion settings cannot change them. When
`svn` is not on `PATH` (the devShell has none), the script runs it from this
repository's pinned nixpkgs with
`nix shell --no-update-lock-file --inputs-from . nixpkgs#subversion`.

The first run, or any run after `build/trunk-r638/` is removed, needs the
network: it prints `exporting XTIDE Universal BIOS r638 source from …` before
contacting the Subversion server, and Nix may download Subversion. Later runs
reuse the cache and work offline, like the cached downloads. The CI `fixtures`
gate runs this script last, so an unreachable server does not stop the other
fixtures from being built.

The archive is an uncompressed POSIX ustar file written by GNU tar: members
sorted by name, every timestamp set to the r638 commit time, owner and group 0
with no names, and modes 0644 or 0755. Its SHA-256 is pinned in `fetch.sh`;
a changed cache fails it, and the message says to remove `build/trunk-r638/`
and export again:

| Output | SHA-256 |
|---|---|
| `xtide-universal-bios-r638-src.tar` | `7bbd5de269f4e1d52ae30b52531fcda842709b9ae49166ac249f9b37187b91e8` |

MEASURED on 2026-11-15 on aarch64-darwin with GNU tar 1.35: a fresh export
and the cached export give this hash. A Linux run has not been measured.
