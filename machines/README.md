# Machine descriptions

`at-freedos.toml` and its equivalent `at-freedos.yaml` describe the current
IBM PC/AT-class FreeDOS machine:
Xi8088 open BIOS, MDA and VGA text cards, an unwired COM1, and a 1.44 MB
floppy on the µPD765 controller. Its pinned images are fetched with
`vendor/kiselev-bios/fetch.sh` and `vendor/freedos/fetch.sh`.

`at-ide.toml` extends `at-freedos.toml`, selecting the open AT BIOS
(`vendor/unemu-at-bios/bios-unemu-at.bin`, the unemu fork of the Kiselev BIOS;
[fixture notes](../vendor/unemu-at-bios/README.md)) and the pinned XTIDE
Universal BIOS r638 XT+ option ROM configured for 16-bit ATA polling at C8000h. A primary IDE channel uses
1F0h/3F6h and IRQ14.
Its master is `vendor/hdd/fat16.img`, geometry 615/4/17, containing the
UNEMUHDD FAT16 volume and 29-byte `MARKER.TXT`. The disk is readonly so
the directory-listing test cannot depend on accepted writes.
The open AT BIOS accepts XUB's AH=00h, DL=80h reset request using IBM AT
semantics; the original BIOS and floppy profile keep their existing bytes. It
reads the display switch with the IBM polarity, so `at-ide.toml` sets
`display_switch = "mono"` to keep the inherited MDA as the console (the
Kiselev profiles set `colour`, which that BIOS reads the other way).
The ignored `ide_freedos_golden` boots A:, types `DIR C:` through the keyboard
and checks the completed listing, prompt instruction count, screen hashes
and POST sequence. Run it with:

```sh
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test ide_freedos_golden -- --ignored --nocapture
```

Fetch the Kiselev BIOS images with `vendor/kiselev-bios/fetch.sh`, the open AT
BIOS with `vendor/unemu-at-bios/fetch.sh`, the option ROMs with
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

Start the desktop library from the repository root with:

```sh
cargo run --offline -p window --bin unemupc
```

The welcome screen lists machine files in the library's machines directory
and its `local/` subdirectory, and selects `at-freedos.toml` there initially
when that file exists. The directory is `UNEMUPC_MACHINES_DIR` when set
(relative to the working directory); else, under `cargo run`, this tree's
`machines/`; else `unemupc/machines` in the platform data directory:
`~/Library/Application Support/unemupc/machines` on macOS,
`$XDG_DATA_HOME/unemupc/machines` on Linux (default
`~/.local/share/unemupc/machines`), `%LOCALAPPDATA%\unemupc\machines` on
Windows. `unemupc --help` prints the directory in use. Select a ready machine and press Start, Enter, or
double-click its name. Missing firmware or invalid media shows the existing
validator diagnostic and disables Start. File › Open machine file… accepts a
TOML/YAML path. Machine › Settings… (⌘, on macOS; Ctrl+, elsewhere outside guest focus) opens the
classic System, Memory, Display, Sound, Storage, Expansion and Firmware tabs.
Hardware choices and their disabled reasons come from assembly capabilities.
Running and paused machines show a read-only sheet; stop before editing.
Help › Keyboard shortcuts lists all bindings.

The sheet shows effective values after `extends`. Review the resulting TOML
preview before OK or Apply. For a shipped machine, saving creates
`machines/local/<parent>-custom.toml`, which extends the selected machine and
overrides changed settings. Opening Settings on a saved local child edits that
child in place. The sheet shows its current text, a diff and the proposed text;
replacement requires explicit consent. Names retain a single `(custom)` suffix.
Shipped files are never edited. The library selects the child after saving;
Apply leaves the sheet open, and Cancel, Esc or the title-bar close button
cancel unsaved changes.

Unchanged media bindings, full SHA-256 pins and journal policies are preserved.
Rewriting an option-ROM list rebases unchanged image paths relative to the new
child directory while preserving symlink spellings. A firmware selection writes
its catalogue pin and a plain filename; ROM folders resolve the location before
Start. Settings never walks ROM folders or hashes images. Missing firmware/media
can be described without reading their bytes; Start still runs normal image
verification.
New-machine/import and layer editing remain unavailable.

Machines and images of the library choose their firmware on the Firmware tab
of their own Settings… (not a machine file's sheet): a System BIOS and, for a
VGA, a Video BIOS among the firmware catalogue entries that fit the board and
that a firmware package of the local store holds. On a machine the choice
applies from its next start; a system BIOS of another CMOS layout resets the
machine's stored CMOS to the bytes its configuration derives. On an image it
publishes a new revision under a new tag. A superseded release shows "Newer
version available" in the details pane, with Switch to …. See
`backlog/docs/design/machine-images.md` §7.

To start a specific machine immediately in the same shell:

```sh
cargo run --offline -p window --bin unemupc -- --machine machines/at-freedos.toml
```

To locate user-supplied firmware, open **Tools › Locate firmware…** (or
**Tools › Preferences…**). Choose **Browse…** and select a ROM folder, or type
its directory path and select **Add folder**. Add more folders if chip dumps
are stored separately, then select **Save**. The welcome screen checks readiness
again on a host worker for the selected machine, showing “Checking firmware…”;
Start is enabled once firmware, media and the machine configuration pass
validation. Remaining missing-media errors still need the local disks described
below. No ROM files are copied into the repository and no machine file is edited.

Folders persist in `~/Library/Application Support/unemupc/preferences.toml`
on macOS, `$XDG_CONFIG_HOME/unemupc/preferences.toml` on Linux (default
`~/.config/unemupc/preferences.toml`), or the app's config directory on Windows.
When that file does not exist yet, the first start copies it from the
directory named after the project's former name; the old file stays.
`UNEMUPC_USER_ROMS_DIR`, when set, is searched as well, and under `cargo run`
also this tree's `vendor/user-roms`. File names do not select
firmware: full-file SHA-256 pins do, including split chips and assembled ROMs.
Correct existing machine-file paths take priority. Unavailable folders and entries
produce warnings and do not block other folders. Discovery ignores symlinks that
leave a chosen root and uses catalogue lengths or bounded ROM sizes to avoid
hashing unrelated archives. Preferences are saved atomically and preserve unknown
fields written by newer versions.

The desktop keeps a local SHA-256 index at
`~/Library/Caches/unemupc/firmware-index.v2` on macOS,
`$XDG_CACHE_HOME/unemupc/firmware-index.v2` on Linux (default
`~/.cache/unemupc/firmware-index.v2`). Persistence is currently disabled on
Windows; discovery rehashes files there. The index contains the user's
own canonical paths, lengths, timestamps, file identities where available and
hashes, never ROM bytes; it stays local. Fresh metadata validates every hit.
A changed file is hashed again; hashes taken within two seconds of the file's
mtime or change time are not persisted and are rehashed in the next discovery
run, protecting against coarse timestamps. They are reused only within one scan.
A corrupt or unknown-version index is rebuilt. An unchanged index is not rewritten;
unreadable indexes are preserved. Stale temporary index files are swept after five minutes.
Successful directory listings remove missing direct-child entries; unavailable
directories retain their cached children. Hashes for other folders and pin sizes
are preserved, capped at 4096 entries and 16 MiB, evicting unseen entries first.
Cache write failures remain nonfatal.

The fixed-height status message pane and Locate firmware dialog show a segmented
progress bar: listing is indeterminate, then hashing shows checked candidates
(including cache hits) against the total, plus match counts. One listing is shared
by the BIOS, option ROMs and machine checks for the whole desktop session.
Startup scans once; saved folder changes list only added roots and drop removed
listings. Draft edits do not scan until Save, and Cancel leaves the session intact.
Saved changes defer readiness updates until a running machine stops.
Selecting another machine reuses the listing.
Start reuses the checked document and shows **Loading machine…**, without repeating
discovery. The loader still verifies firmware and pinned media. If a located ROM
fails verification, its folder is listed again and discovery retries once.
An unresolved pin or a missing/changed cached candidate also refreshes the affected
cached roots once, retaining the hash index, so moved dumps can be found again.
All found lines are retained; up to 256 skipped lines
are shown, with one summary for size skips. **Check again** refreshes listings of
the saved machine roots while reusing non-racy, metadata-validated hashes;
**Rescan** refreshes listings and ignores cached hashes for that run. Both use
saved folders; select **Save** to apply draft edits. Discovery read counters exclude
the normal loader's firmware and pinned-media verification.

**Cancel scan** stops discovery between files and hash chunks, retaining completed,
non-racy hashes; an in-flight operating-system network call must return first.
The following **Loading machine…** phase reads ROMs and pinned disks through the
normal loader and cannot be cancelled mid-read. A cancellation during that phase
is applied when loading returns. Cancelled checks keep Start disabled until a
normal **Check again** resolves them. Save persists folders and refreshes readiness.
Closing with **Cancel** discards draft folder edits and leaves any saved-folder
job and its session listing intact.
Idle dialogs show no scan status. Discovery and loading run on the host worker
and never advance guest time.

The CLI accepts repeatable ROM directories for both validation and running:

```sh
upcli machine check machines/at-games.toml --rom-dir /path/to/roms
upcli run --machine machines/at-games.toml --rom-dir /path/to/roms --print vga
```

Without `--rom-dir`, the CLI performs no ROM discovery. The
window binary also accepts `--rom-dir`; its saved folders and environment are
included. macOS keeps only the native title bar; toolbar icons are painted in
both skins, and the in-window menus remain available on every platform.

On macOS, host shortcuts use Command: ⌘P pauses/resumes, ⌘K stops cleanly
and returns to the library, and ⌘End sends guest Ctrl+Alt+Del. Command chords
never type guest keys. Ctrl and Alt remain immediate guest input, including
Ctrl+Alt+Del, except the Ctrl shortcuts macOS takes first, such as Ctrl+arrows;
see [macOS system shortcuts in games](#macos-system-shortcuts-in-games). ⌘Q quits, ⌘W closes, ⌘H hides, ⌘M minimizes and ⌘Tab switches
applications; mute uses ⌘⇧M. Hold ⌘T for fast-forward; releasing Command or T
ends the hold. ⌘Enter toggles full screen; ⌘N/⌘O/⌘, select New/Open/Settings.

Elsewhere Pause/Resume is Ctrl+Alt+P; Ctrl+Alt+Q stops and Ctrl+Alt+End sends
guest Ctrl+Alt+Del. Click the screen to focus guest input and capture a
configured mouse; ⌘G on macOS (Ctrl+Alt+G elsewhere) releases it while
preserving keyboard focus, as does Pause. Menus and dialogs release capture
and keyboard focus. Help uses ⌘⇧/ on macOS; elsewhere F1, Ctrl+N, Ctrl+O
and Ctrl+, require shell focus or a stopped machine and otherwise
reach the guest. The recipes below spell Ctrl+Alt host chords for other
platforms: on macOS use ⌘F/⌘E/⌘I for next/eject/insert A: (add Shift for B:),
⌘G for pointer release and ⌘⇧M for mute. Guest Ctrl+Alt+Del stays the same.
See [window commands](../backlog/docs/commands.md) for the complete table. View › Skin selects classic (default) or modern controls; Theme follows
the system by default. Drive activity is currently unavailable and labelled in
the status bar. The maintainer checks appearance manually; tests open no windows.

`--display` selects an effective card id;
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

Run `upcli machine check machines/local.toml` before booting with
`unemupc --machine machines/local.toml`. Unknown development firmware
requires `--allow-unlisted-firmware` on either command. For a headless run, use
`upcli run --machine machines/local.toml --print mda`. Run limits and host
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
mkdir -p "$HOME/unemupc-disks"
upcli media create hd --size 32MiB "$HOME/unemupc-disks/disk.img"
upcli media create floppy --type 1.44M --format --label TRANSFER "$HOME/unemupc-disks/transfer.img"
```

Floppy types are `1.44M`, `1.2M`, `720K` and `360K`. To fill a new floppy with
host files instead, see [Building a floppy from host files](#building-a-floppy-from-host-files).
Without `--format` every byte is zero. With it, the command writes standard FAT12 BPB/FAT/root metadata,
a fixed serial (`0x28959855`) and, by default, `NO NAME` in the BPB. The root stays empty
unless `--label` supplies an uppercase ASCII volume label; its timestamps are
fixed at 1980-01-01. The boot sector silently disables interrupts and halts
in a loop; it contains no DOS loader or system files. Use a bootable DOS floppy
for starting the machine. `--label` requires `--format` (or `--add`), accepts 1–11 ASCII
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

Use the open AT BIOS plus XUB option ROM from `at-ide.toml` for the
16×63 mapping: XUB discovers the geometry through ATA IDENTIFY. An IBM AT
system BIOS without a fixed-disk option ROM derives only IBM types 1–14 and
16–22; arbitrary geometry is rejected. For that path, for example, create
`--chs 615/4/17` (IBM type 2). Geometry must match the image exactly.

Save `local.toml` beside the images. Parents in `extends` must be relative
paths (or presets). To keep the example independent of the disks folder's
location, put a symlink to the repository profile beside it. Replace the example
repository path in this shell command:

```sh
ln -s /path/to/unemupc/machines/at-ide.toml "$HOME/unemupc-disks/at-ide.toml"
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
For the [first DOS installation](#first-dos-install), replace the inherited A: with the open tools
floppy built by `vendor/freedos/fetch.sh`. Add this override to the same local
file; use an absolute path to the generated image in your checkout:

```toml
[[card]]
id = "fdc-765"
[[card.drives]]
unit = 0
type = "1.2M"
image = "/path/to/unemupc/vendor/freedos/freedos-tools-1200.img"
sha256 = "8d91283a1c0d6fc5b5fdb5e40db5b66dd9fabbf014d37262b7892f3b150b6f63"
write = "readonly"
```

The drive type must take the image: a 1.44M drive takes 1.44M and 720K images,
a 1.2M drive 1.2M and 360K images, and 720K and 360K drives only their own
format (see [Removable floppy media](#removable-floppy-media)). The tools floppy includes FDISK,
FORMAT, SYS, XCOPY, EDIT, MEM and CTMOUSE; package versions, licences and CPU
compatibility evidence are in [the fixture README](../vendor/freedos/README.md).
Check the local file with `upcli machine check FILE`, then boot it with the
window frontend's `--machine FILE` option. Configure installation/data disks on
B: before booting, or use the [media-list and swap controls](#removable-floppy-media)
below. The single-image B: override below uses the inherited `fdc-765` card id.

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

### Adding a data floppy

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
List and drive-level single-image fields cannot be combined. Every medium must be
one the drive type takes, by its image size, and entries may differ:

| Drive `type` | Images it takes |
|---|---|
| `1.44M` (3.5-inch) | 1.44M (1,474,560 bytes), 720K (737,280 bytes) |
| `1.2M` (5.25-inch) | 1.2M (1,228,800 bytes), 360K (368,640 bytes; read at 300 kbps, double-stepped) |
| `720K` (3.5-inch) | 720K |
| `360K` (5.25-inch) | 360K |

Any other image fails with a message naming the drive type and the images it
takes. The CMOS drive types follow `type`, not the inserted disk.
Paths belong to the layer that declares them, and `extends` replaces a media
list as a whole; a replacement list clears inherited `empty = true` unless the
child restates it. The inherited A: boot floppy stays unchanged in this example.

In the window, **Ctrl+Alt+F** inserts the next A: image, cycling from the last
back to the first; **Ctrl+Alt+E** ejects A:. Hold **Shift** with either shortcut
to operate B:, as in the example. From empty, next inserts entry zero. These
host keys are applied before the next execution slice and do not reach the
guest. The title shows each inserted file name or `empty`.

The **Media** menu shows the inserted name for each drive, with **Insert…**,
**Next image in catalogue** and **Eject**. **Ctrl+Alt+I** opens Insert for A:
(hold Shift for B:). Absent drives and CD-ROM are disabled with a reason.
The C: summary shows its machine-file write policy; live changed-sector counts
are not exposed by the current frontend boundary.

In **Insert floppy**, select an image from this machine's catalogue or use
**Browse…** to check an image's size and full SHA-256. Images the drive does not
take are disabled with the image format and the drive's accepted formats shown;
**From library…** applies the same rule to every disk of a set. The status-bar
chip shows the inserted disk's own format (`720K` in a 1.44M B:). Existing entries keep the write policy from the machine
file. Browsed images default to read-only; journal mode requires an explicit
separate journal path, defaulting to `<base>.upfj` with a Save picker. Relative
journal paths use the selected base's directory. Retention follows the drive's
existing journal policy, or defaults to ten closed generations. **Insert** applies at the same boundary
as the hotkeys, including while paused; **Cancel** keeps the current medium.
Browsed additions last for the running session; edit the machine file to retain
them across starts. The base file is never rewritten. A failed mount or persistence
operation stops execution and opens an error dialog.
Browse validation errors stay in Insert so the path can be corrected or the
dialog cancelled while retaining the running session.

**Tools › Create blank disk…** creates an unpartitioned fixed disk by exact CHS
or rounded size, or a formatted FAT12 1.2M/1.44M/720K/360K data floppy. For a
missing machine-file disk, use its declared path and geometry (for example the
AT games fixed disk uses `615/4/17`, and its transfer floppy uses `1.2M`). Existing
files are never overwritten; an occupied name asks for a new name. The form
prefills the failing drive's resolved path and geometry. Relative output paths
use the selected machine file's directory. A fixed disk still needs partitioning
and formatting in DOS. Readiness refreshes after creation. Missing-media details offer this
action; firmware errors point to **Locate firmware…**. Skipped firmware-search
entries appear once in Locate firmware, separately from each machine's own errors.

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
cd "$HOME/unemupc-disks"
upcli media generations --base disk.img --journal disk.upfj
upcli media verify --base disk.img --journal disk.upfj
# Use a retained closed generation reported above (1 is an example).
upcli media commit --base disk.img --journal disk.upfj --to 1 --output disk-next.img
upcli media rollback --base disk.img --journal disk.upfj --to 1 --backup disk-before-rollback.upfj
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

### Building a floppy from host files

DOS programs that come as loose files and folders go onto new FAT12 data
floppies with `--add`, without mtools. The command only ever writes new images:

```sh
cd "$HOME/unemupc-disks"
# A folder becomes \GAME on the disk, with its whole tree.
upcli media create floppy --type 1.44M --add /path/to/GAME --label GAME game.img
# Loose files under one root folder, \MYGAME.
upcli media create floppy --type 1.44M --add /path/to/PLAY.EXE --add /path/to/DATA.DAT --top MYGAME mygame.img
# A tree too large for one disk: big-1ofN.img, big-2ofN.img… (N disks)
upcli media create floppy --type 1.44M --add /path/to/BIGGAME --label BIGGAME --split big.img
```

`--add PATH` may repeat; a folder is copied whole as a subdirectory. `--add`
implies `--format`, so `--label` applies. `--top NAME` puts everything added
under one root folder; NAME must already be a valid 8.3 name. A path you name
with `--add` must be a regular file or folder: a symbolic link (name its target
instead), host metadata such as `.DS_Store`, or a device is refused, and so are
two `--add` paths that reach the same host file or folder (`./Game` and `game`
on a case-insensitive file system, say).

The disk holds plain DOS short names, no long-name (VFAT) entries: names are
upper-cased; a Latin letter with diacritics becomes its base letter, the same
whether the host stores the name precomposed or decomposed (`Äbc.txt` becomes
`ABC.TXT`); other non-ASCII characters, characters DOS forbids and spaces become
`_`; a long name keeps eight characters of its stem and three of the extension
after its last period; a name already valid as 8.3 is kept; when two names in a
folder would clash, the later one in sorted order takes `~1`, `~2`…; a DOS device
name such as `CON` gains a `_`. Every changed name is printed as
`renamed HOST -> DOS-PATH` (a change of letter case alone is not a rename).
Inside the folders, `.DS_Store`, `._*`, `Thumbs.db` and `desktop.ini` files,
`__MACOSX` folders, symbolic links (never followed), devices, sockets and pipes,
and the images the command is writing (should they sit inside a source tree) are
left out and printed as `skipped HOST: REASON`.

DOS keeps a drive's current directory in 67 bytes (drive, path and a final NUL),
so a folder's path on the disk may have at most 64 characters, counting the
leading backslash and the `--top` folder: `\GAME\LEVELS\WORLD1` has 19. A deeper
folder is refused by name before anything is written; files inside an accepted
folder always fit.

The image depends only on the files' names and contents: entries are ordered by
the bytes of their host names, every date is 1980-01-01 00:00, the serial is the
formatter's fixed one, and no hidden, system or archive attribute is set. Host
file times and directory order never matter.

The fit is checked before anything is written. It counts whole clusters for
every file and folder and the root folder's fixed entries (224 on 1.44M and 1.2M,
112 on 720K and 360K, one fewer with a label). Without `--split` a misfit is an
error that names the shortfall in bytes and suggests `--split` (or, for 720K and
360K, the drive's larger format), and a file larger than the disk is named.
With `--split` the files go greedily, in sorted tree order, onto
`<stem>-1ofN.<ext>`, `<stem>-2ofN.<ext>`… beside `OUT` (`big.img` gives
`big-1of3.img`…); every disk repeats the folders above its files, and disk `k`
is labelled `LABEL k` (the label shortened to fit eleven characters). When
everything fits on one disk, `--split` writes `OUT` itself with the plain label.
A single file larger than one disk is refused, never cut. Without `--force` an
existing output, any disk of a set included, stops the command before the first
image is written. Every image is built in memory first; should writing a later
disk of a set fail, the error names the disks already written. One line per disk
reports its name, label, file count and free bytes, matching what DIR shows.

To install a split set on C:, insert each disk in B: in turn and copy it with its
paths. The recipe needs everything under one folder: a single `--add` of a
folder gives that, and with several `--add` paths use `--top`. Creating the
folder first keeps XCOPY from asking whether the target is a file or a directory:

```bat
MD C:\BIGGAME
XCOPY B:\BIGGAME C:\BIGGAME /S /E
```

Run the XCOPY line once per disk.

### Copying host files

To put host files into existing FAT media, mtools is optional and runs while the
emulator is stopped. Always create a new working image first; never modify an
immutable base with a journal bound to it:

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

## Lo-tech EMS card reference

```toml
[[card]]
kind = "lotech-ems"
port = 0x260
frame = 0xD000
size_kib = 2048
```

`frame` is required and must match the driver's `/p` switch. It is a segment
(for example 0xD000), not a physical address. The card's jumper default is
E000h, but E0000h–FFFFFh is system-board ROM space on `ibm-5170`: use C000h or
D000h there. IBM 5170 is the only implemented machine-file board; only Rust
bare test rigs accept E000h. `port` defaults to 260h and accepts 260h, 264h,
268h or 26Ch; `size_kib` defaults to 2048 and accepts 512, 1024, 1536 or 2048
(one to four 512 KiB SRAM chips). The 64 KiB frame must not overlap RAM,
another MMIO window or a loaded ROM. All four byte ports and every 400h alias
participate in conflict checking. The card has no IRQ or DMA setting.

The r02 schematic's write-only page latches return FFh on reads. Values
80h–FFh or unpopulated chips leave their 16 KiB windows unbacked. Deterministic
power-on uses FFh latches and zero SRAM; a warm reset preserves both latches
and SRAM. Code can run from the frame: a window whose latch selects a
populated page that no other latch selects is held as CPU RAM, so overlay
managers and EMS-aware programs execute from mapped pages. Unbacked windows
read and execute FFh and drop writes, as on the floating bus; a word that
straddles a mapped and an unbacked window reads FFh for the unbacked byte.
When two latches select the same page, both windows keep the board's coherent
data path but cannot be executed from. One known limit remains: a word operand
that crosses from such a shared window into a neighbouring window that is
mapped or unbacked (for example `MOV AX,[3FFFh]` in the frame with window 0
mapped and windows 1 and 2 on one page), or from another card's memory-mapped
window directly below or above the frame into a mapped or unbacked window,
stops the run with a CPU error.
Hardware coverage is recorded in `crates/devices/src/lotech_ems.rs`.

## Playing DOS games

`at-games.toml` combines the IBM 5170 BIOS dated 1985-06-10, register-level
VGA with the IBM Display Adapter video BIOS, SB16/OPL3, the Lo-tech 2 MiB EMS
board, a Microsoft serial mouse on COM1/IRQ4, and a journaled IDE C:. It
extends `preset:ibm-5170` and removes both default text cards. It is a
configuration for compatibility experiments, not a claim that individual games
or mouse drivers have passed.

Supply the two system BIOS chips and the complete 32 KiB VGA EPROM under the
names in the file; [the ROM guide](../vendor/user-roms/README.md) gives their
identities and layout. The pins cover both chips, the assembled system BIOS,
and the complete video container; its 2000h prefix is skipped when mapping
C0000h. Nothing downloads these ROMs. Unlike test discovery, machine-file paths
use these exact filenames; a local child can override them.

`at-games-open.toml` needs no user ROMs. Fetch/build the open AT BIOS, open VGA
BIOS and XTIDE fixtures with `vendor/unemu-at-bios/fetch.sh`,
`vendor/vgabios/fetch.sh` and `vendor/xtide-universal-bios/fetch.sh`. The open
AT BIOS ([fixture notes](../vendor/unemu-at-bios/README.md)) is the unemu fork
of the Kiselev BIOS with the IBM PC/AT's shutdown-byte resume, INT 15h AH=87h
and AH=88h, measured extended memory, CMOS checks and display-switch
polarity; the colour switch selects the VGA card, as with the IBM BIOS. It
removes both preset text cards and adds the register-level `vga` card with
the open VGA BIOS, the unemu fork of the LGPL VGABIOS built for the 286
([fixture notes](../vendor/vgabios/README.md)), at C0000h–C7FFFh; XTIDE
follows at C8000h. POST, FreeDOS and the fork's INT 10h probe floppy are
measured on this profile, and a mode 0Dh golden pins panned and page-flipped
frames. As with the IBM variant, no individual game is qualified. The IBM
variant needs user ROMs for both system and video firmware.

Both shipped game profiles enable the 2 MiB board at D000h, port 260h. For
another machine, add the `lotech-ems` card above to a local child file. Fetch
the local-only driver using `vendor/ltemm/fetch.sh` (see [its terms and
pins](../vendor/ltemm/README.md)); copy LTEMM.EXE to your own working DOS
disk. Add this line to CONFIG.SYS:

```text
DEVICE=LTEMM.EXE /p:D000 /n
```

Use `DEVICE=C:\LTEMM.EXE /p:D000 /n` if it is installed in C:\ instead. `/n`
skips only the keyboard-polled memory test; the RAM probe still writes port
61h. Match `/p` to the configured frame, and pass `/i:264` when using port
264h. Reboot and run `MEM /E` from the open FreeDOS tools disk to check the
total and free EMS pages. Never put the driver into a committed or published
disk image. Extended RAM and XMS are separate from this board's expanded RAM.

Both profiles use 640 KiB conventional plus **1024 KiB extended RAM**, derived
CMOS, a fixed RTC start of 2000-01-01, the preset's instruction/PIT ratio, and
an empty 80287 socket. The 1 MiB extended-memory default shortens the POST memory
test. To raise it, set `memory.extended_kib` in a local child profile; allowed
values are 0–15232 KiB in steps of 64 KiB.
The I286 model supports LMSW entry, 16-bit protected execution and task switches;
the pinned IBM POST tests all 640 KiB + 1 MiB, and the open AT BIOS measures
the extended memory at POST. Both BIOSes pass the open protected-mode test
floppy's INT 15h block move and reset checks
([at-pm-test](../backlog/docs/design/at-pm-test.md)). No memory manager is
supplied; XMS and DOS-extender use are not yet qualified. This CPU is an
80286, with no 386 instruction set.

A: is **1.2M**, with the minimal FreeDOS boot fixture, the `FDTOOLS`
installation floppy, then a local transfer floppy; it also takes 360K disks.
In `at-games-open.toml`, B: is an empty **3.5-inch 1.44M** drive: insert 1.44M
or 720K game disks with **Media › Insert…** (Shift with the hotkeys), or list
them under unit 1 in a local child profile. `at-games.toml` keeps A: only: the
June 1985 BIOS has no 1.44M drive type, and whether it has a 720K one is not
verified from the IBM listing this project has (its diskette module is not
included), so no B: is configured there.

From the repository root, prepare the ignored local media and fetch the
FreeDOS fixtures:

```sh
vendor/freedos/fetch.sh
mkdir -p machines/local
upcli media create hd --chs 615/4/17 machines/local/games.img
upcli media create floppy --type 1.2M --format --label TRANSFER machines/local/transfer-1200.img
upcli machine check machines/at-games.toml
unemupc --machine machines/at-games.toml
# Open firmware, no user ROMs (also run vendor/unemu-at-bios/fetch.sh,
# vendor/vgabios/fetch.sh and vendor/xtide-universal-bios/fetch.sh):
# upcli machine check machines/at-games-open.toml
# unemupc --machine machines/at-games-open.toml
```

The disk is blank, 21,411,840 bytes (about 20.42 MiB), IBM fixed-disk type 2.
`media create hd --size 32MiB` is also available, but produces 66/16/63 geometry;
that needs the XTIDE path and a matching geometry override, as described in
[Preparing disks](#preparing-disks). The IBM profile deliberately uses the exact
615/4/17 mapping its system BIOS supports, with `controller_model = "at-adapter"`.
The open profile uses ATA task-file semantics for XTIDE. Both profiles share
`games.img` and `games.upfj`; run only one at a time.

### macOS system shortcuts in games

macOS takes some keys before any application sees them: Ctrl+←/→ moves to
the next Space, Ctrl+↑ opens Mission Control and Ctrl+↓ shows the front
application's windows; Ctrl+digits switch desktops, Ctrl+F1–F8 move keyboard
focus, Ctrl+Space switches input sources and F11 shows the desktop. Games that
fire with Ctrl and move with the arrows (Duke Nukem II, for example) then
switch Spaces instead. There are two ways to keep these keys in the guest:

- **Preferences › General › Send macOS system shortcuts to the guest while
  it has the keyboard** (off by default). Turning it on asks macOS for
  Accessibility access; allow unemupc (or the terminal that started it) in
  System Settings › Privacy & Security › Accessibility. While a running guest
  has the keyboard, Ctrl with an arrow, Home/End/Page Up/Page Down, a digit,
  Space or a function key, and every function key go to the guest. Command
  chords always stay with macOS, so ⌘Tab, ⌘G and ⌘Q still work. Pause, the
  in-window menus, dialogs and clicking outside the screen give the keys back
  to macOS; the native macOS menu bar does not release the guest's keyboard.
  The keyboard tap exists only while the guest has the keyboard. Without the
  permission the status bar says so once and macOS keeps the shortcuts.
- Turn the colliding shortcuts off in System Settings › Keyboard › Keyboard
  Shortcuts › Mission Control (and Keyboard navigation or Input Sources for
  the others). This needs no permission but applies to every application.

On most Apple keyboards the top row sends brightness and volume until you hold
fn (or turn on "Use F1, F2, etc. keys as standard function keys"); hold fn for
the guest's F1–F12.

### First DOS install

The supplied tools floppy contains FDISK, FORMAT, SYS, XCOPY, EDIT, MEM and
CTMOUSE. See [versions, licences and CPU requirements](../vendor/freedos/README.md).
The transfer image is an empty data disk; populate a new, unmounted copy
before use, then select that copy in a local child profile.

For a single-image A: containing the tools floppy, boot it directly and accept
date/time with two Enters. With the game profiles' media lists, first boot A:,
accept date/time, press **Ctrl+Alt+F once** to select `FDTOOLS`, then press
**Ctrl+Alt+Del in the guest** and accept date/time again. At `A:\>`, run each
command separately, waiting for the prompt:

```text
FDISK /PRIO:100,100 1
FDISK /ACTIVATE:1 1
FDISK /MBR 1
```

`DOC/FDISK/HELP.TXT` documents `/PRIO` for FAT16, the `,100` percentage suffix,
`/ACTIVATE` and `/MBR`. Use a blank guest disk: these commands replace its
partition layout. **First reboot:** press Ctrl+Alt+Del with `FDTOOLS` still
inserted, then accept date/time and run:

```text
FORMAT C: /S /Q /U /V:FREEDOS /Z:seriously
```

`DOC/FORMAT/HELP.TXT` documents `/Q /U` for a fresh disk without a surface
scan, `/S` to run SYS and copy the system files, `/V` for the label and
`/Z:seriously` to suppress the hard-disk confirmation. Wait for
`System transferred.` and the final `A:\>` prompt. **Second reboot:** eject A:
with **Ctrl+Alt+E**, then press Ctrl+Alt+Del to boot C:. Accept date/time to
reach `C:\>`.

The install golden uses guest Ctrl+Alt+Del for both BIOS warm starts. If the
host intercepts Ctrl+Alt+Del, stop and relaunch with the same disk and journal:
the journal replays the partition and filesystem writes. For the second
relaunch, keep A: empty using `machines/local/boot-c.toml`:

```toml
schema = 1
extends = "../at-games-open.toml"

[[card]]
id = "fdc-765"
[[card.drives]]
unit = 0
media = []
empty = true
```

Use `../at-games.toml` as the parent for the IBM variant. The ignored
`freedos_install_golden` measures these exact DOS commands and the two install
warm starts/ejection on the open `at-ide.toml` profile with a blank 32 MiB disk.
The game profiles above use a type-2 disk; the IBM profile's fixed-disk
installation and C: boot remain unqualified by that open-firmware golden.
Its measured IBM boot ends at `A:\>` with a blank disk.

After booting C:, insert the populated transfer disk with Ctrl+Alt+F and copy
its directory tree without a destination-type prompt:

```text
XCOPY A:\*.* C:\GAMES /S /E /I
```

The pinned `DOC/XCOPY/XCOPY.TXT` documents `/I` to assume a directory when
copying multiple files to a destination that does not exist. Use a game's
installer for multi-disk sets.

Copy games and drivers with floppy swaps and DOS COPY/installers, or use
[mtools on a new, unmounted image](#copying-host-files). Never modify a base
already bound to a journal. At the DOS prompt, set the SB16 resources:

```dos
SET BLASTER=A220 I5 D1 H5 T6
```

Select Sound Blaster at 220h/IRQ5/DMA1 (16-bit DMA5 when offered) and
AdLib/Sound Blaster FM for music. SB16 includes OPL3 at 388h–38Bh; no separate
OPL card is needed. The SB16 mixer starts with master, voice and FM at full
level, so no mixer program is needed; at those levels a full-scale digitized
sound peaks at -6 dBFS on the host, one FM voice near -21 dBFS and the PC
speaker at -12 dBFS (its audible swing 12 dB under a full-scale voice). A game
or mixer program that lowers the SB16 mixer lowers the card below that.
Load your compatible mouse driver, such as `CTMOUSE /S1`
(for a CuteMouse version supporting that option) or `MOUSE.COM` configured for
COM1. The tools floppy supplies CTMOUSE 2.1b4; in the maintainer's manual test
on 2026-11-15 it loaded and the mouse worked in FreeDOS EDIT (no automated
golden yet). Change to the installed game's directory on C: and run it.

Click the window to grab the pointer; **Ctrl+Alt+G releases** it (focus loss
also releases). **Ctrl+Alt+M** toggles host mute. **Ctrl+Alt+F** cycles the A:
media list and **Ctrl+Alt+E** ejects it; Shift selects B: (the open profile's
1.44M drive, or one you add to the IBM profile). See
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

The fixture-gated open-profile check uses the real pinned open AT BIOS, open
VGA BIOS, XTIDE and FreeDOS 1.2M images with temporary local media created by
`media create`. The ignored `open_game_profile_boots_freedos` checks the open
AT BIOS sign-on and its 80286 line on the POST screen and pins its FreeDOS A:
prompt's retired count and VGA text hash. Missing open fixtures skip unless
`UNEMUPC_REQUIRE_FIXTURES=1`; this open-firmware boot does not qualify the
IBM fixed-disk BIOS's C: operations.
