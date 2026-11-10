# XTIDE Universal BIOS AT option-ROM fixture

`fetch.sh` prepares the **r638 AT build**, `ide_at.bin`, from the
[XTIDE Universal BIOS binary download centre](https://www.xtideuniversalbios.org/binaries/r638/).
The pinned download is
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

The script reuses `build/ide_at.bin` when present, so a cached build requires
no network. Otherwise it fetches the pinned URL with `curl`. It checks the
upstream SHA-256 on every build, including cached builds.

The raw binary starts with `55 AA 10`: the length byte at offset 2 declares
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
temporary files and output image are gitignored.

## Catalogue and placement

`FirmwareEntry::XUB_R638_AT` identifies `xub-r638-at` as an option ROM for
`ibm-5170`, providing `fixed-disk`, requiring a CPU ≥ 80286 and a primary ATA
channel (`ata@1F0/14`), with suggested address C8000h. It remains `Listed`
until a later boot golden executes it. Option-ROM catalogue enforcement and
ATA assembly are separate work; building this fixture does not enable
hard-disk boot in an existing machine profile.
