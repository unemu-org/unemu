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

The script reuses `build/ide_at.bin` and `build/ide_xtp.bin` when present,
so a cached build requires no network. Otherwise it fetches the pinned URLs
with `curl`. It checks both upstream SHA-256 hashes on every build.

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
Only `fetch.sh` and this README are committed in this directory. The cache,
temporary files and output images are gitignored.

## XT+ configured for 16-bit ATA and floppy boot

The second input is r638 `ide_xtp.bin`, 6,629 bytes, from
<https://www.xtideuniversalbios.org/binaries/r638/ide_xtp.bin>, SHA-256:

```text
338f70795a7c881a839ce25365678db0f2a356f2f255b7493a07f58f3ac65ec4
```

The binary download centre's GNU GPL version 2 statement above applies to
both builds. The configured image retains the conservative `GPL-2.0-only`
catalogue licence. No firmware or cached source bytes are committed.

Source references below are relative to the SVN r638 export root, cached
under `build/src/`. To obtain it, run from `vendor/xtide-universal-bios/`:

```sh
mkdir -p build
svn export -r 638 https://www.xtideuniversalbios.org/svn/xtideuniversalbios/trunk build/src
```

Subversion is available through `nix shell nixpkgs#subversion`. The export is
optional, only for reading these citations; `fetch.sh` does not require it.
The configuration is derived from that revision:

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
