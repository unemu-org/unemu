# Commands

Everyday commands for this project. Keep this file **flat and copy-pasteable** — agents and humans both grep it.

## Local machine images

```sh
cargo run --offline -p cli -- image import machines/at-freedos.toml --store /path/to/store --tag dos-lab:1.0 --create
cargo run --offline -p cli -- image import /path/to/machine.yaml --store /path/to/store --tag dos-lab:recovered --recover medium-0=tip
cargo run --offline -p cli -- image import /path/to/machine.toml --store /path/to/store --tag dos-lab:checkpoint --recover medium-0=generation:1
cargo run --offline -p cli -- image compress dos-lab:1.0 --tag dos-lab:compressed --store /path/to/store
cargo run --offline -p cli -- image ls --store /path/to/store
cargo run --offline -p cli -- image sweep --store /path/to/store
cargo run --offline -p cli -- image run dos-lab:1.0 --store /path/to/store --workspace play --max-steps 1000000
cargo run --offline -p cli -- image run dos-lab:1.0 --store /path/to/store --workspace disposable --new --max-steps 1000000
cargo run --offline -p cli -- image workspace ls --store /path/to/store
cargo run --offline -p cli -- image workspace abandon-publication play --store /path/to/store
cargo run --offline -p cli -- image publish --from-workspace play --store /path/to/store --tag dos-lab:flat --flatten
cargo run --offline -p cli -- image publish --from-workspace play --store /path/to/store --tag dos-lab:1.1
cargo run --offline -p cli -- image workspace discard play --store /path/to/store --yes
cargo run --offline -p cli -- image media import /path/to/disks --title "Installation set" --tag install:1.0 --store /path/to/store --create
cargo run --offline -p cli -- image media import /path/to/disk1.img /path/to/disk2.img --title "Installation set" --tag install:2.0 --label '1=Setup' --extra /path/to/manual.txt:MANUAL.TXT --store /path/to/store
cargo run --offline -p cli -- image media import /path/to/disc.iso --title "Optical set" --tag optical:1.0 --format 1=iso-2048 --store /path/to/store
cargo run --offline -p cli -- image media import vendor/shareware/wolf3d/wolf3d-sw14-1440.img --title "Wolfenstein 3-D: Episode 1 (shareware 1.4)" --tag wolf3d-shareware:1.4 --extra vendor/shareware/wolf3d/extras/VENDOR.DOC:VENDOR.DOC --extra vendor/shareware/wolf3d/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1992 --genre "first-person shooter" --platform DOS --description 'Wolfenstein 3-D, episode 1, Escape from Wolfenstein, version 1.4, the shareware episode of a first-person shooter with texture-mapped VGA graphics by id Software, published by Apogee Software. As B.J. Blazkowicz the player escapes from Castle Wolfenstein through ten floors, one of them secret. It needs a 286 or better. The disk holds the files of the original Apogee shareware archive 1wolf14.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-vendor-v11.01.92 --license-text VENDOR.DOC --store /path/to/store --create
cargo run --offline -p cli -- image media ls --store /path/to/store
cargo run --offline -p cli -- image media show install:1.0 --store /path/to/store
cargo run --offline -p cli -- image push wolf3d-shareware:1.4 --store /path/to/store --dry-run
```

Compression policy (store side). Machine imports, media-set imports and
workspace publication compress payloads of at least 4 KiB with single-threaded
zstd level 3 when the provider declares the `+zstd` type: disk bases, sector
packs, floppies, firmware, and media-set floppy/optical/extra payloads. Metadata,
sealed layer TOML, CMOS and tiny blobs stay raw. Add `--no-compress` to either
import command for raw fixtures. Compressed descriptors carry
`unemu.decoded.digest` and `unemu.decoded.size`; OCI `digest` and `size` name the
compressed bytes, and frames record their content size. Output is deterministic
for the pinned zstd version; a codec update may change blob and image digests,
never machine digests.

Blobs stay compressed in the store, so compression only saves disk. Resolving an
image (import, publication checks, `image compress`, Start, Insert) verifies the
stored SHA-256 and streams the decoded bytes through SHA-256 without writing a
file. A decoded copy in `cache/materialized/<decoded-sha256>` is made only for
media a machine opens by file: a compressed hard-disk base at Start, reused at
later Starts while its SHA-256 still matches, and a media-set medium when it is
inserted. Floppies of machine images, firmware and sector
packs decode in memory, and a layered disk head streams its compressed base and
packs into the one flattened head file. Before decoding, the store must have the
payload's size plus 256 MiB free, or the mount fails with a storage-full error.
`image ls` ends with `materialized:BYTES`, the size of that cache. `image sweep
--store DIR` removes decoded files that no tag or workspace pin reaches, and
stale lock or partial files; it refuses to run while the store is in use, and a
removed file is rebuilt at the next mount. Flattened chain heads stay until the
future GC. No filesystem sparse files are used.

The zstd crates are pinned exactly (`zstd`, `zstd-safe`, `zstd-sys`) and build
the bundled libzstd 1.5.7. Setting `ZSTD_SYS_USE_PKG_CONFIG` in the environment
makes `zstd-sys` link the system libzstd instead, and no Cargo setting can turn
that off; do not set it. The `linked_zstd_is_the_pinned_release` test in
`image-store` and the pinned compressed digest fail if another libzstd is
linked. Media-set compression is limited by declared decoded size: floppies
2880 KiB, optical images 900 MiB, extras 2 GiB; larger payloads stay raw.

`image compress SOURCE --tag TARGET --store DIR` verifies the source and publishes
a new manifest and tag with the declared payloads compressed, keeping the exact
config bytes and machine digest. The source tag and its blobs stay intact; the
destination tag must be absent. It accepts machine images and media sets; a
machine's local import record, with its journal retention, is copied to the new
image digest.
`image ls` adds `stored:BYTES` and `decoded:BYTES` totals, including the manifest
and config, for each complete image closure. No registry transport exists yet;
multi-gigabyte bases must be compressed before any future push.

Media import preserves file-list order. A directory follows symlinks, rejects
non-regular entries, and sorts by filename. Dotfiles are skipped and named in the
preview. Candidates must have an `.img`, `.ima` or `.iso` extension or a known
floppy size; otherwise supply an explicit file list. Every import prints its final
order before copying. Add `--dry-run` to validate and preview without writing blobs
or a tag (a missing store is not created). Label and format indices are one-based. Formats are `360K`, `720K`,
`1.2M`, `1.44M` and `iso-2048`; floppy detection uses exact length, and unknown
sizes require an explicit supported format with matching length. No padding or
resizing occurs. Repeat `--extra PATH[:DOSNAME]` for loose files; DOS names require canonical uppercase
8.3 spelling and cannot be reserved PC device names. Labels and filenames reject
control characters and are limited to 128 characters. All inputs are checked
before streaming any source. Sources stay
unchanged and every copy is streamed with full SHA-256. Existing tags fail before
reading sources; final publication uses compare-and-swap. All descriptors default
to user-supplied. Optical storage does not enable CD-ROM emulation.

Set-level catalogue metadata fills the set's one title record in its metadata
blob ([`design/machine-images.md`](design/machine-images.md) §2.1), the fields
the library's Import installation media… dialog writes plus developers:
`--publisher P`, `--developer D` and `--genre G` may be repeated and keep the
order given; `--year YYYY` is a whole number from 1 to 9999; `--platform` is the
software platform, spelled exactly as one of `DOS`, `Windows 3.x`,
`Windows 9x`, `CP/M`, `MSX`, `ZX Spectrum`, `C64`, `Atari 8-bit`, `Atari ST` or
`other` (not the machine platform `ibm-pc-at`); `--description TEXT` is free
text. Values are trimmed and must not be blank. The year and platform rules are
the dialog's, checked in the shared import both use. Without these flags the metadata, and so the set digest, are unchanged.
The preview and `image media show` print one `publisher`, `developer` or
`genre` line per value and `year`, `platform` and `description` lines for the
fields given. The metadata is generated content: in an open set it is CC0-1.0
like the config. It is part of the set: other metadata is another set digest.
The manifest repeats the record in the OCI image-spec's standard annotations,
which registry web UIs show: `org.opencontainers.image.title`, `.description`
(the description), `.vendor` (the publishers) and `.authors` (the
developers), lists joined with `, ` in the order given. A field not given adds
no annotation, so a set without the flags keeps its digest; there is never a
creation time.

A redistributable set (decision-26 amendment of 2026-11-15, for example a
shareware release fetched under `vendor/shareware/` below: the disk image and
the vendor's terms text in `extras/`) is open only by an explicit statement:
`--open --license LicenseRef-NAME --license-text EXTRA`. `--open` needs both
flags and each flag needs `--open`. The licence must be one SPDX `LicenseRef-`
(the vendor's own terms; `MIT`, `GPL-2.0-only` or any other SPDX List
identifier is refused), and `--license-text` names, by file name, exactly one
of the `--extra` files. Every medium and extra is then marked open under that
licence and linked to the terms text's digest (`unemu.terms`); the generated
config and metadata are open under CC0-1.0. The preview prints
`open LICENCE terms "FILE"`. `image media show` prints `terms "FILE" DIGEST`,
one `layer` line per descriptor (kind, digest, class, licence, linked terms
digest) and a `push` line with the guard's verdict. The push guard lets an
open set into `unemu/`, or onto a registry not marked private, only when every
open medium and extra links the same terms text and that text is an open extra
of the set; a set without the statement stays private. Another local tag that
records one of the set's disks or its terms text as user-supplied (the same
file imported without `--open`, say) keeps the set private, and `image push
--dry-run` names it; delete that tag in the library (Delete tag) or push from
a store without it. Marks are part of the set, so a tag that already names
the set under other marks refuses a new statement: import under a new tag.
Game files and terms texts never enter the repository.

Firmware packages use the same store, with the shared default data root when
`--store` is omitted:

```sh
cargo run --offline -p cli -- image firmware import /path/to/roms --store /path/to/store --create --dry-run
cargo run --offline -p cli -- image firmware import /path/to/even.bin /path/to/odd.bin --store /path/to/store
cargo run --offline -p cli -- image firmware import /path/to/roms --rescan --for ibm-5170-bios-1985-06-10 --extra /path/to/manual.pdf --store /path/to/store
cargo run --offline -p cli -- image firmware import /path/to/roms --for kiselev-xi8088-1.0.2-atreset --source vendor/kiselev-bios/kiselev-bios-a4af458-atreset-src.tar --label open-source --label xi8088 --store /path/to/store
cargo run --offline -p cli -- image firmware import /path/to/rom.bin --unlisted my-option-rom --role option-rom --open --license MIT --built-from "example/option-rom" --revision r7 --store /path/to/store
cargo run --offline -p cli -- image firmware edit firmware/kiselev-xi8088-1.0.2:1 --source vendor/kiselev-bios/kiselev-bios-a4af458-atreset-src.tar --store /path/to/store
cargo run --offline -p cli -- image firmware ls --store /path/to/store
cargo run --offline -p cli -- image firmware show firmware/ibm-5170-bios-1985-06-10:1 --store /path/to/store
```

Firmware import recursively enumerates files in stable name order (32 directory
levels and 10,000 entries maximum; hitting a limit imports no candidates), matches full
SHA-256 and length against the catalogue and verifies reconstructed chip sets.
It reports unmatched/unreadable inputs and incomplete sets with missing sockets;
those inputs are never published. Multiple complete layouts require an explicit
file list containing one layout; ambiguous catalogue identities require `--for`.
Tags are `firmware/<catalogue-id>:1`; a package under the tag with the same ROM
and document bytes reports already imported whatever its metadata (an import
whose own statement would change it is refused and names `image firmware
edit`), while other bytes conflict before copying, with CAS at publication. `--rescan` bypasses reusable persistent ROM hashes. `--dry-run`
previews without writing blobs/tags or creating a store. Extras require
`--for CATALOGUE-ID` or `--unlisted ID`; repeat `--extra PATH[:DOSNAME]` using the media-set attachment
schema and DOS naming rules above, each optionally followed by
`--extra-license SPDX` to mark that document open. Sources remain unchanged.

Every layer carries an SPDX licence next to its redistribution mark; a layer
without a valid licence counts as user-supplied. Rights follow the bytes: a ROM,
chip, document or archive whose SHA-256 and length the catalogue lists as
proprietary (an IBM BIOS chip, say) can never be marked open, whatever its
catalogue id, and `image push` keeps such bytes private in any package,
pulled ones included. A ROM the catalogue lists as open is open under the
catalogue's licence (`--user-supplied` keeps it private). `--unlisted ID
--role ROLE` adds one ROM the catalogue does not know (tag `firmware/ID:1`;
the id may not be a catalogue id); it is user-supplied unless `--open
--license SPDX` states its licence, which is never guessed. A GPL, LGPL, AGPL
or MPL family licence needs an open source archive: `--source ARCHIVE` (zip
or POSIX tar, checked by signature and SHA-256 only) becomes a `source` layer
linked from the ROM, tar archives of at least 4 KiB stored as `+zstd` frames,
zip archives raw. The archive is open without a statement only when it is
the one the catalogue pins for that ROM (the `*-src.tar` the vendor fetch
script builds); any other archive needs `--open --source-license SPDX`. A
catalogue-open copyleft ROM without an open archive is imported
user-supplied, with a `note` line naming the fetch script; an explicit
`--open` that cannot hold is refused. Metadata flags (`--title`,
`--description`, `--notes`, `--vendor`, `--version`, `--date`, repeated
`--label`, `--board`, `--class`, `--requires` for unlisted ROMs,
`--dumped-from`/`--downloaded-from`/`--built-from` with `--revision`, `--url`
and `--provenance-date`) need `--for` or `--unlisted`; lists are stored sorted,
and catalogue fits are prefilled and may only be narrowed. Each import writes
a generated `README.md` layer (UTF-8 Markdown); the README and the config are
open, under CC0-1.0, only when every other layer is open. A package written
before licences existed stays readable; a folder import finds it already
imported when it holds the same ROM and document bytes (any other package
under the tag is a conflict). `image firmware edit NAME:TAG [--to NAME:TAG]`
takes the same metadata and licence flags (`--source` adds or replaces the
archive) and publishes a new revision (`:N` becomes `:N+1` by default) with
the same ROM and document blobs; the old tag keeps the old package. A README
written by hand in the window is kept by every edit; `--regenerate-readme`
generates it from the fields again. `show`
prints every layer's class and licence and, from the push guard's own
classification (local marks and the catalogue included), where the package
may be pushed.
In the window, Import ROM folder… reviews the same statements per set
(Redistribution, SPDX licence, Source archive… and the archive's own licence,
the descriptive fields, fits, provenance and an editable README preview), and
the package pane's Edit metadata… (or Add source archive…) publishes the next
revision as `image firmware edit` does. Every verdict, reason and README text
in both dialogs is the library's preview of the statement, computed on the
library worker. Headless tests:

```sh
cargo test --offline -p window firmware
cargo test --offline -p pc-image --test firmware_metadata
```
The window uses Preferences > store root, falling back to the shared unemu
platform location, for both library media and firmware readiness. CLI `run` uses
the shared default, accepts `--store DIR`, and disables package lookup with
`--no-store`. Plain `load_for_assembly*` library calls never open a host store.
Only wholly pinned declarations participate: an assembled pin or all chip pins
is required, and image/parts shape and chip placement stay unchanged. A store
root that does not exist yet is a silent miss. Other store infrastructure
failures before selection warn and fall back to declared files and ROM
folders; selected-package corruption still blocks use. Machine-image
import uses its explicit target store. Loaded bytes still pass machine pins and ordinary
assembly validation; importing unlisted firmware never grants permission to run it.

File › New machine… in the window creates an empty image from stored firmware
packages (for example after `image firmware import` of the open fixtures) and
blank zero-base disks; it never writes a machine file. Its headless tests:

```sh
cargo test --offline -p pc-image --test zero_base --test zero_base_allocation
cargo test --offline -p window new_machine
```

Settings on an image and the Save as new image… dialog (configuration-only
revisions, CMOS recomputation, tag rules and kept annotations) have headless
tests too:

```sh
cargo test --offline -p pc-image --test revision
cargo test --offline -p image-store --test store version_tags
cargo test --offline -p window -- save_image image_settings layers_dialog \
  library_view::tests::save_as library_view::tests::settings_revision
```

In the window, firmware readiness and Media > Insert > From library use the
Preferences store root when set, otherwise the shared unemu family default:
`~/Library/Application Support/unemu/` on macOS,
`$XDG_DATA_HOME/unemu/` (or `~/.local/share/unemu/`) on Linux,
and `%LOCALAPPDATA%/unemu/` on Windows. Linux falls back when XDG_DATA_HOME
is unset or empty. This directory itself contains `store.toml` and `images/`;
The Insert dialog also accepts an explicit root.
CLI image/media commands require explicit `--store`; firmware commands and
`run` also accept the shared default.
Summary loading and full verification run on background jobs with progress and
nonblocking store-busy errors. Damaged sets remain visible as disabled rows.
Loading all disks in order defaults on; disks already in the catalogue are reused
with their existing write policy. Machine-file sessions journal under
`<machine-file>.local/media/`; image sessions (regular and Gaming mode) use the
running image workspace's `media/` directory, so discarding a Gaming-mode
session removes them. Journal keys contain set digest and medium id, independently of drive
unit. Store-local workspaces are allowed; immutable `images/blobs/` paths and
aliases remain forbidden outputs. Catalogue provenance exposes set/medium GC
roots. Write-protected new disks create no journal. Extras transfer media
remain deferred.

File › Import installation media… in the window validates and publishes through
`pc_image::media_import`, the same code as `image media import`, so its refusals
read as the CLI prints them. The CLI refuses any existing tag before reading a
source. The window writes nothing when the tag exists: it resolves the stored set,
refuses another artifact type or a different layout, metadata or title at once,
and then compares the sources' raw SHA-256 with the stored decoded digests, so the
same content reports already imported whether the set was stored compressed or
raw, and anything else is a tag conflict with a hint to choose another tag. If
another import publishes the same tag between the CLI's check and its publication
with the identical set, the CLI prints that tag and digest and exits 0; a
different set there is a tag conflict.

Defaults differ where the window shows an editable value: a disk's label is the
file name without its extension (the CLI uses the whole file name), the software
platform is DOS (the CLI records none), the tag is derived from the title as
`<title-words>:1.0` until edited (the CLI requires `--tag`), and publisher, year,
genre, labels and description are optional fields (the CLI sets only the title).
An extra's kind comes from the same extension rule and can be changed; its DOS
name is suggested when the file name fits 8.3 and is otherwise left to the
transfer medium. The window always compresses, like the CLI without
`--no-compress`.
Its headless tests, with the shared import and the CLI contract:

```sh
cargo test --offline -p window install_media
cargo test --offline -p pc-image media_import
cargo test --offline -p cli --test image_media_commands
```

In the Insert dialog only entries declared in the machine file show
"fixed by the machine file". Disks added during the session with Browse… or
From library… can be re-inserted read-only or with a journal: browsed images
take a journal path, library disks use their workspace journal key above.
Browse… reuses only rows it added; browsing an image the machine file also
declares adds a separate row, so it can still be journaled.
The drive catalogue only grows, so a switch adds one catalogue entry per policy
and switching back reuses it; the dialog keeps one row per image, while
Next image in catalogue cycles through every entry. Browsing the same file
again (same path and SHA-256) selects its existing row instead of adding one.


Import is opt-in and creates a new local tag using compare-and-swap against
absence. Use another tag when the name exists. It resolves inheritance, verifies
firmware catalogue identities and full media SHA-256, and copies all required
bytes into the store. No input is rewritten. Interrupted journal tails require
`--recover MEDIUM=tip` (the command-complete recovered tip) or
`--recover MEDIUM=generation:N` (a retained closed generation, including zero only
when retained). Repeat the flag for independent choices; unnamed clean journals
use their tip, and an unnamed interrupted journal reports its medium id and
recoverable generations. `cmos` names NVRAM; disk/catalogue ids are `medium-0`,
`medium-1`, etc., in effective traversal order. The local record under
`local/imports/<image-digest>.toml` and CLI output record policy and generation
separately; readonly/volatile media have no generation.
Interior corruption blocks import. Frozen disks and floppies are whole payloads,
compressed by default; CMOS stays raw. `--store` is required. Import uses an
existing store unless `--create` explicitly allows initialization; listing never creates a store. Repeat
`--rom-dir DIR` to locate pinned missing firmware through the same discovery path
as machine-file runs. Tags are checked before ingest and again by CAS at publication.
No platform default or registry is selected.

`image run` owns the workspace until all writers finish. Resuming checks the
supplied tag against the pinned image and prints that identity; a tag pointing elsewhere is rejected. A missing original tag permits
a detached resume. `--new` requires a fresh workspace. Choose a fresh workspace
name for a disposable gaming session; discard removes only that session. Journal
policies persist locally; volatile policies still lose writes on close. Publication
keeps journaled changes, seals fixed disks into sparse parent-bound layers and
freezes changed floppies as whole compressed payloads and CMOS as raw blobs.
A published image keeps the source manifest's annotations (title,
`unemu.firmware.sources`) and records the source in
`org.opencontainers.image.base.digest` (a save without changes is the source
manifest itself). The store refuses to move a version tag and remembers deleted
version tags, so their names can later name only their old digests.
The CLI publishes only new tags. Publish, discard and abandon open existing workspaces only. Discard lists affected files and requires `--yes`; omitting it
provides a preview. `abandon-publication` removes a pending intent while keeping
media and any already published tag. CAS conflicts roll back the intent, so a
losing workspace can immediately run or publish under another tag. `--flatten`
streams layered fixed disks to new compressed bases, permitting chains to restart
before the 64-layer limit. A pending publication blocks Run: repeat the same
publish command to finish its durable intent. The tag changes atomically after closure verification, then
the workspace pins the new image with fresh journals. Interrupted journal tails
must be explicitly recovered with the existing offline media tools before
publication. A crash never automatically discards a workspace. These disk changes
are independent recovery points, not a live machine snapshot.

## Registry push and pull

```sh
cargo run --offline -p cli -- image push dos-lab:1.0.0 --store /path/to/store --dry-run
cargo run --offline -p cli -- image push dos-lab:1.0.0 private/dos-lab:1.0.0 --store /path/to/store
cargo run --offline -p cli -- image push dos-lab:latest private/dos-lab:latest --store /path/to/store --move
cargo run --offline -p cli -- image pull private/dos-lab:1.0.0 --store /path/to/store
cargo run --offline -p cli -- image pull nas.example:5000/private/games:1 --store /path/to/new-store --create --tag games:1
cargo run --offline -p cli -- image push firmware/kiselev-xi8088-1.0.2:1 nas.example:5000/firmware/kiselev-xi8088-1.0.2:1 --store /path/to/store --dry-run
cargo run --offline -p cli -- image pull nas.example:5000/firmware/kiselev-xi8088-1.0.2:1 --store /path/to/store
cargo run --offline -p cli -- image remote ls private/dos-lab
cargo run --offline -p cli -- image remote ls registry.unemu.dev
cargo run --offline -p cli -- image login registry.unemu.dev --user NAME
printf '%s\n' "$TOKEN" | cargo run --offline -p cli -- image login nas.example:5000 --user NAME
cargo run --offline -p cli -- image registry set registry.unemu.dev --namespace unemu --namespace private:private
cargo run --offline -p cli -- image registry set nas.example:5000 --private
UNEMU_REGISTRY_LIVE=1 cargo test --offline -p image-registry -- --ignored live_
UNEMU_KEYCHAIN_TEST=1 cargo test --offline -p image-registry -- --ignored keychain_
```

References resolve like Docker's against `registry.unemu.dev`: `name:tag` is
`registry.unemu.dev/unemu/name:tag`, `owner/name:tag` keeps its owner, a first
component with a dot, a colon or `localhost` is a host, no tag means `latest`,
and `@sha256:…` pins content. `--registry HOST`, then `UNEMU_REGISTRY`, then
`default_registry` in preferences replace the default host. `push` without a
reference uses the local tag. `pull` tags locally in short form (no host, no
`unemu/`), or `--tag`; a digest pull needs `--tag`. `--create` initializes a
missing store.

Firmware packages (`firmware/<catalogue-id>:1`) push and pull like machine
images and media sets; a pulled package lists in `image firmware ls`. A
package whose layers are all open may go to any configured registry,
including `unemu/`, except that an open ROM under a GPL, LGPL, AGPL or MPL
family licence also needs its linked source archive in the same image to
leave a private registry or to enter `unemu/`; the guard refuses it
otherwise. Any user-supplied layer keeps a package in private registries
outside `unemu/`.

Registries are listed in the shared preferences file (`unemupc98/preferences.toml`
in the platform config directory, or `--config FILE`):

```toml
[[registries]]
host = "registry.unemu.dev"
user = "NAME"       # Keychain account; never a password here
# Privacy per namespace (the first path component of a repository):
namespaces = [{ name = "unemu", private = false }, { name = "private", private = true }]

[[registries]]
host = "nas.example:5000"
private = true      # the whole registry; may receive user-supplied blobs outside unemu/
```

An entry without `namespaces` keeps one `private` flag for every repository,
as before. An entry with `namespaces` marks each namespace public or private,
must not also say `private = true`, and takes pushes only into the namespaces
it lists. `unemu/` is the official namespace on every host and never private:
an entry that marks it private is refused when preferences are read. The
array-of-tables spelling (`[[registries.namespaces]]`) is the same document,
and writers keep whichever keys they did not change.

To serve a public `unemu/` and a private `private/` from `registry.unemu.dev`:

- Command line: `image registry set registry.unemu.dev --namespace unemu
  --namespace private:private`. A `--namespace` value is `NAME`, `NAME:private`
  or `NAME:public` (default public); `--private` without `--namespace` marks
  the whole registry private, and with neither the whole registry is public.
  The command creates the entry if needed, keeps its user, plain-HTTP opt-in
  and every other key, and prints a `registry HOST private|public|per
  namespace` line and a `namespace HOST/NAME/ private|public` line per
  namespace.
- Desktop shell: Tools › Preferences… › Registries, Add… (or Edit… on the
  built-in row), type `unemu, private` into Namespaces and keep Private
  registry checked. Listed names without a suffix take the checkbox; `unemu`
  always stays public, and `NAME:public` or `NAME:private` sets one namespace
  on its own.

When the public namespace moves to another host, keep the `private` entry and
give the new host an entry that lists `unemu` only.

The default registry is known without an entry and is public until marked
private. Push refuses any other unlisted host. `plain_http = true` is accepted
only for loopback test registries. Secrets come from `UNEMU_REGISTRY_USER` and
`UNEMU_REGISTRY_PASSWORD`, which belong to one host only: `UNEMU_REGISTRY_HOST`,
or `registry.unemu.dev` when that is unset (so CI that points `UNEMU_REGISTRY`
elsewhere sets `UNEMU_REGISTRY_HOST` too). Otherwise (macOS) they come from the
Keychain item `unemu-registry:<host>` for the entry's user, read through
`/usr/bin/security`. The credential store is chosen at run time, one backend
per operating system; only the macOS Keychain is implemented. Elsewhere the
desktop page keeps a login for the session only, and `image login` refuses and
points to the environment variables. `image login` reads the password or token
from the terminal with canonical mode, echo and signal keys off (termios),
byte by byte: Backspace and Delete edit, Ctrl+U clears, Enter ends, and Ctrl+C
cancels the login and restores the terminal mode; piped standard input is read
as one line. The secret never appears in arguments. Before any request the
store checks that it can keep the secret (for the Keychain: printable ASCII,
at most 1536 characters). The login is then checked against the registry
(anonymous `GET /v2/`, then Basic, with 10 s connect and 15 s read timeouts),
and nothing is stored when the registry refuses it. A registry that already
answers `GET /v2/` without login cannot confirm a password; the login is kept
all the same and `image login` prints an `unverified` line saying so. Then the
secret goes to the Keychain through `security -i`, hex-encoded on the tool's
standard input, and the user is recorded in preferences under the
`preferences.toml.lock` lock that the desktop shell's saves also take, so
neither writer drops the other's keys. The desktop shell's Tools ›
Preferences… › Registries page uses the same functions and store:
- Add and Edit change the namespaces (comma-separated; empty for the whole
  registry), the private flag and the loopback plain-HTTP opt-in. The table
  shows a child row per namespace under its registry, with "Per namespace" in
  the registry's Private column. Only changed keys are written, an entry that
  changed since the dialog opened is refused with "reopen it", and making a
  private registry or namespace public asks for confirmation.
- Remove deletes the entry with its stored secret; the default registry cannot
  be removed.
- Log in… can be cancelled until the user name is recorded; Cancel, Escape or
  closing Preferences cancels it.
- Log out deletes the secret and keeps the user name.
- Set as default, and Test connection (anonymous `GET /v2/`, then Basic, with
  the result per step).

The desktop shell's Image › Push… and Image › Pull… build the same push plan
(`cli::push_plan`: local marks plus the firmware catalogue), the same client
(`cli::registry_client`) and the same providers list as `image push` and
`image pull`, and resolve short names against the same default registry
(`UNEMU_REGISTRY`, then preferences). They run on the library worker thread:
- Push… shows the `--dry-run` classification as a table and the guard's
  verdict while the destination is typed, starting with the registry entry
  and the namespace the destination falls in ("HOST is configured;
  private/ is marked private"). Pull… shows the same line under the resolved
  reference. A non-version tag moves only after a confirmation that shows
  both digests, and only from the digest shown. A
  local tag that names another image than the reviewed one is not pushed.
- Pull… writes the short form of the reference as the local tag unless
  another one is typed.
- Cancel (or Escape, or closing the dialog) stops a transfer before the next
  blob or chunk. A cancelled push publishes no manifest; a cancelled pull
  writes no tag. Quitting during a transfer asks first.

Sign-in states, logins, logouts and removals run on background threads,
because a Keychain call can wait on a system prompt. Existing Docker or oras
logins are not reused; the Log in dialog shows the Docker credential helper as
a disabled, later option. Secrets are not wiped from process memory after use.

The opt-in Keychain test creates a throwaway keychain file in the temporary
directory and never touches the login keychain's items. `security
create-keychain` also adds that file to the user's keychain search list for the
test's duration; the test deletes the keychain, which removes it from the list.
If the test is interrupted, it has printed the path; remove it with `security
delete-keychain "$TMPDIR/unemu-registry-test-<pid>.keychain-db"`, and check
`security list-keychains -d user` afterwards.

Push guard, before any upload: a blob's class is the most restrictive one any
tagged local manifest records for its digest (absent marks count as
user-supplied, and machine import marks everything so). An entry that lists
namespaces takes no push outside them. An image with a user-supplied blob goes
only to a namespace marked private, or to a registry marked private as a
whole, never into `unemu/`;
`unemu/` requires every blob to be open; a restricted push also refuses a
repository that answers anonymous reads, and one whose anonymous answer is a
Bearer token challenge (inconclusive; Bearer authentication is not supported
yet). There is no override. `--dry-run` prints the target, the image digest,
for a configured host a `scope:` line naming the namespace (or the whole
registry) and whether it is private, one `role digest size media-type class
source` line per blob (source `this image`,
or `local tag NAME` when another local image restricts the digest) and
`guard:allowed` or `guard:refused` (exit 1), without network access. A reference
with both a tag and a digest is refused. Version tags (`x.y.z`) never move;
other tags move only with `--move`, and the refusal shows both digests.
Interrupted chunked uploads resume from the registry's offset, also on a later
push. Cross-repository mounts exist in the library API only; the CLI does not
try them. Pull continues a blob cut off mid-download with `Range` requests (up
to three) from the bytes already staged, and never rewrites a local tag that
names another digest. Credentials go only to the registry's own scheme, host
and port; redirects elsewhere (such as object storage) get none, and error
messages omit URL query strings.

The live test pushes to `registry.unemu.dev` (or `UNEMU_REGISTRY`) with
`UNEMU_REGISTRY_USER`/`UNEMU_REGISTRY_PASSWORD`, which need push and delete
rights on `unemu/test-*`; set `UNEMU_REGISTRY_HOST` as well when `UNEMU_REGISTRY`
names another host. It deletes its repository content afterwards.

## Official template images

```sh
bash vendor/unemu-at-bios/fetch.sh
bash vendor/vgabios/fetch.sh
bash vendor/xtide-universal-bios/fetch.sh
cargo run --offline -p cli -- image template build --store /path/to/templates --dry-run
cargo run --offline -p cli -- image template build --store /path/to/templates
```

`image template build` is a command rather than a script around `image
firmware import` and New machine…: the template machines come from the create
API (`pc_image::create`), which has no command of its own, and the push guard
must judge each manifest before its tags are written. It reads the open fixtures
under `vendor/` in the working directory (`--vendor DIR` for another copy) and
writes into `--store` (created when missing), in this order (design:
machine-images §5.5):

| Local tags | Registry repository | Image |
| --- | --- | --- |
| `unemu/firmware/unemu-at-bios-1.0.2-unemu.2:1.1.0`, `:latest` | `unemu/firmware/unemu-at-bios-1.0.2-unemu.2` | unemu AT BIOS package (the unemu fork of Kiselev 8088_bios) with its source archive |
| `unemu/firmware/vgabios-0.8a-unemu.1-std:1.1.0`, `:latest` | `unemu/firmware/vgabios-0.8a-unemu.1-std` | unemu LGPL VGA BIOS package with its source archive |
| `unemu/firmware/xub-r638-xtp-ata1f0:1.1.0`, `:latest` | `unemu/firmware/xub-r638-xtp-ata1f0` | XTIDE Universal BIOS r638 package with its source archive |
| `open-at:1.1.0`, `:latest` | `unemu/open-at` | IBM 5170-class 80286, the three ROMs, 640 KiB + 1 MiB, VGA, COM1 without a mouse, one empty 1.2M drive, a blank zero-base 504 MiB (1024/16/63) ATA disk at 1F0h, no sound card |
| `open-at-games:1.1.0`, `:latest` | `unemu/open-at-games` | `open-at` plus SB16 (220h, IRQ 5, DMA 1/5), a Microsoft mouse on COM1 and a 2 MiB Lo-tech EMS board (260h, frame D000h) |

The local tags are the short form `image pull` writes for each repository, so
`image push TAG` without a reference reaches the repository in the table. Each
source archive must be the one the catalogue pins (its fetch script builds it);
another archive, or a missing fixture, stops the build and names the script.
No operating system or other software goes into a template. The output is one
`TAG<TAB>DIGEST<TAB>firmware|machine<TAB>created|present|moved:OLD|kept:CURRENT`
line per tag, written as soon as its image is built, then `guard:allowed`. A
tag that already names the same image stays; a version tag naming another
image stops the build (give a changed template a new version in
`pc_image::template::VERSION`). `latest` moves by compare-and-swap, and only
away from an image that lower versions of the same repository name alone; it
stays (`kept:`) on a higher version or on an image no version of the
repository names. The same fixtures give the same digests on every run and
host: nothing depends on the clock, the host or the fixture paths; a test pins
the digests of the published generation.

Before any tag of an image is written, the build applies the push guard for
`unemu/` on the default registry, from the store's local marks and the firmware
catalogue: every blob open, every copyleft ROM with its linked source archive.
A refusal stops the build, for example in a store where another local tag
records the same ROM bytes as user-supplied, so build into a dedicated store.
The tag is written only for the manifest the guard judged. `--dry-run` builds
in a scratch store under the system temporary directory, removes it, and
reports what a build would do to each tag of `--store` (which is never written
and need not exist): `new`, `present`, `conflict:DIGEST` (a version tag naming
another image), `moves:DIGEST` or `kept:DIGEST` (the alias), then `dry run; no
blobs or tag written`. When `--store` exists, the guard judges each image with
its marks as well as the scratch store's, so a dry run refuses what the build
would refuse.

The build never pushes; publishing is the maintainer's step. Log in once
(`image login registry.unemu.dev --user NAME`), review every dry run, then push
the version tags and the aliases:

```sh
S=/path/to/templates
cargo run --offline -p cli -- image push unemu/firmware/unemu-at-bios-1.0.2-unemu.2:1.1.0 unemu/firmware/unemu-at-bios-1.0.2-unemu.2:1.1.0 --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/vgabios-0.8a-unemu.1-std:1.1.0 unemu/firmware/vgabios-0.8a-unemu.1-std:1.1.0 --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/xub-r638-xtp-ata1f0:1.1.0 unemu/firmware/xub-r638-xtp-ata1f0:1.1.0 --store "$S" --dry-run
cargo run --offline -p cli -- image push open-at:1.1.0 unemu/open-at:1.1.0 --store "$S" --dry-run
cargo run --offline -p cli -- image push open-at-games:1.1.0 unemu/open-at-games:1.1.0 --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/unemu-at-bios-1.0.2-unemu.2:latest unemu/firmware/unemu-at-bios-1.0.2-unemu.2:latest --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/vgabios-0.8a-unemu.1-std:latest unemu/firmware/vgabios-0.8a-unemu.1-std:latest --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/xub-r638-xtp-ata1f0:latest unemu/firmware/xub-r638-xtp-ata1f0:latest --store "$S" --dry-run
cargo run --offline -p cli -- image push open-at:latest unemu/open-at:latest --store "$S" --dry-run
cargo run --offline -p cli -- image push open-at-games:latest unemu/open-at-games:latest --store "$S" --dry-run

cargo run --offline -p cli -- image push unemu/firmware/unemu-at-bios-1.0.2-unemu.2:1.1.0 unemu/firmware/unemu-at-bios-1.0.2-unemu.2:1.1.0 --store "$S"
cargo run --offline -p cli -- image push unemu/firmware/vgabios-0.8a-unemu.1-std:1.1.0 unemu/firmware/vgabios-0.8a-unemu.1-std:1.1.0 --store "$S"
cargo run --offline -p cli -- image push unemu/firmware/xub-r638-xtp-ata1f0:1.1.0 unemu/firmware/xub-r638-xtp-ata1f0:1.1.0 --store "$S"
cargo run --offline -p cli -- image push open-at:1.1.0 unemu/open-at:1.1.0 --store "$S"
cargo run --offline -p cli -- image push open-at-games:1.1.0 unemu/open-at-games:1.1.0 --store "$S"

cargo run --offline -p cli -- image push unemu/firmware/unemu-at-bios-1.0.2-unemu.2:latest unemu/firmware/unemu-at-bios-1.0.2-unemu.2:latest --store "$S"
cargo run --offline -p cli -- image push unemu/firmware/vgabios-0.8a-unemu.1-std:latest unemu/firmware/vgabios-0.8a-unemu.1-std:latest --store "$S"
cargo run --offline -p cli -- image push unemu/firmware/xub-r638-xtp-ata1f0:latest unemu/firmware/xub-r638-xtp-ata1f0:latest --store "$S"
cargo run --offline -p cli -- image push open-at:latest unemu/open-at:latest --store "$S"
cargo run --offline -p cli -- image push open-at-games:latest unemu/open-at-games:latest --store "$S"
```

Each dry run must end in `guard:allowed` with every blob `open`. A version tag
that already names another image on the registry is refused; push a new
version instead. Once `latest` names an older version there, its push stops
with both digests; repeat it with `--move` after checking them.

The desktop shell's **File › Get templates…** lists these templates from the
default registry and pulls the chosen ones;
[`design/machine-images.md`](design/machine-images.md) §5.5 describes its
behaviour.

## Shareware installation media (local only)

```sh
vendor/shareware/keen4/fetch.sh                # Commander Keen 4 1.4: keen4-sw14-1440.img, extras/
vendor/shareware/wolf3d/fetch.sh               # Wolfenstein 3-D 1.4: wolf3d-sw14-1440.img, extras/
vendor/shareware/duke2/fetch.sh                # Duke Nukem II 1.0: duke2-sw10-1440.img, extras/
SHAREWARE_ARCHIVE=/path/to/1wolf14.zip vendor/shareware/wolf3d/fetch.sh   # your own copy; no network
cargo run --offline -p cli -- image media import vendor/shareware/keen4/keen4-sw14-1440.img --title "Commander Keen 4: Secret of the Oracle (shareware 1.4)" --tag keen4-shareware:1.4 --label '1=Disk 1 of 1 - Shareware v1.4 installation' --extra vendor/shareware/keen4/extras/README.DOC:README.DOC --extra vendor/shareware/keen4/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1991 --genre platformer --platform DOS --description 'Commander Keen 4: Secret of the Oracle, version 1.4, the shareware episode of a side-scrolling platform game in EGA graphics by id Software, distributed by Apogee Software. Keen searches the Shadowlands of Gnosticus IV for the eight missing Council Members. The disk holds the files of the original Apogee shareware archive 4keen.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-readme-keen4-v1.4 --license-text README.DOC --store /path/to/store --create
cargo run --offline -p cli -- image media import vendor/shareware/wolf3d/wolf3d-sw14-1440.img --title "Wolfenstein 3-D: Episode 1 (shareware 1.4)" --tag wolf3d-shareware:1.4 --label '1=Disk 1 of 1 - Shareware v1.4 installation' --extra vendor/shareware/wolf3d/extras/VENDOR.DOC:VENDOR.DOC --extra vendor/shareware/wolf3d/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1992 --genre "first-person shooter" --platform DOS --description 'Wolfenstein 3-D, episode 1, Escape from Wolfenstein, version 1.4, the shareware episode of a first-person shooter with texture-mapped VGA graphics by id Software, published by Apogee Software. As B.J. Blazkowicz the player escapes from Castle Wolfenstein through ten floors, one of them secret. It needs a 286 or better. The disk holds the files of the original Apogee shareware archive 1wolf14.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-vendor-v11.01.92 --license-text VENDOR.DOC --store /path/to/store
cargo run --offline -p cli -- image media import vendor/shareware/duke2/duke2-sw10-1440.img --title "Duke Nukem II: Episode 1 (shareware 1.0)" --tag duke2-shareware:1.0 --label '1=Disk 1 of 1 - Shareware v1.0 installation' --extra vendor/shareware/duke2/extras/LICENSE.DOC:LICENSE.DOC --extra vendor/shareware/duke2/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "Apogee Software" --year 1993 --genre platformer --platform DOS --description 'Duke Nukem II, episode 1, version 1.0, the shareware episode of a side-scrolling platform game by Todd Replogle and Apogee Software, in which Duke fights the alien Rigelatins. It needs VGA graphics and a 286 or better. The disk holds the files of the original Apogee shareware archive 4duke.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-license-v12.01.93 --license-text LICENSE.DOC --store /path/to/store
cargo run --offline -p cli -- image push wolf3d-shareware:1.4 --store /path/to/store --dry-run   # guard verdict for unemu/wolf3d-shareware:1.4
```

Each script downloads the publisher's original shareware archive from a pinned
URL (FUNET's mirror of Apogee's file area, with a byte-identical 1994 CD-ROM copy
as fallback), checks its SHA-256 and member list, and caches it in `build/`, so
later runs are offline. It copies every archive member unmodified, in archive
order and with the archive's DOS timestamps, onto a 1.44M FAT12 floppy with a
fixed label, serial and non-bootable volume sector, checks the image's pinned
SHA-256, and writes the vendor and licence texts to `extras/`. Nothing from the
archive is run. Run the scripts inside `nix develop`, which provides mtools
4.0.49 (it produced the pinned images, measured on macOS with the host's
Info-ZIP UnZip 6.00; Linux not measured); unzip and curl come from the host.
They are not part of CI: they need the network and third-party content.
Everything under `vendor/shareware/` except the scripts and READMEs is
gitignored, so archives, images, extras and unpacked files are never committed.
The maintainer approved publishing the three sets on 2026-11-15 (decision-26
amendment), so the imports above carry the licence statement: each set is open
under the vendor document's own `LicenseRef-` (named after the document and
its revision; Keen 4's `README.DOC` has none, so its id names the release),
with that document as its terms text, and the push guard lets it into
`unemu/`. The set-level metadata names Apogee Software as publisher (the
Wolfenstein 3-D and Duke Nukem II terms ask that a distributor's descriptions
name Apogee), the developer the archive's own texts credit (id Software for
Keen 4 and Wolfenstein 3-D, Apogee Software for Duke Nukem II), the release
year, the genre, the DOS software platform and a one-paragraph description
(the episode, and that the disk holds the files of Apogee's original shareware
archive unmodified). Imported without `--open` a set stays user-supplied and private. Each `vendor/shareware/<title>/README.md` lists the
source and every hash and summarises the distribution terms; the full texts
stay local, in `extras/`.

## Build / run / test

```sh
cargo build                                  # build the workspace
cargo run -p cli -- --boot                   # Real16 timer demo; no args print usage
cargo run --offline -p cli -- --bios vendor/kiselev-bios/bios-xi8088.bin
cargo run --offline -p cli -- --bios /path/to/bios.bin --allow-unlisted-firmware
cargo run --offline -p cli -- machine check machines/at-freedos.toml
cargo run --offline -p cli -- run --machine machines/at-freedos.toml --max-steps 12000000 --idle-limit 1000000 --print mda
cargo run --offline -p window --bin unemupc98-window -- --machine machines/at-freedos.toml --display mda
cargo test --workspace                       # unit + golden + integration
cargo test --release -p devices              # release-only device behaviour (CI runs it too)
cargo test --workspace -- --ignored          # slow goldens: BIOS POST, FreeDOS boot, DOS sessions
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --workspace -- --include-ignored # CI goldens lane: every fixture test and the goldens; fail if a fixture is missing
# Corrected per-clock oracle versus production deadlines (no environment gate):
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test scheduler_ab -- --include-ignored --nocapture
# Opt-in host timing only, BIOS POST and FreeDOS prompt, median of three per engine:
UNEMUPC98_REQUIRE_FIXTURES=1 UNEMUPC98_SCHEDULER_WALL_TIMES=1 cargo test --offline --release -p cli --test scheduler_ab scheduler_engine_wall_times -- --ignored --nocapture
# DOS sessions and their final console screens (requires fetched fixtures):
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test dos_interactive_golden -- --ignored --nocapture
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test dos_write_golden -- --ignored --nocapture
# FreeDOS boots A: with the open AT BIOS and XT+ ROM, then lists readonly C:
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test ide_freedos_golden -- --ignored --nocapture
# Open AT protected-mode test floppy: INT 15h 88h/87h, PM entry, 8042/triple-fault resets, A20 (design/at-pm-test.md)
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test at_pm_test_golden -- --include-ignored --nocapture
cargo test -p devices                        # one crate's tests
cargo build --offline -p window --bin unemupc98-window
cargo run --offline -p window --bin unemupc98-window # default machines/at-freedos.toml
cargo run --offline -p window --bin unemupc98-window -- --machine machines/local.toml --allow-unlisted-firmware
cargo tree --offline -i window               # window is a leaf frontend
```

Ordinary tests and ignored goldens share compiled artifacts because both are
`cargo test` runs in the same profile. The default `test` profile inherits the
workspace's `dev` settings, which lets `cargo build` and `cargo test` also share
library artifacts. Use the same profile for ordinary tests and goldens;
`cargo test --profile test --workspace -- --ignored` makes the choice explicit,
as the CI gate does. Keep the separate release device run to cover behaviour
that intentionally differs when debug assertions are disabled.

Dependencies, including the pinned CPU core, use optimization level 3. The hot
workspace crates (`devices`, `machine`, `bus`, `cpu`, `assembly`, `audio`,
`chipset`, `media`) also use level 3. Debug information, debug assertions and
overflow checks remain enabled; frontends and the other workspace crates keep
their unoptimized dev settings.
Optimized code can be harder to step through even with debug information. For a
focused device investigation, temporarily override just that crate:

```sh
cargo --config 'profile.dev.package.devices.opt-level=0' test -p devices
```

To investigate an intermittent fast-suite failure, run two independent targets
in parallel from the devShell. This captures every run's complete output and
exit code, including failures that pass on an immediate rerun. Additional host
load can run alongside this loop; keep the same load for before/after comparisons.

```sh
scratch="$(mktemp -d)"
for lane in a b; do
  (
    for iteration in $(seq 1 20); do
      if CARGO_TARGET_DIR="$scratch/target-$lane" cargo test --offline --workspace --no-fail-fast >"$scratch/$lane-$iteration.log" 2>&1; then
        echo 0 >"$scratch/$lane-$iteration.exit"
      else
        echo "$?" >"$scratch/$lane-$iteration.exit"
      fi
    done
  ) &
done
wait
rg -n 'FAILED|panicked' "$scratch" --glob '*.log'
rg -n '^[1-9][0-9]*$' "$scratch" --glob '*.exit'
```

System BIOS loads on IBM 5170 use a SHA-256 catalogue. Option ROMs also use catalogue hashes and format checks. An unknown image is rejected
with its full hash and the known entries' fetch instructions. For development,
`--allow-unlisted-firmware` accepts unknown bytes and prints exactly one
`warning: running unlisted firmware (sha256 HASH)` line to stderr. Placement
(positive size divisible by 8 KiB, ending at `0x100000`) is still checked; listed
images still meet board, CPU and required-part constraints. The CLI `--bios`
automatically top-aligns any positive image whose size is a multiple of 8 KiB
and at most 1 MiB: base = `0x100000 - size` (16 KiB gives `0xFC000`). Other
sizes are rejected. An explicit machine-file base is checked as supplied.
This run-only flag
is never saved in machine files or used by goldens. The CLI `--bios PATH`
boots the complete board with a volatile blank 1.44 MB floppy and writes COM1
bytes to stdout. It prints one `stopped: ...` line to stderr and exits with
status 1 for unsupported instructions, execution/exception failures, or
configuration/policy/observation/persistence errors. Halt, reset-request and
budget-exhausted outcomes succeed: reaching the budget is a normal bounded
run's end.
Catalogue boards must have a system BIOS ROM, start at their power-on reset
vector and keep all RAM and other images outside that ROM's range. The escape
flag does not relax these requirements; custom-entry Rust rigs use `Bare`.

`machine check FILE` parses, merges, lowers, validates and assembles to run
image-byte catalogue checks, without executing guest instructions. It exits
0 for valid input, 1 for load/validation/assembly errors and 2 for usage errors;
diagnostics retain the defining file and line, including inherited firmware.
Checks never create or repair journals or locks. Missing journals are valid with
"journal will be created"; recoverable tails/unsealed tips are valid with a
warning that recovery runs on the next mount. Interior damage and wrong base
bindings are invalid at the drive's file:line. A busy journal is valid with a
warning that it was not inspected. Existing locks use offline shared inspection;
a missing lock uses the same read-only scanner without a lock and reports it.
Journal drives assemble as volatile for the remaining catalogue checks.
`run --machine FILE` uses `RunPolicy::natural`: `--max-steps` defaults to
20,000,000 completed-step equivalents and `--idle-limit` to 1,000,000 halted PIT clocks.
Both accept unsigned decimal integers (zero is allowed). It uses the same stop
messages and exit codes as `--bios`, with usage errors exiting 2. `--print CARD`
selects a display text dump or UART TX bytes on stdout. Without it, the first
display is chosen, then the first UART, or no output if neither is configured.
UART capture is bounded to 1 MiB; overflow is an error. Normal halt and budget
exhaustion finish journal persistence and print maintenance warnings on stderr.
Execution errors leave unsealed commands for recovery on the next mount.
A configured card without printable output is an argument error (exit 2). Both file commands accept
`--allow-unlisted-firmware` for development. Existing `--boot`, `--bios` and
`media` commands retain their contracts. The FreeDOS prompt, DIR and file-write
goldens load `machines/at-freedos.toml`; BIOS goldens and synthetic rigs retain
Rust presets.

The window binary requires a graphical desktop and fetched BIOS/FreeDOS fixtures.
It uses a fixed 2× pixel scale and keeps advancing through the FreeCOM date/time
prompts. Press Enter twice to accept them, then type a DOS command such as `dir`
and Enter. Physical key positions use the US layout: letters, digits,
punctuation, Enter/Backspace/Tab/Escape/Space, both Shift/Ctrl/Alt keys, locks,
F1–F12, the numeric keypad, arrows and navigation keys are supported. Pause/Break,
PrintScreen, system and media keys are ignored. Host repeat sends plain make
codes; fake-Shift wrappers are sent only on the first make and final break.
Guest typematic settings do not change the host repeat rate. On macOS, winit 0.30
does not deliver CapsLock key events, so Caps Lock does not reach the guest.
Focus loss releases all held keys, including modifiers. Keys are ignored after
the machine stops. NumLock wrappers follow the guest's keyboard LED state,
including guest reboots and Ctrl+NumLock (Pause), never the host OS lock state.

Host shortcuts on macOS use Command. Shortcut letters and punctuation follow
the logical host keyboard layout, as native macOS menus do. Guest scan codes
continue to use physical positions. Press ⌘ first for Command+Shift shortcuts:
Shift pressed before Command is ordinary guest input, with its balanced break.
Shift pressed while Command is held produces no guest bytes.

Command and its chord keys never produce guest scan codes; Ctrl and Alt reach the guest immediately, including
Ctrl+Alt+Del. The reset menu/toolbar action still sends a complete guest
Ctrl+Alt+Del sequence. On other platforms the existing bindings stay in effect.

| Action | macOS | Other platforms |
| --- | --- | --- |
| Pause / resume | ⌘P | Ctrl+Alt+P |
| Stop, return to library | ⌘K | Ctrl+Alt+Q |
| Send guest Ctrl+Alt+Del | ⌘End | Ctrl+Alt+End |
| Next / eject / insert A: | ⌘F / ⌘E / ⌘I | Ctrl+Alt+F / E / I |
| Next / eject / insert B: | ⌘⇧F / ⌘⇧E / ⌘⇧I | Ctrl+Alt+Shift+F / E / I |
| User mute | ⌘⇧M | Ctrl+Alt+M |
| Release pointer | ⌘G | Ctrl+Alt+G |
| Full screen | ⌘Enter | Ctrl+Alt+Enter |
| Hold fast-forward | ⌘T | Ctrl+Alt+T |
| New / open machine file | ⌘N / ⌘O | Ctrl+N / Ctrl+O outside guest focus |
| Settings | ⌘, | Ctrl+, outside guest focus |
| Keyboard shortcuts | ⌘⇧/ | F1 outside guest focus |

On macOS, ⌘Q quits, ⌘W closes, ⌘H hides, ⌘M minimizes and ⌘Tab switches
applications. Help > Keyboard shortcuts and the menus show the platform's bindings.
New machine remains a placeholder. Enter starts the selection when stopped.

While a machine runs, File > Machine library, Open in File > Open machine
file… (after a path is entered; closing that dialog changes nothing), Exit,
⌘Q, ⌘W and the window's close button pause it first and ask
"unemupc98 is running <machine>. Stop it and continue?" with Stop machine and
Cancel (Esc; Enter and Space do not answer). Cancel resumes the machine where
it paused, without catch-up, and hands back guest keyboard focus and pointer
capture if the guest had them; an explicit pause stays in effect. Exit, ⌘Q,
⌘W and the close button also work while a menu or another dialog is open:
they close or cancel it first, keeping only this question or the Gaming-mode
keep-or-discard question. One Escape closes only the top-most dialog. A
stopped, halted or failed machine is not asked about, and Machine > Stop and
Power cycle act at once. A Gaming-mode session asks Keep as new image or
Discard after stopping instead. On macOS the global menu bar holds only the
application menu: About unemupc98, Hide unemupc98 (⌘H), Hide Others (⌥⌘H),
Show All and Quit unemupc98 (⌘Q). Quit and ⌘Q reach this question like Exit.
Dock › Quit, logout, restart and shutdown wait for it as well: with no machine
running they quit at once after persistence; Stop machine quits once the stop
and persistence have finished; Cancel keeps the application running (logout
reports that unemupc98 cancelled it). A stop that fails to finish its journals
also cancels them and keeps the error dialog up. While a file dialog is open
they are cancelled; close the dialog and quit again.

View > Speed selects 100% (the default on every Start), 200%, 400% or Unlimited.
The toolbar's Speed combobox, right after Layers, offers the same choices and
issues the same command. Its closed box shows the mode in effect, so it reads
Unlimited while the hold below is active and the selected mode again
afterwards; its list and the View > Speed check mark the persistent selection,
and a choice made during the hold takes effect when the hold ends.
Hold ⌘T on macOS or Ctrl+Alt+T elsewhere for Unlimited. On macOS,
releasing Command or T restores the selected mode; elsewhere releasing T
restores it even if Ctrl or Alt was released first. Focus loss or opening a
menu/dialog also ends the hold. The chord's T make, repeat and break never reach the guest;
on other platforms, previously forwarded modifiers receive balanced breaks.
The status bar reports the mode and measured virtual-time/host-time ratio over the last second.
This host pacing control is separate from the machine's configured instruction rate.

Input is queued until the next run boundary; a full 8042, a disabled
interface or a pending serial byte defers input without drops. Only the window
frontend reads `Instant`; it targets virtual time from elapsed host time scaled
by the selected mode. Guest time remains retired instructions plus halted PIT
clocks. The host budget follows the observed presentation period, clamped between
one 60 Hz period (about 16.7 ms) and 50 ms. Paced modes run until their virtual
target is reached or that full period is used up. Unlimited has no virtual-time
target and leaves 3 ms of the period for presentation (about 13.7 ms of execution
at 60 Hz). Budget checks occur between chunks of at most 1 ms of virtual time,
plus scheduler instruction-boundary rounding. A 100% frame needing 8 ms of host
work can finish in one presentation. Slower repaint cadences allow larger slices
within the 50 ms bound. Rendering and other host costs still
contribute to the measured achievable speed. Display
capture and texture upload run at most once on each 60 Hz presentation deadline;
extra UI repaints grant no guest work. Paced modes discard backlog above 50 ms
times the selected ratio, so a slow host accumulates no catch-up debt. Mode
changes, pause/resume and uncovering re-anchor pacing at the current guest clock.
Hidden, suspended, paused and modal time grants no catch-up work.
Host audio is muted outside 100%, including during the hold, without time-stretch.
Returning to 100% clears queued samples and converter history and prefills fresh
audio; explicit user mute remains in effect. Status distinguishes `Audio muted
(speed)` from user mute. During speed mute, the View menu shows whether
user mute is on or off; ⌘⇧M on macOS (Ctrl+Alt+M elsewhere) toggles that preference for the return to 100%
and does not override the speed mute. Playback never drives virtual time.
Headless CLI execution and goldens retain their virtual-time budgets and never
use host pacing. See [`design/observation.md`](design/observation.md#window-pacing)
for board-clock conversion and the input replay contract.
A natural halt, CPU failure or exhausted virtual-time range stops advancement
and retains the last screen until closed. Its speed cell keeps the selected mode
without a stale ratio; Stop returns the cell to `Speed 100%`. Blink stays visible; its deterministic
phase can derive from virtual time later. Workspace and presenter tests remain
headless. The input log is test-owned, with no user-facing log flag.

Machine > Settings… and ⌘, on macOS (Ctrl+, elsewhere outside guest focus) open
the seven-tab property sheet for the selected machine file. Stop before editing;
running/paused sessions are read-only. Review the child-file preview before
OK/Apply. The saved local child is selected in the library; replacing an existing
child requires consent.

The library tree includes All images, Recently played, Favourites, Machine files, Labels and
Genre/Platform/Year/Developer/Publisher/Series facets. Search matches cached
catalogue text; click a column header to sort, click a repository to expand its
tags, and select a tag for hardware, readiness and storage details. Start, Enter
and double-click check the pinned selection cheaply, then launch it when ready
or display its blocker. Selection reads descriptors and file sizes; Start verifies
and materializes the selected closure once. Start reuses the regular workspace.
Machine > Gaming mode creates an isolated disposable workspace for every writable
disk, catalogue medium and CMOS. On stop or exit choose Keep as new image… or
Discard. Successful Keep publishes before removing the disposable workspace.
After a crash or deferred exit, unfinished sessions appear in the library with
those same choices. Read-only and volatile media retain their declared policies.

File > Import image… imports a selected machine file under a new repository:tag.
The status bar reports background import progress; missing inputs offer the
existing Locate firmware flow. Tools > Image store folder… changes the optional
`store_root` preference. Defaults are `~/Library/Application Support/unemu`
on macOS, `%LOCALAPPDATA%/unemu` on Windows and
`$XDG_DATA_HOME/unemu` (otherwise `~/.local/share/unemu`)
on other Unix hosts. The store still requires a supported local filesystem.

Machine > Disk layers… and the Layers toolbar button show per-medium chains,
digests, sizes, sharing and working sectors. Save as layer… and New image from
here… ask for a new repository:tag and stop the machine before publication;
New also offers Flatten. Gaming sessions offer Keep as new image… instead.
Discard changes lists writable media with changes before confirmation.
For machine files the dialog offers Import image…. Edit > Delete tag… confirms
removal of that tag only; shared bases remain. These menu commands have no new
key chords; existing shell shortcuts retain their platform mapping.

The window loads `machines/at-freedos.toml` from the workspace recorded by
`CARGO_MANIFEST_DIR` at build time, independently of the current directory.
`--machine FILE` is relative to the current directory; image and inheritance
paths inside it are relative to their defining file. A missing default file is
an error naming that path. Running without the build workspace needs an explicit `--machine`.
`--display CARD` chooses a display by id; the default is the first display in
file order (MDA in the shipped machine). Positional BIOS/floppy paths are removed;
use an extending file as shown in `machines/README.md`. From outside the
workspace, Cargo also needs `--manifest-path /path/to/workspace/Cargo.toml`.
`--help`, `--version` and `--about` include the embedded fonts' attribution.
On Linux the window requires the host's X11 or Wayland client libraries at run
time; a bare Nix devShell may not put them on the dynamic loader's search path.

The ignored set includes firmware/DOS goldens and corrected PerClock-vs-Deadline
comparisons, including final RAM/register/device state. The per-clock engine is
available through the non-default `reference-scheduler` feature enabled by CLI
dev-dependencies, never through `MachineConfig`. Legacy comparisons and
`UNEMUPC98_SCHEDULER_AB` are retired. Step budgets include REP iterations; the
command budget is calibrated to the final DOS file-write listing. No arguments
print usage to stdout with status 0; unknown or extra arguments print usage to
stderr with status 2. The informative
`scheduler_engine_wall_times` test skips with one line unless
`UNEMUPC98_SCHEDULER_WALL_TIMES=1` is set, including in ordinary slow/CI runs.

## Offline persistent media

```sh
cargo run -p cli -- media generations --base disk.img --journal disk.img.upfj
cargo run -p cli -- media verify --base disk.img --journal disk.img.upfj
cargo run -p cli -- media verify --base disk.img --journal disk.img.upfj --repair-tail --backup before-repair.upfj
cargo run -p cli -- media rollback --base disk.img --journal disk.img.upfj --to 1 --backup before-rollback.upfj
cargo run -p cli -- media commit --base disk.img --journal disk.img.upfj --to 1 --output restored.img
```

The same `media generations`, `verify`, `verify --repair-tail`, `rollback`,
`commit` and `compact` commands accept CMOS NVRAM journals. Pass the original
64-byte `cmos.bin` with `--base` and its separate journal with `--journal`.
Exports remain 64 bytes, preserving the original clock/control prefix and the
selected generation's general RAM (0Eh–3Fh except the base-supplied shutdown
byte 0Fh). See `design/persistent-media.md`
§ CMOS NVRAM journals.

Inspections replay read-only under a shared stable journal lock. An existing
lock is opened read-only, so a 0444 lock file is supported. If the lock is absent
and its directory forbids creation (EACCES, EPERM or EROFS), inspection proceeds
without a lock and prints one line saying so. A writing session must already
have created that lock; an existing lock is never bypassed. Mutating tools still
require an exclusive lock with write access. A live journal session excludes all
tools. Never delete `<journal>.lock`: it is not a stale-PID file; deletion during
a session defeats the single-writer guarantee for both sessions and tools.
`generations` includes the base SHA-256, retained
anchor, closed ids/reasons/end offsets/counts and any unsealed tip. A closed
marker does not certify filesystem consistency or bootability.

`verify` reports binding/frame/grammar failures, recoverable-tail bytes and the
latest reconstructible state. Exit codes: **0** fully valid/closed, **1**
corruption, wrong base or I/O failure (including a missing journal and lock
conflicts, a dangling journal symlink or a
post-lock canonical-path change), **2** usage/config error, **3** recoverable tail
or unsealed commands. Reserved journal temporary names in any input/output,
including the base and resolved symlink targets, are configuration errors (exit 2).
Missing base/journal inputs are checked before locking,
leave no new files and report the input's role and path. Non-regular inputs,
including FIFOs, are refused without blocking. `--repair-tail` takes an exclusive
lock, synchronizes a full backup at an unused path, then performs normal mount
recovery/sealing. Interior damage and incomplete anchors are refused.

Rollback requires a retained closed generation; generation 0 is available only
before retention introduces a positive anchor. An unavailable id is exit 2.
After synchronizing the full backup and its directory, rollback truncates after
the chosen marker (128 bytes for 0) and synchronizes the journal. Later history
and any open tip are removed; their ids can be reused on subsequent mounts.
Keep the immutable base alongside backups. Existing backup paths (including
aliases of inputs) and reserved `.create-` filenames are refused; tools never
delete backups. On Unix a backup inherits the journal's exact permission bits
before any bytes are copied. A copy or backup-sync failure reports the retained
partial, invalid backup's path; do not use it as a valid recovery source. All I/O
errors identify the affected base, journal, lock or backup path. Tail diagnostics name
the safe truncation offset and discarded byte count separately from the failure
offset. Journal and image bytes stay unchanged on inspection.

`media commit` (alias `compact`) exports a retained closed generation, including
available generation 0, to a new raw image. It requires a shared lock and a valid
sealed tip; repair/seal interrupted history first. An occupied output, including
an input alias, is exit 2. The same-directory temporary is read back for size and
SHA-256 verification, synchronized, published by a no-clobber hard link, then its
directory is synchronized. Neither input changes. Select the new base and a fresh
journal path explicitly when continuing from an export; config is never edited.

Journal mode applies `keep_generations` after each successful checkpoint/session
close. The count includes the oldest retained point encoded as an anchor; keep=2
over ids 1–5 retains anchor 4 and id 5. Later ids/reasons survive; sequences restart
at 1. Retention verifies all retained states before file sync, atomic journal
rename under the stable lock, and directory sync. Maintenance failures
produce one warning per affected drive after every drive has been finished; they
do not turn a durable session close into a run failure. Pre-publication failures preserve the old
journal and permit retry at a later close; publication/sync uncertainty requires
reopening and verifying before further persistent work. Retention keeps exact
journal mode bits, but the replacement is owned by its creator. The next exclusive
mount removes this journal's regular decimal-named retention leftovers after safe
identity checks; it never follows a symlink or mounts a leftover. Reserved
`.retain-` and `.export-` names, like `.create-`,
cannot be mounted as journals or used for backups. Temporaries are never imported,
and backups are never pruned.

### Journal session walkthrough

Journal mode is available in Rust `MachineConfig` data and in TOML/YAML
machine-file floppy drive tables with `write = "journal"` and an explicit
`journal` path. The CLI and window frontends do not load machine files yet;
that integration is in progress, and their run arguments have no journal flag.
This runnable Rust example uses the small WRITE DATA preset: it programs DMA
channel 2, writes one sector, drains the controller result and halts. Run from
the repository root in the devShell. Generated media and the example project's
build output stay in a new temporary directory; `cargo build -p cli` uses the
repository's `target/` directory.

```sh
repo_dir="$PWD"
demo="$(mktemp -d)"
mkdir "$demo/src"
cat > "$demo/Cargo.toml" <<EOF
[package]
name = "journal-demo"
version = "0.1.0"
edition = "2021"

[dependencies]
assembly = { path = "$repo_dir/crates/assembly" }
EOF
cat > "$demo/src/main.rs" <<'RS'
use assembly::{assemble, at_fdc_write, ImageSource, RunPolicy, WriteMode};
use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(args.next().expect("media directory"));
    let value: u8 = args.next().expect("byte value").parse()?;
    let mut guest = vec![0xFA, 0xFC];
    for (port, byte) in [
        (0x3F2u16, 0x1C), (0x0A, 6), (0x0C, 0), (0x0B, 0x4A),
        (4, 0), (4, 6), (0x81, 0), (5, 0xFF), (5, 1), (0x0A, 2),
    ].into_iter().chain([5, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF].map(|b| (0x3F5, b))) {
        guest.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, byte, 0xEE]);
    }
    guest.extend([0xEC; 7]);
    guest.push(0xF4);
    let mut config = at_fdc_write(guest, vec![value; 512], 0x600);
    let base = directory.join("disk.img");
    if !base.exists() {
        std::fs::write(&base, vec![0; config.drives[0].kind.image_bytes() as usize])?;
    }
    config.drives[0].image = ImageSource::File(base);
    config.drives[0].write = WriteMode::Journal {
        path: directory.join("disk.upfj"), keep_generations: 10,
    };
    let mut machine = assemble(&config)?;
    println!("{:?}", machine.run_session(&RunPolicy::natural(Some(10_000), None))?);
    machine.finish_persistence()?;
    Ok(())
}
RS
cargo run --offline --manifest-path "$demo/Cargo.toml" -- "$demo" 17
cargo run --offline --manifest-path "$demo/Cargo.toml" -- "$demo" 34
cargo build --offline -p cli
(
  cd "$demo"
  "$repo_dir/target/debug/unemupc98" media generations --base disk.img --journal disk.upfj
  "$repo_dir/target/debug/unemupc98" media verify --base disk.img --journal disk.upfj
  "$repo_dir/target/debug/unemupc98" media rollback --base disk.img --journal disk.upfj --to 1 --backup before-rollback.upfj
  "$repo_dir/target/debug/unemupc98" media commit --base disk.img --journal disk.upfj --to 1 --output restored.img
  "$repo_dir/target/debug/unemupc98" media compact --base disk.img --journal disk.upfj --to 1 --output restored-alias.img
)
```

**MEASURED:** both guest runs print `Halted`. These excerpts were captured from
that generated-media run (all commands exited 0); `verify` prints the same
closed-generation report as `generations`:

```text
base sha256: 36bd753facc985aad613c884a2040210d208b1aa520e957b31ba2e1e19cd4185
retained anchor: 0
generation 0: immutable base, end 128, commands 0, sectors 0
generation 1: session end, end 2048, commands 1, sectors 1
generation 2: session end, end 3968, commands 1, sectors 1
recoverable-tail bytes: 0
latest reconstructible state: generation 2 (closed), end 3968
status: fully valid/closed
closed markers do not establish filesystem health or bootability
```

Rollback prints the warning and changes the report to generation 1:

```text
rollback to generation 1: later generations and any open tip will be removed
generation 1: session end, end 2048, commands 1, sectors 1
latest reconstructible state: generation 1 (closed), end 2048
status: fully valid/closed
```

The two export commands print:

```text
exported generation 1 to restored.img
exported generation 1 to restored-alias.img
```

The base stays all zeroes. Both exports contain byte 17 throughout sector 0 and
zeroes elsewhere; `before-rollback.upfj` retains the complete two-generation
journal. Keep the immutable base with that backup. To continue from the export,
select `restored.img` as the new base and choose a fresh journal path in Rust.
See [Supported host guarantees](design/persistent-media.md#supported-host-guarantees)
for the measured macOS/APFS behavior and storage assumptions.

## Typecheck / lint / format

```sh
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --locked -p cli --bin unemupc98 -- -D warnings # production, no reference-scheduler
cargo fmt --all
cargo fmt --all -- --check                   # CI check
cargo deny --all-features list               # inspect dependency licences
cargo deny --all-features check advisories licenses bans sources
```

The `deny.toml` allowlist covers the all-features graph: AGPL-3.0-or-later for
workspace crates, MIT, Apache-2.0, Unicode-3.0, BSD-2-Clause, BSD-3-Clause
and ISC for dependencies. CC-BY-SA-4.0 is allowed only for the `window` crate
(a crate-scoped exception), whose embedded font assets carry it; see
`crates/window/assets/fonts/NOTICE.md`. New licences
require review before extending it. Unknown registries and git sources are denied;
only the pinned x86jit repository is explicitly allowed. Wildcard dependencies
are denied and duplicate versions warn.

The advisory check uses the RustSec database and covers the full dependency graph.
Vulnerabilities and yanked crates fail the gate. An unmaintained crate requires
an advisory-specific ignore entry in `deny.toml` with a dated reason and a review
date; review each entry on that date and remove or renew it with an updated reason.
Review dates are a maintainer policy, not an automatic cargo-deny expiry.
The current exception is RUSTSEC-2026-0192 (`ttf-parser` 0.25.1, unmaintained,
not a known vulnerability): winit's default Wayland client-side decorations keep
window frames on GNOME, and no safe upgrade is available. Review it on 2027-05-11.
CI fetches the advisory database. With a cached database and no network, run:

```sh
cargo deny --all-features check --disable-fetch advisories licenses bans sources
```

## CI

Forgejo Actions runs `.forgejo/workflows/ci.yml` on pushes to `main` and manual
dispatches on the x86-64 runner. From the repository root, run the same gates locally:

```sh
bash scripts/ci-gate.sh devshell
bash scripts/ci-gate.sh fmt
bash scripts/ci-gate.sh guard
bash scripts/ci-gate.sh clippy
bash scripts/ci-gate.sh deny                     # advisories, licences, bans, sources; all features
bash scripts/ci-gate.sh tests
bash scripts/ci-gate.sh fixtures                 # build/hash-check BIOSes, FreeDOS, test386, XTIDE, HDD, VGA BIOS, open AT BIOS, at-pm-test, source archives
bash scripts/ci-gate.sh goldens                  # slow tests in the test profile; requires fixtures
```

Fixture-dependent tests print a skip message when a fetched vendor file is missing.
Set `UNEMUPC98_REQUIRE_FIXTURES=1` to make a missing fixture fail with its path and
fetch script instead. The `goldens` gate always sets this variable, so a failed or
incomplete fixture fetch cannot silently pass CI. The µPD765 FAT12 test follows
the same policy when `mformat` is missing and points to `nix develop`.

### User-supplied ROM tests

This local opt-in lane is never called by CI; CI never has proprietary firmware:

```sh
bash scripts/ci-gate.sh user-roms
# Equivalent command inside the devShell:
cargo test --locked --profile test --workspace --no-fail-fast -- --ignored user_rom_
```

These tests are `#[ignore]` with names starting with `user_rom_`. The shared
`assembly::require_user_firmware!(entry_id, board.firmware_catalogue())` helper
is available through assembly's `test-support` feature. It searches recursively
in `vendor/user-roms/` first, then the optional directory
`UNEMUPC98_USER_ROMS_DIR`. Set it to an absolute path to a folder of unpacked,
plain dump files, rather than a volume root or home directory. Archives such as
MAME `.zip` sets are not unpacked. Names and subdirectories do not matter;
catalogue SHA-256 identities select assembled
images or complete chip layouts. Only regular files of matching sizes are hashed,
once per canonical path per test process; keep dumps unchanged during a run.
Root, file and directory symlinks are followed; visited canonical directories
prevent loops. Earlier roots win, and parts can span both roots.
A set-but-missing, relative or non-directory external root fails configuration,
even if vendor firmware matches. Unreadable subdirectories fail scanning.
Private root names are redacted in discovery errors and display names. Use the
returned firmware's `redact`, `expect_redacted` and `expect_err_redacted` helpers
for downstream diagnostics that may embed paths.

Missing, incomplete or wrong layouts print exactly one visible line:

```text
SKIPPED: user-supplied firmware <entry id> not found (set UNEMUPC98_USER_ROMS_DIR or see vendor/user-roms/README.md)
```

If some parts match, the same line appends counts for each partial layout, such
as `; layout 0: 1 of 2 parts found`, without hashes or paths.

`UNEMUPC98_REQUIRE_USER_ROMS=1` makes absence panic with that same text;
other values leave skipping enabled. `UNEMUPC98_REQUIRE_FIXTURES` does not change
this policy. Discovery I/O errors fail with redacted diagnostics instead of skipping.
Require/skip panics point at the calling test.
The ignored `user_rom_ibm_1985_identity` checks assembled identity, per-chip hashes
and misplaced/bad-chip diagnostics through assembly and machine-file lowering,
without executing the BIOS. It accepts either the chip pair or an assembled image.
`user_rom_ibm_5170_at_pm_test_resumes_through_the_shutdown_byte` boots the
open AT protected-mode test floppy with the IBM 5170 BIOS and expects the
INT 15h AH=88h and AH=87h lines to pass and every reset to resume through
the CMOS shutdown byte (`design/at-pm-test.md`).
`user_rom_landmark_at_identity` similarly checks the Landmark AT assembled hash,
mirrored halves and per-chip hashes. The CLI test
`user_rom_landmark_at_real_mode_prefix` runs its first twelve diagnostics headlessly
through the same preset loader, assembler and session policy as `run --machine`,
with no disk and the intended SIDT stop. Adapter-gap reads now use board-owned
read-only zero mappings; its retired count and MDA plane hash retain
TODO-MEASURE sentinels until a new local run. See
`design/compatibility-diagnostics.md` §3. Run only the Landmark tests with:

```sh
cargo test --offline -p cli --test landmark -- --ignored user_rom_landmark_at_real_mode_prefix --nocapture
```

```sh
UNEMUPC98_REQUIRE_USER_ROMS=1 bash scripts/ci-gate.sh user-roms
```

See `vendor/user-roms/README.md` for catalogue hashes and the proprietary golden rule.

The CI `fixtures` gate runs the Kiselev, FreeDOS, test386, XTIDE, hard-disk,
open VGA BIOS, open AT BIOS and at-pm-test scripts below; each checks pinned
output SHA-256 hashes. All but the two Nix-built BIOSes rebuild offline when
their pinned source commits and release archives are cached (Nix caches the
VGA and AT BIOS sources and store paths); `vendor/at-pm-test/build.sh` always builds offline, assembling the
committed program source with the devShell's nasm into a 1.2M boot floppy.
FreeDOS also fixes the volume serial (`2895-9853`), label (`FREEDOS`) and all filesystem
timestamps (1980-01-01 UTC). A hash mismatch requires checking the sources and
toolchain and deliberately rebuilding the goldens' expectations before changing
the expected hash. See each `vendor/*/README.md` for cache paths and hashes.

The Kiselev, XTIDE, open VGA BIOS and open AT BIOS scripts also write the
corresponding source of their open (GPL and LGPL) images next to them, which a published firmware
package carries as its `source` layer:

| Archive | Contents | Licence |
|---|---|---|
| `vendor/kiselev-bios/kiselev-bios-a4af458-atreset-src.tar` | upstream at the pinned commit with `at-reset.patch` applied (without upstream's prebuilt `binaries/` and `xtide_r625/`), the patch, `NOTICE` | GPL-3.0-or-later |
| `vendor/xtide-universal-bios/xtide-universal-bios-r638-src.tar` | SVN r638 trunk export, `NOTICE` | GPL-2.0-only |
| `vendor/vgabios/vgabios-0.8a-unemu.2-src.tar` | the fork at the pinned revision (without `.beads/config.yaml`, the issue tracker's settings, which name a developer's own sync remote), `NOTICE` | LGPL-2.1-or-later |
| `vendor/unemu-at-bios/unemu-at-bios-1.0.2-unemu.2-src.tar` | the fork at the pinned revision (without upstream's prebuilt `binaries/` and `xtide_r625/`), `NOTICE` | GPL-3.0-or-later |

Each is an uncompressed POSIX ustar archive written by GNU tar (from the
devShell) with members sorted by name, one timestamp (the source revision's
commit time), owner and group 0 without names and modes 0644/0755, so its
bytes do not depend on the host; each script pins its SHA-256
(`SOURCE_ARCHIVE_SHA256`) and checks it. Tar rather than zip: zip entries carry
local-time DOS timestamps and per-entry extra fields; ustar carries no
atime/ctime and needs no extended headers here. No compression keeps a
compressor's version and header fields out of the bytes. The `NOTICE` is the
vendor directory's committed `source-notice.txt`; it states what the archive is,
the upstream location and revision, the licence and the commands that rebuild
the images. Before writing its archive, the Kiselev script assembles the AT
reset image from the staged tree, the open AT BIOS script runs `nix build` on
the staged tree's own flake and the XTIDE script runs upstream's `make` on a
copy of the staged trunk; each compares the result with the pinned hashes, so
an archive always builds what is shipped. BSD tar (the macOS
default outside the devShell) cannot write these archives, and the scripts stop
with a message. The XTIDE script needs the network and Subversion only when
`vendor/xtide-universal-bios/build/trunk-r638/` is not cached (it says so
before exporting); without `svn` on `PATH` it runs it with
`nix shell --no-update-lock-file --inputs-from . nixpkgs#subversion`. The CI
`fixtures` gate runs it last. The default suite checks that every archive pin
parses. The ignored archive test checks each archive's hash, top-level
entries, `NOTICE` text, absent prebuilt images (`.bin`, `.rom`) and excluded
upstream paths, and host-free metadata; the `goldens` gate runs it with
`UNEMUPC98_REQUIRE_FIXTURES=1`, so a missing archive fails there:

```sh
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test media_fixtures source_archive -- --include-ignored
```

The XTIDE Universal BIOS option-ROM fixtures:

```sh
vendor/xtide-universal-bios/fetch.sh           # r638 AT binary, padded/checksummed to 8 KiB
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p assembly --lib xub
```

It checks both the download and output SHA-256, reusing
`vendor/xtide-universal-bios/build/ide_at.bin` and the source export
`build/trunk-r638/` for offline rebuilds. The output is
`vendor/xtide-universal-bios/ide_at-r638-8k.bin`. Its catalogue entry is `Listed`;
building it does not add an ATA device or hard-disk boot profile. See
`vendor/xtide-universal-bios/README.md` for provenance, padding and checksum rules.

The open VGA BIOS and its INT 10h probe floppy are built with Nix (in the
devShell) from the sibling `vgabios` repository on this project's forge:

```sh
vendor/vgabios/fetch.sh                        # vgabios-std.bin and int10-probe-1200.img
UNEMU_VGABIOS_CHECKOUT=../vgabios vendor/vgabios/fetch.sh   # pinned revision from a local clone
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p assembly --lib open_vga
cargo test --offline -p cli --test open_vga_golden -- --include-ignored
```

The script derives the forge from this repository's `origin` remote (same host,
transport and organization, repository `vgabios`), builds release
`v0.8a-unemu.2` at its pinned revision and checks both output SHA-256 values
before replacing anything, then writes the source archive of the tree it
built from. `UNEMU_VGABIOS_FLAKE` overrides the whole flake
reference and `--print-flake` shows the one it would build. A clone from a mirror
derives the mirror's address, so the mirror must publish `vgabios` next to this
repository. In CI the origin is the checkout's HTTPS URL without credentials:
the `fixtures` gate builds the VGA BIOS only while `vgabios` is readable
anonymously, and otherwise fails with a message naming `UNEMU_VGABIOS_FLAKE`.
The open VGA tests then do not skip: the `goldens` gate sets
`UNEMUPC98_REQUIRE_FIXTURES=1`, so a missing ROM or probe fails them. Locally,
without that variable, they print a skip line. See `vendor/vgabios/README.md`
for pins, licence and the later registry delivery. `machines/at-games-open.toml`
needs the ROM.

The open AT BIOS, the unemu fork of Kiselev 8088_bios, is built the same way
from the sibling `unemu-at-bios` repository:

```sh
vendor/unemu-at-bios/fetch.sh                  # bios-unemu-at.bin and its source archive
UNEMU_AT_BIOS_CHECKOUT=../unemu-at-bios vendor/unemu-at-bios/fetch.sh   # pinned revision from a local clone
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p assembly --lib unemu_at_bios
```

It builds release `v1.0.2-unemu.2` at its pinned revision (`#unemu-at`),
checks the ROM's SHA-256, then writes the source archive. The overrides are
`UNEMU_AT_BIOS_FLAKE` and `UNEMU_AT_BIOS_CHECKOUT`, and `--print-flake` shows
the reference; in CI it builds only while `unemu-at-bios` is anonymously
readable next to this repository. `machines/at-games-open.toml`,
`machines/at-ide.toml` and the `open-at` templates need the ROM. See
`vendor/unemu-at-bios/README.md`.

The SeaBIOS ISA fixture uses its own firmware devShell, separately from the CI
`fixtures` gate:

```sh
nix develop .#firmware --command bash vendor/seabios/fetch.sh
```

It exports pinned rel-1.17.0 from `vendor/seabios/src/` (offline when cached),
checks input hashes and builds `vendor/seabios/bios-1.17.0-isa.bin` with fixed
version metadata. The output SHA-256 is pinned after two matching independent
builds on 2026-11-11. No image is installed before verification. No board or
catalogue entry is added; that entry waits for `generic-386-isa` and the x86jit
386 CPU model.
See `vendor/seabios/README.md` for tools, configuration and provenance.

The hard-disk data fixture builds without downloads or cached inputs:

```sh
vendor/hdd/build.sh                          # partitioned FAT16 data disk; pinned SHA-256
mdir -i vendor/hdd/fat16.img@@8704 ::          # UNEMUHDD, MARKER.TXT (29 bytes, 1980-01-01)
```

It uses mtools 4.0.49 with a fixed serial, label and FAT-epoch timestamps.
See `vendor/hdd/README.md` for the IBM AT type-2 geometry, partition/BPB layout,
hash and licence. The CI `fixtures` gate builds it.

Each gate uses `nix develop --no-update-lock-file --command` (except `guard`, which
runs directly), preserves its exit status, and tees combined output to the gitignored
`ci-logs/<step>.log`. Set `CI_LOG_DIR` to choose another log directory. Cargo builds/tests
use `--locked`; the clippy gate lints all features and then the production CLI
binary without dev-dependency feature unification; formatting uses `cargo fmt --check`. CI runs every gate even after failures,
saves the Cargo cache, and reports failing steps in commit status `ci / x86_64`.
See [`dev-setup.md`](dev-setup.md#ci) for the runner and cache setup.

## x86jit (CPU core)

```sh
# develop against a local x86jit checkout: a gitignored override, never a path dep in Cargo.toml
cat > .cargo/config.toml <<'TOML'
[patch."<the git URL x86jit-core is pinned to in Cargo.toml>"]
x86jit-core = { path = "<your x86jit checkout>/x86jit-core" }
TOML
git -C <your x86jit checkout> rev-parse HEAD    # must match the rev in Cargo.lock for goldens to be valid
```

A change the machine needs from the CPU lands in x86jit under its own beads and gate; this repo
then re-pins and re-runs the slow goldens. See `dev-setup.md` § CPU core.

## Guest test-ROMs

```sh
nasm -f bin boot.asm -o boot.bin             # flat binary for the guest
# (or nasm -f elf + ld + objcopy for larger programs)
```

## Pre-commit

```sh
pre-commit install                                  # one-time, per clone
pre-commit run --all-files                          # run active hooks
pre-commit run --all-files --hook-stage manual      # run staged-as-manual hooks too
```

## beads (`bd`) — open work

```sh
bd memories                                      # the shared working rules — read first
bd ready --exclude-type epic                     # unblocked work, highest priority first
bd list --status in_progress                     # what is claimed
bd show <id>                                     # one bead
bd list --type epic                              # the themes
bd create --title "<t>" --description "<d>" --acceptance "<ac>" --type task --priority P2 \
          --parent <epic-id> --no-inherit-labels --labels <labels>
bd update <id> --claim                           # before writing code
bd update <id> --append-notes "<progress>"       # never --notes on an existing bead
bd close <id> --reason "<commit> — <what landed>"
bd list --external-ref backlog:TASK-41           # the bead an old task id became
```

Details: [`working-on-tasks.md`](working-on-tasks.md).

## Backlog.md — docs and decisions only

```sh
backlog doc create "<title>"                      # → docs/doc-N - Title.md
backlog decision create "<title>"                 # → decisions/decision-N - Title.md
```

Never `backlog task create`: tasks moved to beads.
