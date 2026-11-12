# vendor/freedos — bootable FreeDOS floppy fixtures

`fetch.sh` builds two **bootable FAT12 FreeDOS floppy images** in one run:

- `freedos.img`: 1.44 MB, 1,474,560 bytes (2880 sectors), used by the existing
  open-BIOS boot and DOS session goldens.
- `freedos-1200.img`: 1.2 MB, 1,228,800 bytes (2400 sectors), used by the
  user-supplied IBM 5170 BIOS boot measurement. The 1985-06-10 BIOS supports
  360K, 1.2M and 720K drive types, so this measurement uses a 1.2M drive.

Both are read-only boot fixtures. Images and downloaded sources are not
committed; pinned FreeDOS inputs rebuild them reproducibly, like
`vendor/kiselev-bios` and `vendor/test386`.

## What it produces

Both floppies contain the same files in the root:

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

**MEASURED:** `fetch.sh` reproduces both pinned hashes from the pinned archives and source with mtools 4.0.49, whose version appears in the boot-sector OEM name.

A hash mismatch fails the build and reports both hashes. Check sources and tools;
changing a pinned output hash requires deliberately rebuilding and reviewing the
goldens' expectations.

The volume label is `FREEDOS`, and its serial is fixed at `2895-9853` to match
the DOS session goldens. `SOURCE_DATE_EPOCH=315532800` and `TZ=UTC` fix all mtools
volume-label and file creation, access and modification timestamps at the FAT
epoch, 1980-01-01 00:00:00 UTC. `mcopy` deliberately omits `-m`, so archive and
host file mtimes do not enter the image. Use mtools with `SOURCE_DATE_EPOCH`
support (the devShell provides 4.0.49); an incompatible build fails the image hash
check.

## License

FreeDOS kernel and FreeCOM are **GNU GPL v2 or later**, freely redistributable.
This project only *reads* the image at runtime (a boot fixture); it does not link
or incorporate FreeDOS code. Upstream license text ships inside each archive
(`docs/` in the kernel zip, sources in the FreeCOM zip).

## Usage

```sh
vendor/freedos/fetch.sh          # builds freedos.img and freedos-1200.img
```

Requires `git`, `nasm`, `unzip`, `curl`, and mtools (`mformat`/`mcopy`/`mdir`) —
all in the flake devShell. SHA-256 uses `sha256sum` or macOS's `shasum -a 256`;
size and boot-signature checks also use portable utilities. The images, archives
and source clone are gitignored. Once `build/ke2044_86f16.zip`,
`build/command.zip` and the pinned commit in `build/kernel-src/` are cached,
rebuilding needs no network. Cached archives are still hash-checked; missing
archives or commits are fetched as before.
