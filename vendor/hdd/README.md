# vendor/hdd — reproducible FAT16 hard-disk data fixture

`build.sh` builds `fat16.img` entirely offline for the AT IDE disk fixture in
[`firmware-and-machine-classes.md` §9 item 10](../../backlog/docs/design/firmware-and-machine-classes.md).
It is a data disk for a later FreeDOS floppy-boot test that lists `C:`. It has
no operating system, its partition is inactive, and its volume sector only
rejects boot attempts. The MBR code area remains zero: nothing boots from this disk.
The generated image and temporary `build/` directory are gitignored.

## Usage and reproducibility

```sh
vendor/hdd/build.sh
mdir -i vendor/hdd/fat16.img@@8704 ::
```

Requires Bash, mtools (`mformat`, `mcopy`, `mdir`) with `SOURCE_DATE_EPOCH`
support, standard utilities (`dd`, `printf`, `od`, `awk`, `tr`, `wc`, `mktemp`,
`rm`, `mv`), and either `sha256sum` or `shasum -a 256`. The project devShell
provides mtools 4.0.49, the version used for the pinned image. No downloads,
mounts, privileges or partitioning tools are needed.

SHA-256 of the complete 21,411,840-byte image:

```text
e97f1eb42c1dcef1a2fbf9541135b22a955704836088c46bd280d27780e9ba2d
```

The script checks this hash before moving its temporary image into place; a
mismatch reports expected and actual hashes and leaves any existing output
untouched. Temporary files are removed on exit. A new image starts with zero
bytes throughout, so unused sectors and file slack are deterministic.

The volume label is `UNEMUHDD`, and the serial is `2895-9854` (`0x28959854`).
`SOURCE_DATE_EPOCH=315532800` and `TZ=UTC` fix volume-label and file creation,
access and modification times at 1980-01-01 00:00:00 UTC. `mcopy` omits `-m`,
so host file timestamps do not enter the image. The script ignores per-user
mtools configuration and clears timestamp/FAT-layout overrides. A different
toolchain must reproduce the pinned hash; changing it requires reviewing the
image deliberately.

## Disk and partition layout

Geometry is IBM AT fixed-disk **type 2**: 615 cylinders, 4 heads, 17 sectors
per track, 512 bytes per sector (41,820 sectors). Source: *IBM Personal
Computer AT Technical Reference*, BIOS listing, fixed-disk parameter table,
type 2 entry. It is within the bounds accepted by `AtaGeometry` in
`crates/devices/src/ata.rs` (1–16383 cylinders, 1–16 heads, 1–63 sectors).

The script writes the MBR bytes directly with `printf` and `dd` rather than
letting `mpartition` choose a layout or boot code. CHS uses zero-based cylinders
and heads and one-based sectors: `LBA = (cylinder * 4 + head) * 17 + sector - 1`.

| Absolute byte offset | Length | Contents |
|---|---|---|
| `0x000` | 446 | Zero MBR boot-code/disk-signature area |
| `0x1BE` | 16 | Primary partition: `00 01 01 00 04 03 91 66 11 00 00 00 4B A3 00 00` |
| `0x1CE` | 48 | Three unused, zero partition entries |
| `0x1FE` | 2 | MBR signature `55 AA` |
| `0x200` | 8192 | Unused remainder of the first track, zero |
| `0x2200` (8704, LBA 17) | 21,403,136 | FAT16 partition, 41,803 sectors, through LBA 41,819 |

Partition entry fields are inactive (`00h`), start CHS **0/1/1**, type **04h**
(FAT16 below 32 MiB), end CHS **614/3/17**, start LBA **17**, and length
**41,803** sectors. End CHS encodes as `03 91 66`: cylinder `0x266` contributes
its high bits to the sector byte. Both CHS endpoints agree with the LBA fields.

## FAT16 volume layout

BPB offsets below are relative to the partition at `0x2200`; multi-byte values
are little-endian. The volume sector is a project-written, zero-filled template
with jump bytes `EB 3C 90`, OEM text `UNEMUPC `, extended-BPB flags, a minimal
"not bootable" stub and signature; mformat fills the BPB and preserves our jump
and stub. The jump reaches the stub at `3Eh`, which calls `INT 18h` (`CD 18`),
then loops on `HLT` (`F4 EB FD`) if the interrupt returns. The remaining code
area stays zero, with no mtools boot stub embedded.

Sources: *Microsoft FAT32 File System Specification* (fatgen103), "Boot Sector
and BPB", `BS_jmpBoot`; *IBM Personal Computer AT Technical Reference*, BIOS
listing, INT 18h (no bootable disk).

| Relative offset | Absolute offset | Field | Value |
|---|---|---|---|
| `0x00` | `0x2200` | Jump to the stub at `0x3E` | `EB 3C 90` |
| `0x0B` | `0x220B` | Bytes per sector | 512 |
| `0x0D` | `0x220D` | Sectors per cluster | 4 (2048 bytes) |
| `0x0E` | `0x220E` | Reserved sectors | 1 |
| `0x10` | `0x2210` | FAT copies | 2 |
| `0x11` | `0x2211` | Root directory entries | 512 (32 sectors) |
| `0x13` | `0x2213` | 16-bit total sectors | 41,803 |
| `0x15` | `0x2215` | Media descriptor | `F8h` (fixed disk) |
| `0x16` | `0x2216` | Sectors per FAT | 41 |
| `0x18` | `0x2218` | Sectors per track | 17 |
| `0x1A` | `0x221A` | Heads | 4 |
| `0x1C` | `0x221C` | Hidden sectors | 17 |
| `0x20` | `0x2220` | 32-bit total sectors | 0 (16-bit field used) |
| `0x24` | `0x2224` | BIOS drive number | `80h` |
| `0x26` | `0x2226` | Extended BPB signature | `29h` |
| `0x27` | `0x2227` | Volume serial | `0x28959854` |
| `0x2B` | `0x222B` | Volume label (11 bytes) | `UNEMUHDD   ` |
| `0x36` | `0x2236` | Filesystem type (8 bytes) | `FAT16   ` |
| `0x3E` | `0x223E` | Not-bootable stub (5 bytes) | `CD 18 F4 EB FD` |
| `0x43` | `0x2243` | Remaining code area (443 bytes) | Zero |
| `0x1FE` | `0x23FE` | Volume signature | `55 AA` |

The two FATs start at absolute LBAs 18 and 59. The root directory starts at
LBA 100; data starts at LBA 132. The data-cluster count is
`floor((41803 - 1 - 2 * 41 - 32) / 4) = 10422`, in the FAT16 range
4085–65524, which the script asserts by reading the generated BPB. Every data
sector belongs to a complete cluster. Each FAT has room for all data-cluster
entries plus the two reserved entries.

The only root file is **`MARKER.TXT`**, 29 bytes, with exactly this ASCII content
followed by CRLF (`0D 0A`):

```text
unemupc98 hard disk fixture
```

## License

The script and fixture contents are **AGPL-3.0-or-later**, like the project.
The image contains only project-written marker/template bytes, zero padding,
and generated MBR/FAT16 metadata. It contains no firmware, FreeDOS files or
third-party executable boot code. mtools is a build-time tool.
