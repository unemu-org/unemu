# Machine descriptions

`at-freedos.toml` and its equivalent `at-freedos.yaml` describe the current
IBM PC/AT-class FreeDOS machine:
Xi8088 open BIOS, MDA and VGA text cards, an unwired COM1, and a 1.44 MB
floppy on the µPD765 controller. Its pinned images are fetched with
`vendor/kiselev-bios/fetch.sh` and `vendor/freedos/fetch.sh`.

Image paths resolve relative to the file that declares them. For example,
`../vendor/freedos/freedos.img` starts in this directory, independently of the
process working directory. The loader canonicalizes the machine file path
for layer resolution: a symlink uses its target file's directory for both
images and relative parent files.
Diagnostics keep the path supplied by the caller. The supplied name also
selects the format, even when the symlink target has another extension.
Image errors show both spelled and resolved paths when they differ.
The loader reads each image once, checks its SHA-256 pin
and carries the bytes inline into an assembly-validated config.

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

IDE PIO channels and volatile/readonly hard disks are implemented. Journal
attachment is deferred; frontends do not select these files yet. The schema and defaults are in
[`machine-files.md`](../backlog/docs/design/machine-files.md).
