# vendor/freedos — bootable FreeDOS 1.44 MB floppy (fixture)

`fetch.sh` builds a **bootable 1.44 MB FAT12 FreeDOS floppy image**
(`freedos.img`, 1,474,560 bytes) used as read-only boot fixture for the
north-star "boot FreeDOS to a prompt" run. Nothing here is committed — the
archives are fetched from pinned FreeDOS releases and the image is rebuilt from
them, exactly like `vendor/kiselev-bios` and `vendor/test386`.

## What it produces

A FAT12 floppy with, in the root:

- **`KERNEL.SYS`** — the FreeDOS kernel, **8086 build** (`ke2044_86f16`, FAT12/16,
  no FAT32). The default installer kernel is 386-only and would fault on our
  real-mode 286-class core; the 8086 build boots in pure real mode without
  A20/HMA.
- **`COMMAND.COM`** — FreeCOM, **8086 build** (`com086`).
- A **FreeDOS FAT12 boot sector**, assembled from the kernel source
  (`boot/boot.asm`, `-dISFAT12`) and installed over the formatted image's boot
  code while preserving the 1.44 MB BPB `mformat` wrote.

No `FDCONFIG.SYS`/`AUTOEXEC.BAT` is placed: with none present the kernel loads
`COMMAND.COM` as the default shell and reaches the prompt on its own.

## Pinned versions (provenance)

| Component | Source | Pin |
|-----------|--------|-----|
| Kernel (8086) `KERNEL.SYS` | `github.com/FDOS/kernel` release | tag `ke2044`, asset `ke2044_86f16.zip` |
| Shell (8086) `COMMAND.COM` | `github.com/FDOS/freecom` release | tag `com086`, asset `command.zip` |
| Boot sector `boot/boot.asm` | `github.com/FDOS/kernel` source | commit `8535ddb7a783f94b3aae844c8fe7f5ccce9037eb` (tag `ke2044`) |

Archive SHA-256 sums are checked in `fetch.sh`; the assembled boot sector is
size-checked (512 bytes) and the final image is checked for size (1,474,560) and
the `0x55AA` signature.

## License

FreeDOS kernel and FreeCOM are **GNU GPL v2 or later**, freely redistributable.
This project only *reads* the image at runtime (a boot fixture); it does not link
or incorporate FreeDOS code. Upstream license text ships inside each archive
(`docs/` in the kernel zip, sources in the FreeCOM zip).

## Usage

```sh
vendor/freedos/fetch.sh          # builds vendor/freedos/freedos.img
```

Requires `git`, `nasm`, `unzip`, `curl`, and mtools (`mformat`/`mcopy`/`mdir`) —
all in the flake devShell. The image and the fetched archives are gitignored.
