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

To journal C: with the fixture profile, create a local child next to
`at-ide.toml`. From the repository root, first create `tmp-golden/ide-media/`.
The root `tmp-golden/` directory is gitignored; journals, backups and exports in
this example stay there so guest writes cannot enter the tracked repository.
Journal parent directories must exist:

```sh
mkdir -p tmp-golden/ide-media
```

```toml
schema = 1
extends = "at-ide.toml"

[[card]]
id = "ide"
[[card.drives]]
channel = "master"
write = "journal"
journal = "../tmp-golden/ide-media/c.upfj"
keep_generations = 10
```

Run the child with `--machine`; software writes to C: survive the next run.
The inherited FAT16 base remains immutable. Close the emulator before offline
operations; use the base image actually named by the drive:

```sh
unemupc98 media generations --base vendor/hdd/fat16.img --journal tmp-golden/ide-media/c.upfj
unemupc98 media verify --base vendor/hdd/fat16.img --journal tmp-golden/ide-media/c.upfj
unemupc98 media rollback --base vendor/hdd/fat16.img --journal tmp-golden/ide-media/c.upfj --to 1 --backup tmp-golden/ide-media/c-before-rollback.upfj
unemupc98 media commit --base vendor/hdd/fat16.img --journal tmp-golden/ide-media/c.upfj --to 1 --output tmp-golden/ide-media/c-new.img
```

Choose an available closed generation from `generations`. Rollback backs up the
journal and discards later history. Commit exports a new immutable base without
changing the old inputs. To use that export, override the drive's `image`, set
its new SHA-256 pin, and explicitly select `write = "journal"` with a **new**
journal path; image overrides discard inherited journal settings. Commands and
recovery semantics match floppies; ATA stores only full 512-byte sectors and
offline capacity comes from image length.

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
