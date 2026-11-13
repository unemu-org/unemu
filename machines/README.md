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
The loader reads each image once and checks its SHA-256 pin. Single-image
volatile/readonly bytes stay inline; catalogues and journal bases retain
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

## Selecting a machine

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

Keep generated disks, local machine files and journals outside the repository
or in the gitignored `machines/local/` directory used by the game profiles.
A mounted image is an immutable base. Guest writes belong in an overlay;
`media commit` exports a new base and never changes its input. SHA-256 binding
covers the entire base, including large disks. Never edit a mounted base with
host tools or replace a base that still has a journal bound to it.

### Creating images and a local profile

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
`--machine FILE` option. Configure installation/data disks on B: before booting,
or use the [media-list and swap controls](#removable-floppy-media) below. The
single-image B: override below uses the inherited `fdc-765` card id.

### Write modes and journals

The IDE journal lives next to the immutable disk base. Guest writes survive
restarting the emulator, and mounting the same file replays the journal before
execution. Journal directories must already exist. The same write modes and
offline commands apply to floppies and IDE disks; ATA persists full 512-byte
sectors, and offline capacity comes from the complete image length.

Drives accept `write = "volatile"`, `"readonly"` or `"journal"`; NVRAM supports
volatile and journal modes. `keep_generations` is positive and defaults to 10.
`journal` and `keep_generations` are errors with volatile/readonly modes. Journal
paths must be nonempty, must not name a directory, and must not alias an image
or machine-file layer; diagnostics identify the drive's file and line.

Overriding a drive's `image` discards its inherited journal and retention count.
An inherited journal write mode falls back to volatile unless restated;
readonly and volatile modes retain their values. Restating journal mode needs
a new journal path. Setting volatile or readonly without repeating the image
also discards inherited journal options; explicit journal options remain errors.

Completed IDE commands are journaled at completion. For an unfinished command,
completed sectors reach the journal only at a checkpoint, session finish, CPU
reset or run error; partial sectors are excluded. Changing a floppy closes only
that floppy's journal.

### Partitioning and formatting

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

### Removable floppy media

A floppy keeps its existing single `image`, `sha256`, `write`, `journal` and
`keep_generations` form, or uses an ordered `media` list with those fields on
each entry. For multiple installation/data disks, replace the single-image B:
override above with this card override in `local.toml`. Supply `install-1.img`
beside that file; `transfer.img` is the formatted image created earlier:

```toml
[[card]]
id = "fdc-765"
[[card.drives]]
unit = 1
type = "1.44M"
media = [
  { image = "install-1.img", write = "readonly" },
  { image = "transfer.img", write = "journal", journal = "transfer.upfj" },
]
# empty = true
```

The first entry is inserted at power-on unless `empty = true`, which also permits
an empty catalogue. A configured drive with `empty = true` starts with its
disk-change line asserted; unconfigured positions and drives starting with a
medium keep the line clear. YAML uses the same fields and a sequence for `media`.
List and drive-level single-image fields cannot be combined. Every medium must have
exactly the drive type's geometry; a 360K disk in a 1.2M drive is not modelled.
Paths belong to the layer that declares them, and `extends` replaces a media
list as a whole; a replacement list clears inherited `empty = true` unless the
child restates it. The inherited A: boot floppy stays unchanged in this example.

In the window, **Ctrl+Alt+F** inserts the next A: image, cycling from the last
back to the first; **Ctrl+Alt+E** ejects A:. Hold **Shift** with either shortcut
to operate B:, as in the example. From empty, next inserts entry zero. These
host keys are applied before the next execution slice and do not reach the
guest. The title shows each inserted file name or `empty`.

Ejected volatile media retain their in-memory writes for the session. Ejecting
journaled media flushes and seals that medium's generation and releases its
lock; insertion reopens and replays it. The base is never rewritten, and a
persistence failure stops execution. The journal API always closes a generation,
even after an insert with no writes; it has no clean no-op close. Dropping it
would use the recovery path instead. Each eject therefore uses a retention slot,
so repeated swaps can prune older rollback states after ten closes by default.
Export important states first or increase each medium's `keep_generations`.
Host tools still require stopping the
emulator and writing a new image, as described below.

Headless embedders and tests use `AssembledMachine::run_floppy_schedule` with
ordered `FloppyEvent { at, controller, unit, action }` inputs. `at` is cumulative
virtual time in instruction equivalents, including halted PIT progress. Halted
caps follow the scheduler's whole-PIT-clock rounding. `change_floppy` applies
one event at an already reached boundary; no host timer enters the device path.

### Exporting and rolling back

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
path. Journal tools preserve
sector state, not filesystem health. Use the same commands with `transfer.img`
and `transfer.upfj` for the floppy; for example, export it to `transfer-next.img`
before using the host-copy example below.

### Copying host files

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

## Playing DOS games

`at-games.toml` combines the IBM 5170 BIOS dated 1985-06-10, register-level
VGA with the IBM Display Adapter video BIOS, SB16/OPL3, a Microsoft serial
mouse on COM1/IRQ4, and a journaled IDE C:. It extends `preset:ibm-5170` and
removes both default text cards. It is a configuration for compatibility
experiments, not a claim that individual games or mouse drivers have passed.

Supply the two system BIOS chips and the complete 32 KiB VGA EPROM under the
names in the file; [the ROM guide](../vendor/user-roms/README.md) gives their
identities and layout. The pins cover both chips, the assembled system BIOS,
and the complete video container; its 2000h prefix is skipped when mapping
C0000h. Nothing downloads these ROMs. Unlike test discovery, machine-file paths
use these exact filenames; a local child can override them.

`at-games-open.toml` needs no user ROMs. Fetch/build the Kiselev AT-reset and
XTIDE fixtures with `vendor/kiselev-bios/fetch.sh` and
`vendor/xtide-universal-bios/fetch.sh`. It keeps the preset's MDA and VGA text
cards and uses the established Kiselev colour-switch workaround; the window
opens MDA, the first display. This is a text console and disk/audio/input
baseline, not a VGA graphics-game substitute. No open VGA video BIOS is
qualified here for the 286. The IBM variant needs user ROMs for both system
and video firmware.

Both profiles use 640 KiB conventional plus **4096 KiB extended RAM**, derived
CMOS, a fixed RTC start of 2000-01-01, the preset's instruction/PIT ratio, and
an empty 80287 socket. This fits the board's 15232 KiB extended-memory limit.
The I286 model supports LMSW entry, 16-bit protected execution and task switches;
the pinned IBM POST tests all 640 KiB + 4 MiB. No memory manager is supplied.
XMS, INT 15h block move and DOS-extender use are not yet qualified. This CPU
is an 80286, with no 386 instruction set.

A: is **1.2M**, starting with the pinned FreeDOS fixture and followed by a local
transfer floppy. B: is omitted: the June 1985 BIOS has no 1.44M drive type
(it supports 360K, 1.2M and 720K). Each image in this A: list must be exactly
1,228,800 bytes; mixed-density media in one drive are not modelled.

From the repository root, prepare the ignored local media and fetch the
FreeDOS fixtures:

```sh
vendor/freedos/fetch.sh
mkdir -p machines/local
unemupc98 media create hd --chs 615/4/17 machines/local/games.img
unemupc98 media create floppy --type 1.2M --format --label TRANSFER machines/local/transfer-1200.img
unemupc98 machine check machines/at-games.toml
unemupc98-window --machine machines/at-games.toml
# Open firmware, text console only:
# unemupc98 machine check machines/at-games-open.toml
# unemupc98-window --machine machines/at-games-open.toml
```

The disk is blank, 21,411,840 bytes (about 20.42 MiB), IBM fixed-disk type 2.
`media create hd --size 32MiB` is also available, but produces 66/16/63 geometry;
that needs the XTIDE path and a matching geometry override, as described in
[Preparing disks](#preparing-disks). The IBM profile deliberately uses the exact
615/4/17 mapping its system BIOS supports, with `controller_model = "at-adapter"`.
The open profile uses ATA task-file semantics for XTIDE. Both profiles share
`games.img` and `games.upfj`; run only one at a time.

Boot from A: and press Enter for the date and time to reach the FreeDOS prompt.
The fixture contains only the 8086 builds of `KERNEL.SYS` and `COMMAND.COM`,
with a boot sector; it has **no FDISK, FORMAT, SYS, mouse driver or installer**.
Supply 286-compatible utilities or your own 1.2M DOS installation floppies.
The formatted transfer image above is an empty data disk, not a boot disk.
Replace its contents before its first use, or extend the A: media list in a
local child file. Relative paths in that child belong to the child's directory.
For example, save `machines/local/play.toml` and place your tools disk beside it:

```toml
schema = 1
extends = "../at-games.toml"

[[card]]
id = "fdc-765"
[[card.drives]]
unit = 0
media = [
  { image = "../../vendor/freedos/freedos-1200.img", write = "readonly" },
  { image = "tools-1200.img", write = "readonly" },
  { image = "transfer-1200.img", write = "readonly" },
]
```

Check and start this child with `--machine machines/local/play.toml`. Swap to
the tools disk after FreeDOS reaches the prompt. Run `FDISK`, create an active
primary DOS partition, reboot with FreeDOS inserted, then run `FORMAT C: /S`
(or `FORMAT C:` and `SYS C:` if your FORMAT has no `/S`). See
[Partitioning and formatting](#partitioning-and-formatting) for the full disk
workflow. These C: operations use the IBM fixed-disk BIOS on the AT adapter
and have not been measured: the IBM profile's only measurement is POST with
a blank type-2 disk, ending at `A:\>`. FDISK, FORMAT, copying files and running
games from C: remain unqualified, as does C: boot for this pairing.

Copy games and drivers with floppy swaps and DOS COPY/installers, or use
[mtools on a new, unmounted image](#copying-host-files). Never modify a base
already bound to a journal. At the DOS prompt, set the SB16 resources:

```dos
SET BLASTER=A220 I5 D1 H5 T6
```

Select Sound Blaster at 220h/IRQ5/DMA1 (16-bit DMA5 when offered) and
AdLib/Sound Blaster FM for music. SB16 includes OPL3 at 388h–38Bh; no separate
OPL card is needed. Load your compatible mouse driver, such as `CTMOUSE /S1`
(for a CuteMouse version supporting that option) or `MOUSE.COM` configured for
COM1. Driver versions and options vary; no driver is bundled or qualified by
this profile. Change to the installed game's directory on C: and run it.

Click the window to grab the pointer; **Ctrl+Alt+G releases** it (focus loss
also releases). **Ctrl+Alt+M** toggles host mute. **Ctrl+Alt+F** cycles the A:
media list and **Ctrl+Alt+E** ejects it; Shift selects B: if you add one. See
[Removable floppy media](#removable-floppy-media) for swap and overlay behavior.

Stop the emulator before using `media generations`, `media verify`,
`media commit` or `media rollback`; follow
[Exporting and rolling back](#exporting-and-rolling-back) with base
`machines/local/games.img` and journal `machines/local/games.upfj`. Commit
exports a new base, preserving the original; adopting it requires a fresh
journal path in a local child. Rollback uses a retained generation and a new
backup path. The full base is SHA-256-bound even without an explicit disk pin.

Compatibility targets are experiments, not measured passes. Suggested 286
starting points are Prince of Persia, Commander Keen and Wolfenstein 3D;
version-specific graphics, CPU, memory and driver requirements still apply.
The maintainer's named **Tyrian 2000** and **Duke Nukem 3D** targets are
**not yet**: both require a 386 or later and cannot run on this 80286 profile.
No memory manager is supplied; XMS, INT 15h block move and DOS-extender use
remain unqualified. A20 and installed extended RAM do not supply a 386.

The synthetic profile smoke covers configuration and `machine check` without
proprietary bytes. It checks C0000h in the original document, then relocates
only the synthetic video ROM to C8000h because an unknown system BIOS cannot
establish the catalogue's video scan. The ignored
`user_rom_at_games_freedos_prompt` exercises the unmodified IBM profile's
C0000h layout with discovered user ROMs, the 1.2M fixture and a temporary blank
HDD. Its stop/counter/hash and POST pins record a measured FreeDOS boot with
both user-supplied IBM ROMs.

The fixture-gated open-profile check uses the real pinned Kiselev AT-reset,
XTIDE and FreeDOS 1.2M images with temporary local media created by `media create`.
The ignored `open_game_profile_boots_freedos` pins its FreeDOS A: prompt's
retired count and MDA text hash. Missing open fixtures skip unless
`UNEMUPC98_REQUIRE_FIXTURES=1`; this open-firmware boot does not qualify the
IBM fixed-disk BIOS's C: operations.
