# Machine descriptions

`at-freedos.toml` and its equivalent `at-freedos.yaml` describe the current
IBM PC/AT-class FreeDOS machine:
Xi8088 open BIOS, MDA and VGA text cards, an unwired COM1, and a 1.44 MB
floppy on the µPD765 controller. Its pinned images are fetched with
`vendor/kiselev-bios/fetch.sh` and `vendor/freedos/fetch.sh`.

`at-ide.toml` extends `at-freedos.toml`, selecting the separately built
`bios-xi8088-atreset.bin` and the pinned XTIDE Universal BIOS r638 XT+ option
ROM configured for 16-bit ATA polling at C8000h. A primary IDE channel uses
1F0h/3F6h and IRQ14.
Its master is `vendor/hdd/fat16.img`, geometry 615/4/17, containing the
UNEMUHDD FAT16 volume and 29-byte `MARKER.TXT`. The disk is readonly so
the directory-listing test cannot depend on accepted writes.
The compatibility BIOS accepts XUB's AH=00h, DL=80h reset request using IBM
AT semantics; the original BIOS and floppy profile keep their existing bytes.
The ignored `ide_freedos_golden` boots A:, types `DIR C:` through the keyboard
and checks the completed listing, prompt instruction count, screen hashes
and POST sequence. Run it with:

```sh
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test ide_freedos_golden -- --ignored --nocapture
```

Fetch both BIOS images with `vendor/kiselev-bios/fetch.sh`, the option ROMs with
`vendor/xtide-universal-bios/fetch.sh`, and build the HDD with
`vendor/hdd/build.sh`. See
[`firmware-and-machine-classes.md` §9 item 11](../backlog/docs/design/firmware-and-machine-classes.md).

Every path, including `journal`, resolves relative to the layer that declares
it. For example,
`../vendor/freedos/freedos.img` starts in this directory, independently of the
process working directory. The loader canonicalizes the machine file path
for layer resolution: a symlink uses its target file's directory for
images, journals and relative parent files.
Diagnostics keep the path supplied by the caller. The supplied name also
selects the format, even when the symlink target has another extension.
Image errors show both spelled and resolved paths when they differ.
The loader reads each image once, checks its SHA-256 pin
and carries the bytes inline for volatile/readonly images. Journal bases retain
a file path for assembly's alias checks and are read again at mounting; keep the
base unchanged between loading and assembly.

Complete TOML (`.toml`) and YAML (`.yaml` or `.yml`) files load through
`machine_file::load`. YAML requires one document, unique keys and strict
booleans (`true`/`false`); anchors and aliases have bounded parser budgets.
RTC start times must be quoted. Quoting CPU model names, hashes and drive
types is optional.

`at-freedos-from-preset.toml` lowers to the same configuration by extending
`preset:ibm-5170`. The preset in `presets/ibm-5170.toml` is embedded in the
binary and contains board, CPU, memory, firmware services and default cards,
with no image or journal paths. The child supplies its pinned BIOS and floppy.

`extends` also accepts a relative machine-file path, such as
`extends = "at-freedos.toml"`. Each file independently chooses TOML or YAML.
Tables merge field by field, cards by `id` (defaulting to `kind`), and drives
by `unit` or `channel`. Parent order is retained and new cards append. Use
`remove = true` with an inherited key to delete a card or drive. Cycles and
chains longer than eight layers fail. A final completeness check reports
missing fields and their defining files before images are read. The caller's
argument is always a filesystem path; `preset:` is interpreted only in an
`extends` field. Cycle and completeness chains retain the entry spelling and each written
parent name. Per-value origins retain the layer I/O path, including joined
relative components.

Setting an image path also replaces its hash pin: omit `sha256` to remove an
inherited pin, or supply a new pin. A `sha256`-only override re-pins an inherited
image. Mismatches identify the file and line that set the pin. Each machine
file is limited to 1 MiB, including parent files.

For drives, overriding `image` discards the inherited journal and retention
count. An inherited journal write mode falls back to volatile unless the child
restates it; readonly and volatile modes keep their values. A child selecting
journal mode again must provide a new journal path. A child can also turn
journaling off by setting `write = "volatile"` or `write = "readonly"` without
repeating the image: inherited journal options are discarded. Explicit journal
options with either of those modes remain errors.

Floppy and IDE journals are explicit. For example, in a local machine file's
`[[card.drives]]` table, alongside a generated or copied floppy base:

```toml
write = "journal"
journal = "../tmp-golden/ide-media/work.upfj"
keep_generations = 10 # optional, positive; defaults to 10
```

The journal is relative to the canonical directory of the file that sets it,
including through `extends`. Guest writes never modify the base image. Reloading
and assembling the file replays the journal before the first instruction.
`journal` and `keep_generations` are errors with volatile/readonly modes.
Journal paths must be nonempty and must not name a directory or alias any
image or machine-file layer; failures identify the drive's file and line.
Repository examples keep their existing write modes and create no sidecars next
to fixtures. Persistence goldens use temporary copies. IDE supports the same
three modes; NVRAM supports volatile and explicit journal mode. The window and
CLI select these files with `--machine`.

For a complete HDD and floppy journal workflow, including installation,
commit and rollback, see [Preparing disks](#preparing-disks) below.

The schema and
defaults are in
[`machine-files.md`](../backlog/docs/design/machine-files.md).

The window defaults to this workspace's `at-freedos.toml` (located at build time),
independent of its working directory. `--display` selects an effective card id;
otherwise the first display in file order is used. Positional BIOS/floppy
arguments have been removed. For different images, save a local file beside
`at-freedos.toml`, for example:

```toml
schema = 1
extends = "at-freedos.toml"

[firmware.bios]
image = "images/custom-bios.bin" # replaces the inherited BIOS and its pin
# sha256 = "..."               # optionally pin the replacement

[[card]]
id = "fdc-765"                 # merge with the inherited controller
[[card.drives]]
unit = 0
image = "images/work.img"      # replaces drive A and removes its inherited pin
```

Run `unemupc98 machine check machines/local.toml` before booting with
`unemupc98-window --machine machines/local.toml`. Unknown development firmware
requires `--allow-unlisted-firmware` on either command. For a headless run, use
`unemupc98 run --machine machines/local.toml --print mda`. Run limits and host
output selection stay outside the hardware description.

## Preparing disks

Keep generated disks, local machine files and journals **outside the repository**.
A mounted image is an immutable base. Guest writes belong in an overlay;
`media commit` exports a new base and never changes its input. SHA-256 binding
covers the entire base, including large disks. Never edit a mounted base with
host tools or replace a base that still has a journal bound to it.

Create a blank hard disk and a formatted data floppy without external tools:

```sh
mkdir -p "$HOME/unemupc98-disks"
unemupc98 media create hd --size 32MiB "$HOME/unemupc98-disks/disk.img"
unemupc98 media create floppy --type 1.44M --format --label TRANSFER "$HOME/unemupc98-disks/transfer.img"
```

Floppy types are `1.44M`, `1.2M`, `720K` and `360K`. Without `--format` every
byte is zero. With it, the command writes standard FAT12 BPB/FAT/root metadata,
a fixed serial (`0x28959855`) and, by default, `NO NAME` in the BPB. The root stays empty
unless `--label` supplies an uppercase ASCII volume label; its timestamps are
fixed at 1980-01-01. The boot sector silently disables interrupts and halts
in a loop; it contains no DOS loader or system files. Use a bootable DOS floppy
for starting the machine. `--label` requires `--format`, accepts 1–11 ASCII
characters and rejects leading spaces and FAT filename punctuation. `--force`
replaces an existing directory entry atomically; it never opens the old image for writing. Use a new
output name for any base already referenced by a journal.

For HDDs, an integer size defaults to MiB; `MiB` and `GiB` suffixes are explicit
binary units. `--size` rounds **up** to whole cylinders of 16 heads × 63 sectors
× 512 bytes. The command reports the actual size and geometry. The maximum is
1024 cylinders, exactly 504 MiB, the conservative INT 13h CHS limit used by
this command; larger/LBA images are refused even if a different BIOS could
address them. `--chs C/H/S` creates the exact geometry instead (C=1–1024,
H=1–16, S=1–63). Images contain no partition table or filesystem.

Use the AT-reset Kiselev BIOS plus XUB option ROM from `at-ide.toml` for the
16×63 mapping: XUB discovers the geometry through ATA IDENTIFY. An IBM AT
system BIOS without a fixed-disk option ROM derives only IBM types 1–14 and
16–22; arbitrary geometry is rejected. For that path, for example, create
`--chs 615/4/17` (IBM type 2). Geometry must match the image exactly.

Save `local.toml` beside the images. Parents in `extends` must be relative
paths (or presets). To keep the example independent of the disks folder's
location, put a symlink to the repository profile beside it. Replace the example
repository path in this shell command:

```sh
ln -s /path/to/unemupc98/machines/at-ide.toml "$HOME/unemupc98-disks/at-ide.toml"
```

The relative `extends = "at-ide.toml"` below follows that link. The parent's
vendor image paths and its own `extends = "at-freedos.toml"` resolve from the
symlink target's directory in the repository. The child still resolves its
`disk.img` and `disk.upfj` beside `local.toml`:

```toml
schema = 1
extends = "at-ide.toml"

[[card]]
id = "ide"
[[card.drives]]
channel = "master"
image = "disk.img"
geometry = { cylinders = 66, heads = 16, sectors = 63 }
write = "journal"
journal = "disk.upfj"
keep_generations = 10
```

The 32 MiB request above produces 66/16/63 (34,062,336 bytes). The command also
prints a mount snippet with the actual geometry and an absolute image path.
The inherited A: remains the bootable FreeDOS floppy. That fixture contains
only the kernel and shell: supply compatible FDISK, FORMAT and SYS utilities on
B: (run them as `B:\FDISK`, `B:\FORMAT`, `B:\SYS`), or use your own DOS system
floppy. Check the local file with
`unemupc98 machine check FILE`, then boot it with the window frontend's
`--machine FILE` option. No hot swapping is currently exposed: configure an
installation/data floppy as drive B: before booting, or stop and change its
image between sessions. A floppy override uses the inherited `fdc-765` card id,
`unit = 1`, `type = "1.44M"`, `image = "transfer.img"` and a write mode.

The IDE journal lives next to the immutable disk base. Guest writes survive
restarting the emulator, and mounting the same file replays the journal before
execution. Journal directories must already exist. The same write modes and
offline commands apply to floppies and IDE disks; ATA persists full 512-byte
sectors, and offline capacity comes from the complete image length.

Inside DOS, the preparation sequence is `FDISK` (create a primary DOS partition
and mark it active if it will boot), then reboot with the same local machine
file so the journal preserves the partition table. Run `FORMAT C: /S`. If FORMAT
does not support `/S`, use `FORMAT C:` followed by `SYS C:` from a suitable boot/system
floppy. These commands are destructive to the selected guest disk. Check its
reported capacity first. Install or copy files from A: or B:, for example
`COPY B:\*.* C:\` for loose files; use the software's installer for multi-disk
sets and directory trees. Installing system files does not by itself qualify
hard-disk boot support for a particular BIOS/guest combination.

For drive B:, the same journal policy applies. For a local data floppy, use this
complete card override and keep the journal beside its base:

```toml
[[card]]
id = "fdc-765"
[[card.drives]]
unit = 1
type = "1.44M"
image = "transfer.img"
write = "journal"
journal = "transfer.upfj"
keep_generations = 10
```

After installing or copying software, exit DOS cleanly and stop the emulator.
Inspect the HDD's closed generation numbers, export the installed state as a
new base, or roll back with a fresh backup path:

```sh
cd "$HOME/unemupc98-disks"
unemupc98 media generations --base disk.img --journal disk.upfj
unemupc98 media verify --base disk.img --journal disk.upfj
# Use a retained closed generation reported above (1 is an example).
unemupc98 media commit --base disk.img --journal disk.upfj --to 1 --output disk-next.img
unemupc98 media rollback --base disk.img --journal disk.upfj --to 1 --backup disk-before-rollback.upfj
```

Choose an available closed generation from `generations`. Rollback backs up the
journal and discards later history; commit exports a new immutable base without
changing either input. Keep the original base with its original journal. To
adopt `disk-next.img`, override the drive's `image`, optionally set its new
SHA-256 pin, and explicitly select `write = "journal"` with a **new** journal
path. Image overrides discard inherited journal settings. Journal tools preserve
sector state, not filesystem health. Use the same commands with `transfer.img`
and `transfer.upfj` for the floppy; for example, export it to `transfer-next.img`
before using the host-copy example below.

To put host files into FAT media, mtools is optional and runs while the emulator
is stopped. Always create a new working image first; never modify an immutable
base with a journal bound to it:

```sh
cp transfer-next.img transfer-host.img
mcopy -i transfer-host.img file ::
mdir -i transfer-host.img ::
# For an already partitioned disk, use its partition-start LBA times 512.
# Example only: a partition starting at LBA 63 has byte offset 32256.
cp partitioned.img partitioned-host.img
mcopy -i partitioned-host.img@@32256 file ::
```

Do not guess the partition offset or use `@@32256` on an unpartitioned blank
image. Obtain the start LBA from its partition table; the fixture in
`vendor/hdd/README.md` instead starts at LBA 17 (`@@8704`). Adopt the host-edited
copy with a fresh journal path and, optionally, a new SHA-256 pin. Host mtools
operations may use host file timestamps; the built-in formatter itself is
byte-deterministic and has no mtools dependency.
