# Commands

unemu maintains application commands and gates; [unemupc](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/commands.md) maintains core runners and hardware fixture commands.

Everyday commands for this project. Keep this file **flat and copy-pasteable** — agents and humans both grep it.

## Front end: window and command words

`unemu` opens the library with no arguments. Desktop options (`--machine FILE`,
`--open FILE`, `--display CARD`) select the window; a first argument that is a
word selects the command line (`image`, `media`, `recipe`, `library`, `run`).
Unknown words receive the command-line usage error (exit 2). `--help` lists
both entry points; `--version` and `--about` print the same front-end notice.
Source commands use `cargo run --offline -p window --bin unemu -- …`.
The `upcli` executable has been removed; its composition library keeps that
package name internally.

On headless Linux the release binary still links ALSA: install `libasound2`
(Ubuntu 22.04) or the distribution's equivalent runtime package. No display or
audio device is opened by command words. ASSUMED: without `libasound.so.2`,
the Linux loader refuses even `unemu image ls` before Rust runs. The release
workflow tests this in a bare Ubuntu container, then installs the runtime and
runs `image ls`, `--version` and `media create`. The sandbox cannot run that container;
no headless Cargo feature is added. `unemupc` remains toolkit-free.

## PC fixtures for application commands

Run these commands in the Nix devShell before the examples that use PC firmware
or machine files:

```sh
bash scripts/core-checkout.sh
fixture=$(bash scripts/core-checkout.sh --print-root)
export "$fixture"
```

`--print-root` verifies the pinned checkout and prints the assignment
`UNEMUPC_FIXTURE_ROOT=<absolute checkout path>`. Core files below are read from
that root. This repository keeps its own recipe fixtures under `vendor/freedos`,
`vendor/kenslab` and `vendor/shareware`.

## Local machine images

```sh
cargo run --offline -p window -- image import "$UNEMUPC_FIXTURE_ROOT/machines/at-freedos.toml" --store /path/to/store --tag dos-lab:1.0 --create
cargo run --offline -p window -- image import /path/to/machine.yaml --store /path/to/store --tag dos-lab:recovered --recover medium-0=tip
cargo run --offline -p window -- image import /path/to/machine.toml --store /path/to/store --tag dos-lab:checkpoint --recover medium-0=generation:1
cargo run --offline -p window -- image import /path/to/machine.toml --store /path/to/store --tag dos-lab:1.1 --min-app-version 0.0.3
cargo run --offline -p window -- image compress dos-lab:1.0 --tag dos-lab:compressed --store /path/to/store
cargo run --offline -p window -- image ls --store /path/to/store
cargo run --offline -p window -- image sweep --store /path/to/store
cargo run --offline -p window -- image run dos-lab:1.0 --store /path/to/store --workspace play --max-steps 1000000
cargo run --offline -p window -- image run dos-lab:1.0 --store /path/to/store --workspace disposable --new --max-steps 1000000
cargo run --offline -p window -- image workspace ls --store /path/to/store
cargo run --offline -p window -- image workspace abandon-publication play --store /path/to/store
cargo run --offline -p window -- image publish --from-workspace play --store /path/to/store --tag dos-lab:flat --flatten
cargo run --offline -p window -- image publish --from-workspace play --store /path/to/store --tag dos-lab:1.1
cargo run --offline -p window -- image workspace discard play --store /path/to/store --yes
cargo run --offline -p window -- image machine ls --store /path/to/store
cargo run --offline -p window -- image machine new dos-lab:1.0 --store /path/to/store --name "DOS games"
cargo run --offline -p window -- image machine rename dos-games "Keen" --store /path/to/store
cargo run --offline -p window -- image machine duplicate dos-games --store /path/to/store --name "Keen copy"
cargo run --offline -p window -- image machine settings dos-games --store /path/to/store --set memory.extended_kib=3072 --dry-run
cargo run --offline -p window -- image machine settings dos-games --store /path/to/store --set memory.extended_kib=3072
cargo run --offline -p window -- image machine settings dos-games --store /path/to/store --clear
cargo run --offline -p window -- image machine run dos-games --store /path/to/store --max-steps 1000000
cargo run --offline -p window -- image machine export dos-games --store /path/to/store
cargo run --offline -p window -- image machine export dos-games --format raw --slot C: --to exported --store /path/to/store
cargo run --offline -p window -- image machine commit dos-games --store /path/to/store --tag dos-games:1.0.0
cargo run --offline -p window -- image machine commit dos-games --store /path/to/store --tag dos-games:1.1.0 --title "DOS games" --keep-on-old-image
cargo run --offline -p window -- image machine reset dos-games --store /path/to/store --yes
cargo run --offline -p window -- image machine rm dos-games --store /path/to/store --yes
cargo run --offline -p window -- image media import /path/to/disks --title "Installation set" --tag private/media/ibm-pc/install:1.0-r1 --store /path/to/store --create
cargo run --offline -p window -- image media import /path/to/disk1.img /path/to/disk2.img --title "Installation set" --tag private/media/ibm-pc/install:2.0-r1 --label '1=Setup' --extra /path/to/manual.txt:MANUAL.TXT --store /path/to/store
cargo run --offline -p window -- image media import /path/to/disc.iso --title "Optical set" --tag private/media/ibm-pc/optical:1.0-r1 --format 1=iso-2048 --store /path/to/store
cargo run --offline -p window -- image media import /path/to/game.img --title "Game" --tag private/media/ibm-pc/game:1.0-r1 --need cpu=286+ --need memory=640K --need video=ega,vga --need sound=adlib,pc-speaker,none --requirements "IBM AT or compatible; 640K; EGA or VGA" --work game --synopsis "What the game is." --work-id mobygames=game --store /path/to/store
cargo run --offline -p window -- image media import vendor/shareware/wolf3d/wolf3d-sw14-1440.img --title "Wolfenstein 3-D: Episode 1 (shareware 1.4)" --tag private/media/ibm-pc/wolf3d-shareware:1.4-r1 --extra vendor/shareware/wolf3d/extras/VENDOR.DOC:VENDOR.DOC --extra vendor/shareware/wolf3d/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1992 --genre "first-person shooter" --platform DOS --description 'Wolfenstein 3-D, episode 1, Escape from Wolfenstein, version 1.4, the shareware episode of a first-person shooter with texture-mapped VGA graphics by id Software, published by Apogee Software. As B.J. Blazkowicz the player escapes from Castle Wolfenstein through ten floors, one of them secret. It needs a 286 or better. The disk holds the files of the original Apogee shareware archive 1wolf14.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-vendor-v11.01.92 --license-text VENDOR.DOC --store /path/to/store --create
cargo run --offline -p window -- image media ls --store /path/to/store
cargo run --offline -p window -- image media show private/media/ibm-pc/install:1.0 --store /path/to/store
cargo run --offline -p window -- image push private/media/ibm-pc/wolf3d-shareware:1.4-r1 unemu/media/ibm-pc/wolf3d-shareware:1.4-r1 --store /path/to/store --dry-run
```

Machines (machine-images §3.2) are named local instances of images: the
desktop's Machines list on the command line. Each has a fixed id (its
workspace, a slug of the first name; never starting `regular-`, `gaming-`,
`disposable-` or `fresh-`, which get `m-` in front) and a free-text name,
1–64 characters, unique ignoring case; `new` defaults the name to the image
title, then `title (2)`. Except for the read-only `export`, every `image machine` command first adopts older
workspaces as machines and reports each on the error stream as
`migrated:ID`, and each workspace it leaves alone because it is damaged as
`migration-skipped:NAME REASON`: a desktop `regular-…` workspace takes its
image's title and a new id, a workspace named on the command line keeps its
name, and Gaming-mode workspaces stay. `ls` prints
`ID NAME SOURCE DIGEST STATE last-run:… changes:BYTES` with `detached`,
`tag-moved:DIGEST`, `insert-pending:DRIVE`, `publication-pending` and
`problem:…` flags, tab-separated; STATE is `Never started`, `Same as image`,
`Changed: A:, C:, inserted media, CMOS, settings` or `Running`. `settings`
prints the hardware-table diff from the image (`[table]`, `-old`, `+new`) and
the CMOS bytes the change rewrites (`cmos:17h old:00 new:0C …`); `--set`
takes a dotted path into the tables and a TOML value (other text is a
string), `--clear` drops the overlay and returns to the image's settings, a
`cmos-note:` line explains stored CMOS bytes kept because SETUP values
cannot be derived, and the change applies at the machine's next start. `commit` (Save as new image…) moves the machine onto
the new image with fresh journals unless `--keep-on-old-image`; its title
defaults to the machine name, it warns on the error stream about writes to
disks inserted from installation sets (never part of an image), and, like
`image publish`, it moves no existing non-version tag (there is no
`--expect DIGEST`). `reset` and `rm` print the machine and refuse
without `--yes`; neither deletes the image. A running machine refuses every
change but `rename`; a machine whose migration move is still pending refuses
until its old desktop workspace is free. `run` records the last run and drops
the pending inserts only once the machine has assembled. `image run NAME:TAG
--workspace ID` on a machine's id starts it as the machine, refusing a tag
that names another image; `image publish --from-workspace ID` and
`image workspace discard ID` refuse a machine's id and name the machine
command to use.

### Exporting a machine's next-run medium

`unemu image machine export ID [--store DIR]` lists the formats advertised by
the selected core (`id`, display name, extension, scope, availability), followed
by its configured slots (`label`, kind, byte length, detail), tab-separated.
Detail is the core's verbatim display text; an unknown length is printed as
`unknown`. A core without media descriptions falls back to the store's baseline
slot listing. Unknown scopes and formats without an available slot query are
unavailable. Use the IDs printed by `unemu image machine ls`.

Export with `--format FMT --to DIR`. Slot formats also require `--slot NAME`;
machine formats reject `--slot` and export exactly the core's required slot set.
The pinned PC core advertises `raw` (`.img`, slot), `vhd-fixed` (`.vhd`, fixed
slot), and `machine` (`.toml` plus `.img` media, machine). The command discovers
these through the core contract. The old `--slot NAME --to FILE` spelling has
been removed; use `--format raw --slot NAME --to DIR` instead.

The selected media are what `image machine run` would mount on its next run,
including pending inserts. Export retains workspace locks for the whole call,
reads bases, layers and journals through the existing overlay interface, and
verifies full base SHA-256 and length, including large disks. Pending inserts
remain pending. Desktop kept-catalogue selections are ignored, as by the CLI run.

Outputs use the core's logical names inside `DIR`, which is created if needed.
Each output is printed as `NAME bytes:N sha256:HEX`, tab-separated, followed by
every core report note as `note:TEXT`. Machine documents refer to the exported
images by those relative logical names. They may retain firmware bindings;
notes explain omitted or changed settings and content. Firmware is not copied.

Export requires a stopped machine. It never recovers journals, migrates the
store or writes caches. Destinations inside the store are refused. Every output
is streamed and hashed in an exclusively created temporary file beside the
outputs (`.NAME.unemu-export-PID-N.part`), then flushed, synced and closed to
further writes; their handles stay open until publication or cleanup.
Only after the core export succeeds and its report matches the sink's receipts
are these files hard-linked to their final names, without replacing existing
entries, and the temporary names removed. On Unix, all file operations and the
directory sync use one held directory handle; replacing `DIR` during export
does not redirect writes; store exclusion walks the opened directory's parents,
and each temporary name must match its held file's device and inode before
linking or unlinking. Windows uses paths and does not sync directories.
The output directory belongs to the command's user: export protects against
accidents and concurrent unemu processes by never overwriting an existing name,
removing a file it did not create, writing inside the store or leaving temporary
files behind; hostile same-user entry swaps during export are outside the model.
Existing files, including temporary files from earlier attempts, are never
overwritten or removed. A core failure or inconsistent report removes only
this attempt's temporary files and publishes nothing. If publication fails
part-way, already published finals remain; the error names them and the failed
output, and the remaining temporary files are removed. Final names are never
removed, including after an error printing the report. Parser and contract
usage/config errors exit 2; unsupported operations, content failures and storage
failures exit 1 with the diagnostic. Without `--store`, the command uses the
executable's default store.

Stored-image runs (`image run` and `image machine run`) start through the
core contract in the platform-neutral `cli-core` runner. The binary supplies
its compiled-in cores; platform hooks select and own the pinned session, prepare
the launch and confirm pending inserts after creation. The PC hook also keeps
failed-run CMOS/journal persistence and the existing diagnostic chain. Platforms
without a stored-image runner refuse the command explicitly.
Their last output field is its stop: `Steps`, `Halted` or
`PoweredOff`, replacing the old `RunOutcome` debug text (for example,
`RetiredCapReached` becomes `Steps`). A guest-requested reset is performed
inside the core and reported on standard error; the old assembly loop also
continued after a reset.
There is no idle cutoff: a guest waiting with interrupts enabled advances to
`--max-steps` rather than stopping after 1,000,000 idle clocks. The PC's step
budget includes time advanced while halted, as before. Machine messages and
finish warnings go to standard error; journals finish before the result line.
The workspace pin, pending-insert confirmation, journal paths and format stay
the same. `run --machine FILE` keeps its existing policy.

Compression policy (store side). Machine imports, media-set imports and
workspace publication compress payloads of at least 4 KiB with single-threaded
zstd level 3 when the provider declares the `+zstd` type: disk bases, sector
packs, floppies, firmware, and media-set floppy/optical/extra payloads. Metadata,
sealed layer TOML, CMOS and tiny blobs stay raw. Add `--no-compress` to either
import command for raw fixtures. Compressed descriptors carry
`unemu.decoded.digest` and `unemu.decoded.size`; OCI `digest` and `size` name the
compressed bytes, and frames record their content size. The encoder, and the
rules that choose which payloads it compresses, are frozen as Zstandard encoder
profile 1 (the `image_store::codec` documentation, machine-images §2): the same
input always gives the same stored bytes, and recipe locks pin them. An encoder
that writes other bytes is a new profile beside it, never an update of profile
1; it would change blob and image digests, never machine digests.

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
`image-store` fails if another libzstd release is linked; a system libzstd of
the same release passes it. The real guard is the profile 1 test vectors in
`crates/image-store/tests/zstd_profile.rs`, which fail for any library or
setting whose frames differ, and also fail if `Cargo.lock` resolves another
version or source of the three crates. Media-set compression is limited by
declared decoded size: floppies 2880 KiB, optical images 900 MiB, extras 2 GiB;
larger payloads stay raw. These limits and the 4 KiB threshold are part of
profile 1.

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
control characters and are limited to 128 characters. Every layer carries its
export file name in `org.opencontainers.image.title`, the name a generic OCI
client such as ORAS gives the pulled file (design/machine-images.md §2, Export
file names): each disk and extra its source's file name as given, without the
folder, and the metadata `metadata.json`; a layer stored compressed has
`.zst` after the name (`x86BOOT.img.zst`), so `zstd -d` on the pulled file
gives back `x86BOOT.img`. A file name that macOS, Linux or Windows would not
take as it is (a control character, one of `/ \ : * ? " < > |`, a trailing
space or dot, a device name Windows reserves such as `CON.img`, more than 251
bytes, which leaves room for `.zst`), a name the export writes itself
(`config.toml`, `metadata.json`, `manifest.json`, `SHA256SUMS`,
`EXPORT-VERSION`), or a name another file of the set already has, ignoring
case (with or without `.zst`), is refused naming both files; rename the file.
A file listed twice is two disks with one layer, and different files with
identical bytes are one layer under the first file's name. A tag that holds
the same files under other names, or without names as earlier releases stored
sets, is a conflict that says so. All inputs are checked
before streaming any source. Sources stay
unchanged and every copy is streamed with full SHA-256. Existing tags fail before
reading sources; final publication uses compare-and-swap. All descriptors default
to user-supplied. Optical storage does not enable CD-ROM emulation.

Set-level catalogue metadata fills the set's one title record in its metadata
blob ([`design/machine-images.md`](design/machine-images.md) §2.1), the fields
the library's Import installation media… dialog writes plus developers, the
version and the edition: `--publisher P`, `--developer D` and `--genre G` may
be repeated and keep the order given; `--year YYYY` is a whole number from 1 to
9999; `--platform` is the software platform, spelled exactly as one of `DOS`,
`Windows 3.x`, `Windows 9x`, `CP/M`, `MSX`, `ZX Spectrum`, `C64`,
`Atari 8-bit`, `Atari ST` or `other` (not the machine platform `ibm-pc`);
`--description TEXT` is free text; `--version V` and `--edition E` name the
release the set holds and its edition, once each (the FreeDOS sets below: `1.4`
and `Floppy Edition`). `--set-label L` may be repeated and gives the set its
labels, as the dialog's Labels field does, kept in the order given, so the same
labels in another order are another set digest; a repeated label is refused,
and a label, like a disk label, rejects control characters and is limited to
128 characters. Values are trimmed and must not be blank. The year and platform
rules are the dialog's, checked in the shared import both use. Without these
flags the metadata, and so the set digest, are unchanged. The preview and
`image media show` print one `publisher`, `developer` or `genre` line per
value, `year`, `platform`, `description`, `version` and `edition` lines for
the fields given, and one `set-label` line per label. The metadata is
generated content: in an open set it is CC0-1.0
like the config. It is part of the set: other metadata is another set digest.
The manifest repeats the record in the OCI image-spec's standard annotations,
which registry web UIs show: `org.opencontainers.image.title`, `.description`
(the description), `.vendor` (the publishers) and `.authors` (the
developers), lists joined with `, ` in the order given. A field not given adds
no annotation, so a set without the flags keeps its digest; there is never a
creation time.

Five more flags fill the fields of metadata schema 2
([`design/machine-images.md`](design/machine-images.md) §2.1): `--need
KIND=VALUE[,VALUE]...` states one kind of feature the title needs and its
alternatives, best first (`--need cpu=286+ --need memory=640K --need
sound=sb,adlib,pc-speaker,none --need video=vga`), may be repeated once per
kind, and is checked against the `ibm-pc` needs vocabulary when the set is
planned: an unknown kind or value is refused before anything is written,
naming what is known (`cpu` `8088`, `286`, `386`, `486`, `pentium`, each with
an optional `+` for "or later"; `fpu` `none`, `287`, `387`; `video` `mda`,
`hercules`, `cga`, `ega`, `mcga`, `vga`, `svga`; `sound` `pc-speaker`,
`tandy`, `adlib`, `sb`, `sbpro`, `sb16`, `gus`, `mt32`; `memory`, `ems` and
`xms` as one minimum such as `640K` or `4M`; `input` `mouse`, `joystick`;
`none` in every kind but `cpu` and the quantities). `--requirements TEXT` is
the box's own words about the machine it wants, shown, never evaluated.
`--work ID` names the work the set is a release of, and must equal the last
component of the tag's repository (`--work contra` with
`private/media/ibm-pc/contra:europe-r1`): the releases of a work are tagged in
the repository named after it. `--synopsis TEXT` says what the work is, and
`--work-id CATALOGUE=ID` gives the work's id in an outside catalogue,
`wikidata`, `mobygames` or `igdb`, once per catalogue. Values are trimmed and
must not be blank, except a need's, which take no spaces; the requirements and
the synopsis take no control characters. A refusal names the field and its
rule. Any of these makes the
metadata schema 2, which release 0.0.2 shows read-only; without them the
metadata stays schema 1 and the set digest is unchanged. The needs are also
written to the manifest annotation `unemu.needs` in their compact form
(`cpu=286+;memory=640K;sound=sb,adlib,pc-speaker,none;video=vga`), so a
registry browser can warn without fetching the metadata. The preview and
`image media show` print a `requirements` line, one `need KIND
VALUE,VALUE` line per kind in kind order, `work` and `synopsis` lines and
one `work-id KIND "ID"` line per kind, after `edition`.

The command line reproduces the FreeDOS 1.4 Floppy Edition sets that the
software repository (<https://github.com/unemu-org/software>) builds and
publishes as `unemu/media/ibm-pc/freedos:1.4-r1` (the edition's own 1.44M
disks), `:1.4-1.2m-r1` and `:1.4-720k-r1`, with the release aliases `:1.4`,
`:1.4-1.2m` and `:1.4-720k`, byte for byte, with the same set digests: these
are the arguments its publish step passes (`lib/manifest.py sets`), and
`pc_image::template::build_media`, the local build the tests use, gives the
same sets. A test imports each set both ways and compares them, and another
pins the published digests. The sets published so far name the platform by
its retired id `ibm-pc-at`, under `unemu/media/freedos` without the
platform and with the first packaging as `:1.4`, and carry no export file
names; the pins are those of their `ibm-pc` rebuilds with a file name on
every layer, which replace them under the new names before the first
public release (decision-32, decision-35, decision-37, decision-38). Fetch the fixtures
with `bash vendor/freedos/fetch-floppy-1.4.sh` and import into a store that
does not hold these tags yet (the command line never moves a tag; the store
moves each release alias forward with its revision). Each command
states the disks in installer order with their labels and format, the licence
index and the source archive as extras with their DOS names, the title record
(the work's title, `FreeDOS`, with the release and edition in fields of their
own), the set labels and the open statement. Each extra's kind comes from its
extension. To use the published sets, pull them instead (Official template
images, below).

```sh
cargo run --offline -p window -- image media import vendor/freedos/floppy-1.4/144m/x86BOOT.img vendor/freedos/floppy-1.4/144m/x86DSK01.img vendor/freedos/floppy-1.4/144m/x86DSK02.img vendor/freedos/floppy-1.4/144m/x86DSK03.img vendor/freedos/floppy-1.4/144m/x86DSK04.img vendor/freedos/floppy-1.4/144m/x86DSK05.img vendor/freedos/floppy-1.4/144m/x86DSK06.img --title FreeDOS --tag unemu/media/ibm-pc/freedos:1.4-r1 --label '1=Disk 1 of 7 - boot disk and installer (x86BOOT.img)' --format 1=1.44M --label '2=Disk 2 of 7 - packages (x86DSK01.img)' --format 2=1.44M --label '3=Disk 3 of 7 - packages (x86DSK02.img)' --format 3=1.44M --label '4=Disk 4 of 7 - packages (x86DSK03.img)' --format 4=1.44M --label '5=Disk 5 of 7 - packages (x86DSK04.img)' --format 5=1.44M --label '6=Disk 6 of 7 - packages (x86DSK05.img)' --format 6=1.44M --label '7=Disk 7 of 7 - packages (x86DSK06.img)' --format 7=1.44M --extra vendor/freedos/floppy-1.4/LICENSES.TXT:LICENSES.TXT --extra vendor/freedos/floppy-1.4/freedos-1.4-floppy-src.tar:FD14SRC.TAR --publisher 'The FreeDOS Project' --developer 'The FreeDOS Project' --year 2025 --genre 'operating system' --platform DOS --description 'The official FreeDOS 1.4 Floppy Edition on seven 3.5-inch 1.44M diskettes, unmodified: the boot disk with the floppy installer, then six disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.' --version 1.4 --edition 'Floppy Edition' --set-label freedos --set-label open-source --set-label operating-system --open --license 'GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND LicenseRef-FreeDOS-other' --license-text LICENSES.TXT --source freedos-1.4-floppy-src.tar --store /path/to/store --create
cargo run --offline -p window -- image media import vendor/freedos/floppy-1.4/120m/x86BOOT.img vendor/freedos/floppy-1.4/120m/x86DSK01.img vendor/freedos/floppy-1.4/120m/x86DSK02.img vendor/freedos/floppy-1.4/120m/x86DSK03.img vendor/freedos/floppy-1.4/120m/x86DSK04.img vendor/freedos/floppy-1.4/120m/x86DSK05.img vendor/freedos/floppy-1.4/120m/x86DSK06.img vendor/freedos/floppy-1.4/120m/x86DSK07.img --title FreeDOS --tag unemu/media/ibm-pc/freedos:1.4-1.2m-r1 --label '1=Disk 1 of 8 - boot disk and installer (x86BOOT.img)' --format 1=1.2M --label '2=Disk 2 of 8 - packages (x86DSK01.img)' --format 2=1.2M --label '3=Disk 3 of 8 - packages (x86DSK02.img)' --format 3=1.2M --label '4=Disk 4 of 8 - packages (x86DSK03.img)' --format 4=1.2M --label '5=Disk 5 of 8 - packages (x86DSK04.img)' --format 5=1.2M --label '6=Disk 6 of 8 - packages (x86DSK05.img)' --format 6=1.2M --label '7=Disk 7 of 8 - packages (x86DSK06.img)' --format 7=1.2M --label '8=Disk 8 of 8 - packages (x86DSK07.img)' --format 8=1.2M --extra vendor/freedos/floppy-1.4/LICENSES.TXT:LICENSES.TXT --extra vendor/freedos/floppy-1.4/freedos-1.4-floppy-src.tar:FD14SRC.TAR --publisher 'The FreeDOS Project' --developer 'The FreeDOS Project' --year 2025 --genre 'operating system' --platform DOS --description 'The official FreeDOS 1.4 Floppy Edition on eight 5.25-inch 1.2M diskettes, unmodified: the boot disk with the floppy installer, then seven disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.' --version 1.4 --edition 'Floppy Edition' --set-label freedos --set-label open-source --set-label operating-system --open --license 'GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND LicenseRef-FreeDOS-other' --license-text LICENSES.TXT --source freedos-1.4-floppy-src.tar --store /path/to/store --create
cargo run --offline -p window -- image media import vendor/freedos/floppy-1.4/720k/x86BOOT.img vendor/freedos/floppy-1.4/720k/x86DSK01.img vendor/freedos/floppy-1.4/720k/x86DSK02.img vendor/freedos/floppy-1.4/720k/x86DSK03.img vendor/freedos/floppy-1.4/720k/x86DSK04.img vendor/freedos/floppy-1.4/720k/x86DSK05.img vendor/freedos/floppy-1.4/720k/x86DSK06.img vendor/freedos/floppy-1.4/720k/x86DSK07.img vendor/freedos/floppy-1.4/720k/x86DSK08.img vendor/freedos/floppy-1.4/720k/x86DSK09.img vendor/freedos/floppy-1.4/720k/x86DSK10.img vendor/freedos/floppy-1.4/720k/x86DSK11.img --title FreeDOS --tag unemu/media/ibm-pc/freedos:1.4-720k-r1 --label '1=Disk 1 of 12 - boot disk and installer (x86BOOT.img)' --format 1=720K --label '2=Disk 2 of 12 - packages (x86DSK01.img)' --format 2=720K --label '3=Disk 3 of 12 - packages (x86DSK02.img)' --format 3=720K --label '4=Disk 4 of 12 - packages (x86DSK03.img)' --format 4=720K --label '5=Disk 5 of 12 - packages (x86DSK04.img)' --format 5=720K --label '6=Disk 6 of 12 - packages (x86DSK05.img)' --format 6=720K --label '7=Disk 7 of 12 - packages (x86DSK06.img)' --format 7=720K --label '8=Disk 8 of 12 - packages (x86DSK07.img)' --format 8=720K --label '9=Disk 9 of 12 - packages (x86DSK08.img)' --format 9=720K --label '10=Disk 10 of 12 - packages (x86DSK09.img)' --format 10=720K --label '11=Disk 11 of 12 - packages (x86DSK10.img)' --format 11=720K --label '12=Disk 12 of 12 - packages (x86DSK11.img)' --format 12=720K --extra vendor/freedos/floppy-1.4/LICENSES.TXT:LICENSES.TXT --extra vendor/freedos/floppy-1.4/freedos-1.4-floppy-src.tar:FD14SRC.TAR --publisher 'The FreeDOS Project' --developer 'The FreeDOS Project' --year 2025 --genre 'operating system' --platform DOS --description 'The official FreeDOS 1.4 Floppy Edition on twelve 3.5-inch 720K diskettes, unmodified: the boot disk with the floppy installer, then eleven disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.' --version 1.4 --edition 'Floppy Edition' --set-label freedos --set-label open-source --set-label operating-system --open --license 'GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND LicenseRef-FreeDOS-other' --license-text LICENSES.TXT --source freedos-1.4-floppy-src.tar --store /path/to/store --create
```

A redistributable set (decision-26 amendment of 2026-11-15, for example a
shareware release fetched under `vendor/shareware/` below: the disk image and
the vendor's terms text in `extras/`) is open only by an explicit statement:
`--open --license LicenseRef-NAME --license-text EXTRA`. `--open` needs both
flags and each flag needs `--open`. The licence must be one SPDX `LicenseRef-`
(the vendor's own terms; `MIT`, `GPL-2.0-only` or any other SPDX List
identifier is refused unless the set also names its source archive, below),
and `--license-text` names, by file name, exactly one
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

A free-software set is opened under an SPDX licence expression with its
source: `--open --license EXPRESSION --license-text EXTRA --source EXTRA`.
`--source` names, by file name, exactly one `--extra` file, the archive of
the set's corresponding source, and needs `--open`; an expression that is not
one `LicenseRef-` is refused without it (copyleft or not), and a `LicenseRef-`
statement may name one too. Every medium and extra then also links the
archive's digest (`unemu.source`); the preview prints `source "FILE"` after
the `open` line and `image media show` prints `source "FILE" DIGEST` after
the `terms` lines. The push guard lets a set whose licence has a GPL, LGPL,
AGPL or MPL part into `unemu/`, or onto a registry not marked private, only
when every such medium and extra links a source archive that travels in the
set; otherwise `show` reports `private registries outside unemu/ only: open
copyleft media without the set's source archive` (machine-images §5.2).

Firmware packages use the same store, with the shared default data root when
`--store` is omitted:

```sh
cargo run --offline -p window -- image firmware import /path/to/roms --store /path/to/store --create --dry-run
cargo run --offline -p window -- image firmware import /path/to/even.bin /path/to/odd.bin --store /path/to/store
cargo run --offline -p window -- image firmware import /path/to/roms --rescan --for ibm-5170-bios-1985-06-10 --extra /path/to/manual.pdf --store /path/to/store
cargo run --offline -p window -- image firmware import /path/to/roms --for kiselev-xi8088-1.0.2-atreset --source "$UNEMUPC_FIXTURE_ROOT/vendor/kiselev-bios/kiselev-bios-a4af458-atreset-src.tar" --label open-source --label xi8088 --store /path/to/store
cargo run --offline -p window -- image firmware import /path/to/rom.bin --unlisted my-option-rom --role option-rom --open --license MIT --built-from "example/option-rom" --revision r7 --store /path/to/store
cargo run --offline -p window -- image firmware edit private/firmware/ibm-pc/kiselev-xi8088-1.0.2:r1 --source "$UNEMUPC_FIXTURE_ROOT/vendor/kiselev-bios/kiselev-bios-a4af458-atreset-src.tar" --store /path/to/store
cargo run --offline -p window -- image firmware ls --store /path/to/store
cargo run --offline -p window -- image firmware show private/firmware/ibm-pc/ibm-5170-bios-1985-06-10:r1 --store /path/to/store
```

Firmware import recursively enumerates files in stable name order (32 directory
levels and 10,000 entries maximum; hitting a limit imports no candidates), matches full
SHA-256 and length against the catalogue and verifies reconstructed chip sets.
It reports unmatched/unreadable inputs and incomplete sets with missing sockets;
those inputs are never published. Multiple complete layouts require an explicit
file list containing one layout; ambiguous catalogue identities require `--for`.
Tags are `private/firmware/ibm-pc/<catalogue-id>:r1`, the first revision
(recipes.md §8.3), open or not: a local import never takes the official
`unemu/…` name a pull of the published package writes, which only the
template builders write. When the store already holds the catalogue entry's
first revision under `unemu/` and no `private/` one, an import checks that
one instead (already imported, or a conflict). The guard, not the name,
decides where a push may go, and `image push LOCAL TARGET` names any target.
A package has no release alias unless its tag names a release
(`2x32k-r1`). `image firmware edit` publishes the next revision (`:r2`); a
package an earlier release imported as `firmware/<catalogue-id>:1` keeps that
tag, and its edit goes to `:r2`. A package under the tag with the same ROM
and document bytes reports already imported whatever its metadata (an import
whose own statement would change it is refused and names `image firmware
edit`), while other bytes conflict before copying, with CAS at publication. `--rescan` bypasses reusable persistent ROM hashes. `--dry-run`
previews without writing blobs/tags or creating a store. Extras require
`--for CATALOGUE-ID` or `--unlisted ID`; repeat `--extra PATH[:DOSNAME]` using the media-set attachment
schema and DOS naming rules above, each optionally followed by
`--extra-license SPDX` to mark that document open. Sources remain unchanged.

Every layer carries its export file name in `org.opencontainers.image.title`
(design/machine-images.md §2, Export file names): each ROM image or chip, the
source archive and each extra under its file name as given, without the
folder, with `.zst` after it when stored compressed (`bios-unemu-at.bin.zst`),
and the README as `README.md`. A name that macOS, Linux or Windows would not
take as it is, a name the export writes itself (`config.toml`,
`manifest.json`, `SHA256SUMS`, …), or two layers whose names are equal
ignoring case (an extra called `readme.md`, say), is refused before anything
is written, naming both files. An edit keeps the names; the next revision of
a package imported before ROM layers had names calls each ROM layer
`<rom id>.bin` (`<rom id>-<n>.bin` per chip), or `<rom id>-rom.bin` when that
name is taken. A folder import of the same ROM under another file name still
finds the package already imported; with a statement, it is refused with a
note that only the file names differ.
Every layer carries an SPDX licence next to its redistribution mark; a layer
without a valid licence counts as user-supplied. Rights follow the bytes: a ROM,
chip, document or archive whose SHA-256 and length the catalogue lists as
proprietary (an IBM BIOS chip, say) can never be marked open, whatever its
catalogue id, and `image push` keeps such bytes private in any package,
pulled ones included. A ROM the catalogue lists as open is open under the
catalogue's licence (`--user-supplied` keeps it private). `--unlisted ID
--role ROLE` adds one ROM the catalogue does not know (tag
`private/firmware/ibm-pc/ID:r1`;
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
cargo test --offline -p library-model -- save_image
cargo test --offline -p library-ui -- save_image
cargo test --offline -p window -- save_image image_settings layers_dialog \
  library_view::tests::save_as library_view::tests::settings_revision
```

In the window, firmware readiness and Media > Insert > From library use the
Preferences store root when set, otherwise the shared unemu family default:
`~/Library/Application Support/unemu/` on macOS,
`$XDG_DATA_HOME/unemu/` (or `~/.local/share/unemu/`) on Linux,
and `%LOCALAPPDATA%/unemu/` on Windows. Linux falls back when XDG_DATA_HOME
is unset, empty or relative. This directory itself contains `store.toml` and
`images/`.

The shared preferences are `unemu/preferences.toml` under the platform config
directory: `~/Library/Application Support` on macOS, an absolute
`$XDG_CONFIG_HOME` (otherwise `~/.config`) on Linux, `%APPDATA%` on Windows.
On first use, a missing file is copied from `unemupc/preferences.toml`, falling
back to the original project directory for users who skipped the earlier rename.
Machines (including their journals), diagnostics/trace logs, saves and states
keep using their `<data>/unemupc/NAME` tree while that old tree exists, including
for new writes. Only an absent old tree selects `<data>/unemu/NAME`; an existing
new tree never hides an old one. Location selection never traverses, copies,
merges or removes trees. This keeps absolute journals, relative paths in
symlinked machine files and private permissions intact, and avoids duplicating
large journals. Move a tree explicitly to switch its location, taking care of
absolute paths inside machine files. Abandoned `.NAME-import` staging trees
are left alone and never published. Preferences alone are copied with a lock
and atomic publication; a failed copy can be retried and leaves the old file.
Explicit paths (including `UNEMUPC_MACHINES_DIR` and `store_root`) and cargo's
source-tree directories keep their locations. The default image store is
already `unemu`: its summary index (`cache/summaries/index.json`), recipe
library index (`cache/libraries/`), decoded index (`cache/decoded/index.json`),
materialized blobs, download/build caches and `workspaces/` stay there. Diagnostics'
`trace-N.txt` files move with `diagnostics/`; other application messages go to
standard error, with no separate default log directory. The PC firmware index
stays `<cache>/unemupc/firmware-index.v2` (macOS `~/Library/Caches`, Linux an
absolute `$XDG_CACHE_HOME` or `~/.cache`); `UNEMUPC_*` names are unchanged.
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
`private/media/ibm-pc/<title-words>:1.0-r1` until edited (the CLI requires
`--tag`), and publisher, year,
genre, labels and description are optional fields (the CLI sets only the title).
An extra's kind comes from the same extension rule and can be changed; its DOS
name is suggested when the file name fits 8.3 and is otherwise left to the
transfer medium. The window always compresses, like the CLI without
`--no-compress`.
Its headless tests, with the shared import and the CLI contract:

```sh
cargo test --offline -p window install_media
cargo test --offline -p pc-image media_import
cargo test --offline -p cli-pc --test image_media_commands
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

## Minimum release of an image

After the split, `unemu.min-app-version` names the selected core's release
(`CoreDescription::version`), while `unemu.min-frontend-version` names the
application release. Front-end tables live in `image_store::compat`,
`pc_image::compat` and `recipe::compat`; PC-core tables live in
[`pc_config::compat` (unemupc)](https://github.com/unemu-org/unemupc/blob/main/crates/pc-config/src/compat.rs).
The extraction baseline is `0.0.3`; historical rows keep their values.
See decision-30 and [image compatibility](design/machine-images.md#56-compatibility-of-published-images).


An image, installation set or firmware package may declare the oldest
unemupc that reads and runs it, in the manifest annotation
`unemu.min-app-version` (`MAJOR.MINOR.PATCH`, no pre-release or build part;
machine-images §5.6). Writers compute it from per-feature tables
(`image_store::compat` for the shared rows, `pc_image::compat` for the PC
ones) and leave it out when it is 0.0.1, so everything 0.0.1 reads keeps its
bytes and digest: import, media and firmware import, template build, New
machine…, Save as new image…, `image publish`, `image machine commit`,
`image firmware edit`, `image compress` and catalogue edits. The derived
ones keep their source's value as a floor, and a machine image also takes
the highest value of the firmware packages it binds (`unemu.firmware.sources`),
so a fix stated on a package reaches every machine made with it. No writer
puts a set's media into a machine image yet; when one does, it takes the
set's value the same way.

The PC platform id `ibm-pc` is itself a row: release 0.0.3 first reads it,
because 0.0.1 and 0.0.2 compare platform ids exactly and know only the
retired id `ibm-pc-at` (machine-images §1). Every PC writer writes `ibm-pc`,
so everything written since declares 0.0.3. This build's level is 0.0.3,
its package version since release 0.0.3 (the release commit updates this
sentence; see Releases). Artifacts that name `ibm-pc-at` carry no value
and stay readable.

`--min-app-version X.Y.Z` on `image import`, `image media import`,
`image firmware import` (with `--for` or `--unlisted`) and `image firmware
edit` raises the value, for behaviour the tables cannot see, such as an
emulation fix a title needs. It never lowers it. A value below 0.0.1 or above
this release is refused when the command line is read; one below what the
tables compute is refused naming both (`the stated minimum version 0.0.2 is
lower than 0.0.3, the oldest unemupc that reads this artifact`), and an edit
refuses one below the package's own value. Re-importing a package's ROMs
with another value names `image firmware edit`. `image import` computes the
value only once its payloads are in the store (the config binds their
digests), so that refusal leaves unreferenced blobs for the sweep; the other
importers check before copying.

A release reads an image whose value is no higher than its level: its own
version, or the newest version its tables name when that is higher. A value
that does not parse counts as unmet. For any other image:

- `image pull` refuses before it fetches any blob and tags nothing:
  `REFERENCE needs unemupc X.Y.Z or newer (this is A.B.C)`, or
  `REFERENCE needs a newer unemupc (this is A.B.C)`;
- `image run`, `image machine new`, `image machine run`, `image media show`
  and `image firmware show` refuse with the same sentence, naming the tag
  (or the pinned digest), before the config is read;
- `image ls`, `image media ls`, `image firmware ls` and `image machine ls`
  (for each machine's image) append `min-app-version:X.Y.Z` to an image that
  declares a value, then
  `Needs unemupc X.Y.Z or newer` (or `Needs a newer unemupc`) when this
  release does not read it; `image media show` and `image firmware show`
  print `min-app-version<TAB>X.Y.Z`.

`image push` into `unemu/` computes the value again with the provider's
tables and refuses a missing, lower or unparseable declaration, naming the
computed release, and so does `image template build` before it tags a
template (`TARGET is in unemu/, which every release reads, and the
image needs unemupc X.Y.Z or newer by this release's tables, but its
unemu.min-app-version declares none; …`). Other namespaces are not checked.
Releases 0.0.1 and 0.0.2 ignore the annotation and fail as before.

A change that adds something a reader accepts adds its compatibility row in
the same change. Front-end rows live in `image_store::compat` (shared formats),
`pc_image::compat` (PC package formats) or `recipe::compat` (recipe versions
and steps). Use the owning module's `NEXT_RELEASE` value, currently 0.0.4 in
all three, as a historical literal, `AppVersion::new(0, 0, 4)`, and raise that
module's `NEWEST_ROW`. Do not use the constant in a row: the literal must keep
its meaning when `NEXT_RELEASE` advances. Keep `EXTRACTION_BASELINE` frozen at
0.0.3 and leave existing rows unchanged. These rows raise only
`unemu.min-frontend-version`; core hardware, vocabulary and firmware catalogue
rows live in `pc_config::compat` in unemupc and raise `unemu.min-app-version`.
Application composition takes the maximum of the three front-end
`NEWEST_ROW` values and the application's release, independently of the core's
level. A writer refuses a value above its domain's level. Completeness tests
catch missing rows and maxima:

```sh
cargo test --offline -p image-store compat
cargo test --offline -p pc-image compat
cargo test --offline -p recipe compat
```

## Registry push and pull

```sh
cargo run --offline -p window -- image push dos-lab:1.0.0 --store /path/to/store --dry-run
cargo run --offline -p window -- image push dos-lab:1.0.0 private/dos-lab:1.0.0 --store /path/to/store
cargo run --offline -p window -- image push dos-lab:latest private/dos-lab:latest --store /path/to/store --move
cargo run --offline -p window -- image pull private/dos-lab:1.0.0 --store /path/to/store
cargo run --offline -p window -- image pull nas.example:5000/private/games:1 --store /path/to/new-store --create --tag games:1
cargo run --offline -p window -- image push private/firmware/ibm-pc/kiselev-xi8088-1.0.2:r1 nas.example:5000/private/firmware/ibm-pc/kiselev-xi8088-1.0.2:r1 --store /path/to/store --dry-run
cargo run --offline -p window -- image pull nas.example:5000/private/firmware/ibm-pc/kiselev-xi8088-1.0.2:r1 --store /path/to/store
cargo run --offline -p window -- image pull unemu/media/ibm-pc/freedos:1.4 --store /path/to/store
cargo run --offline -p window -- image remote ls private/dos-lab
cargo run --offline -p window -- image remote ls registry.unemu.dev
cargo run --offline -p window -- image login registry.unemu.dev --user NAME
printf '%s\n' "$TOKEN" | cargo run --offline -p window -- image login nas.example:5000 --user NAME
cargo run --offline -p window -- image registry set registry.unemu.dev --namespace unemu --namespace private:private
cargo run --offline -p window -- image registry set nas.example:5000 --private
UNEMU_REGISTRY_LIVE=1 cargo test --offline -p image-registry -- --ignored live_
UNEMU_KEYCHAIN_TEST=1 cargo test --offline -p image-registry -- --ignored keychain_
```

References resolve like Docker's against `registry.unemu.dev`: `name:tag` is
`registry.unemu.dev/unemu/name:tag`, `owner/name:tag` keeps its owner, a first
component with a dot, a colon or `localhost` is a host, no tag means `latest`,
and `@sha256:…` pins content. `--registry HOST`, then `UNEMU_REGISTRY`, then
`default_registry` in preferences replace the default host. `push` without a
reference uses the local tag: a host-qualified tag goes to its own host, a
short one to the default host. `pull` from `registry.unemu.dev` tags locally in
short form (no host, no `unemu/` when one name remains); from any other
registry, a configured default one included, the local tag is the full
reference with host and port, Docker-style:
`pulled<TAB>nas.example:5000/private/media/x:1.1<TAB>DIGEST`. `--tag` names the
local tag instead; a digest pull needs `--tag`. Each tag a pull writes records
its registry (`unemu.pull.registry` in the store's `index.json`); tags made
here, and tags pulled before that record existed, have none. `--create`
initializes a missing store.

`push` leaves a tag that already names the image alone and never moves a
version tag. Installation media sets and firmware packages are tagged
`[<release>-]rN` (recipes.md §8.2, decision-38): `1.4-r1`, `1.4-720k-r2`,
`usa-rev1-r1`, `r1`. A tag that ends in `rN` never moves, even with
`--move`: push the changed set or package under the revision the refusal
suggests (`1.4-r2`). The release alone (`1.4`, also a release shaped like a
version, `1.0.0`, which the version rule of machine images does not bind) is
an alias that names the newest revision of its release: push it after that
revision, and it moves, with `--move`, only forward to a newer revision of
the same release that the registry already has. The client checks that on
the registry: it lists the repository's tags, takes the revisions of the
release, reads the digest each names highest first, and refuses unless the
pushed image is the highest revision's and the image the alias names now is
a lower one's; an alias published before revisions, which no revision
names, is to be deleted on the registry, not moved. Sets and packages have
no `latest` and take no tag outside the scheme (`Beta`). A machine image's tag that names another
image, `latest` included, moves only with `--move`; without it the refusal
shows both digests. A tag counts as taken whatever manifest or index type it
names, an image index pushed by another tool included.

`pull` of a set or package by its alias (`unemu/media/ibm-pc/freedos:1.4`)
writes the revision that names the same image on the registry
(`…:1.4-r2`); a pull by a revision writes that one. Either way the store
moves the local alias with it, forward only and under its index lock, so
`freedos:1.4` locally names the newest revision of 1.4 the library holds;
the output says so with an `alias` line before the `pulled` line. A
registry alias published before revisions, which no revision names there,
is pulled under the alias as it is. Locally too, a new alias is written
only at the newest revision of its release (`image media import --tag
x:1.0` is refused and names `x:1.0-r1`), no set or package takes a new tag
outside the scheme, and a deleted revision tag names no other image. A
store an earlier release wrote keeps its tags as they are (`freedos:1.4`,
`firmware/<catalogue-id>:1`, paths without the platform): they still list,
resolve and run, and nothing rewrites or moves them.

Firmware packages (`<namespace>/firmware/ibm-pc/<catalogue-id>:r1`) push and pull like machine
images and media sets; a pulled package lists in `image firmware ls`. A
package whose layers are all open may go to any configured registry,
including `unemu/`, except that an open ROM under a GPL, LGPL, AGPL or MPL
family licence also needs its linked source archive in the same image to
leave a private registry or to enter `unemu/`; the guard refuses it
otherwise. Any user-supplied layer keeps a package in private registries
outside `unemu/`.

Registries are listed in the shared preferences file (`unemu/preferences.toml`
in the platform config directory, or `--config FILE`; a file left in the
directory named after the project's former name is copied there once):

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

The desktop shell's Image › Push… and Image › Pull by reference… (and Pull…
of the tags selected in the library's Remotes) build the same push plan
(`platforms::push_plan`: local marks plus the firmware catalogue), the same
client (`image_registry::accounts::registry_client`) and the same providers
list (`platforms::with_image_providers`) as `image push` and
`image pull`, and resolve short names against the same default registry
(`UNEMU_REGISTRY`, then preferences). They run on the library worker thread:
- Push… shows the `--dry-run` classification as a table and the guard's
  verdict while the destination is typed, starting with the registry entry
  and the namespace the destination falls in ("HOST is configured;
  private/ is marked private"). Pull by reference… shows the same line under
  the resolved reference. A non-version tag moves only after a confirmation that shows
  both digests, and only from the digest shown. A
  local tag that names another image than the reviewed one is not pushed.
- Pull by reference… and Pull… of Remotes write the local tag `image pull`
  would (short from `registry.unemu.dev`, host-qualified from any other
  registry) unless another one is typed.
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
bash scripts/core-checkout.sh
fixture=$(bash scripts/core-checkout.sh --print-root)
export "$fixture"
cargo run --offline -p window -- image template build --vendor "$UNEMUPC_FIXTURE_ROOT/vendor" --vendor vendor --store /path/to/templates --dry-run
cargo run --offline -p window -- image template build --vendor "$UNEMUPC_FIXTURE_ROOT/vendor" --vendor vendor --store /path/to/templates
```

`image template build` builds the two template machines and nothing else. It
is a command rather than a script around `image firmware import` and New
machine…: the template machines come from the create API (`pc_image::create`),
which has no command of its own, and the push guard must judge each manifest
before its tags are written. Use `--vendor "$UNEMUPC_FIXTURE_ROOT/vendor"
--vendor vendor` for the verified core's open firmware fixtures and the front-end's
media. Repeated `--vendor` directories are searched in order for each input file;
the first match wins. A single `--vendor` still works, and the default is
`vendor/` in the working directory. It writes into `--store`
(created when missing), in this order (design:
machine-images §5.5):

| Local tags | Registry repository | Image |
| --- | --- | --- |
| `open-at:1.2.0`, `:latest` | `unemu/open-at` | IBM 5170-class 80286, the three ROMs, 640 KiB + 1 MiB, VGA, COM1 without a mouse, an empty 1.2M A: and an empty 1.44M B: drive, a blank zero-base 504 MiB (1024/16/63) ATA disk at 1F0h, no sound card |
| `open-at-games:1.2.0`, `:latest` | `unemu/open-at-games` | `open-at` plus SB16 (220h, IRQ 5, DMA 1/5), a Microsoft mouse on COM1 and a 2 MiB Lo-tech EMS board (260h, frame D000h) |

**Firmware packages and installation sets.** The open firmware packages the
templates bind and the FreeDOS 1.4 Floppy Edition installation sets are built
and published by the software repository
(<https://github.com/unemu-org/software>), not by this command. Pull them
from the default registry (`--create` for a new store):

```sh
unemu image pull unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1 --store /path/to/store
unemu image pull unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1 --store /path/to/store
unemu image pull unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1 --store /path/to/store
unemu image pull unemu/media/ibm-pc/freedos:1.4 --store /path/to/store
unemu image pull unemu/media/ibm-pc/freedos:1.4-1.2m --store /path/to/store
unemu image pull unemu/media/ibm-pc/freedos:1.4-720k --store /path/to/store
```

| Local tag | Image |
| --- | --- |
| `unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1` | unemu AT BIOS package (the unemu fork of Kiselev 8088_bios) with its source archive |
| `unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1` | unemu LGPL VGA BIOS package with its source archive |
| `unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1` | XTIDE Universal BIOS r638 package with its source archive |
| `unemu/media/ibm-pc/freedos:1.4-r1`, `:1.4` | the edition's seven 1.44M disks |
| `unemu/media/ibm-pc/freedos:1.4-1.2m-r1`, `:1.4-1.2m` | its eight 1.2M disks |
| `unemu/media/ibm-pc/freedos:1.4-720k-r1`, `:1.4-720k` | its twelve 720K disks |

A package's tag is its revision, `r1`, and a set's names the release (the
version and, unless it is the edition's own 1.44M, the floppy format) and
the revision; neither revision tag moves and neither has `latest`. A pull by
a set's release alias writes the revision it names, and the local alias
follows it. Each set holds the disks unmodified in installer order
and, as extras, `LICENSES.TXT` (the licence index, its licence text) and the
deterministic source archive, open under the conjunction of the programs'
licences with both linked; its title is the work's, `FreeDOS`, with the
release and edition in their own fields (machine-images §5.5). The same
images can be built locally from the fixtures:
`pc_image::template::build_firmware` and `build_media` (fixtures of the sets:
`bash vendor/freedos/fetch-floppy-1.4.sh`) give them under these tags, and
their tests pin the published digests, so each pin checks that the two
repositories still build the same image. The packages and sets published so
far name the platform by its retired id `ibm-pc-at`, which this release reads
as `ibm-pc`, under the old names (`unemu/firmware/<catalogue-id>:1`,
`unemu/media/freedos:1.4`), whose layers have no export file names except a
package's source archive and README; the pins are those of their `ibm-pc`
rebuilds with a file name on every layer (a pull names the files
`x86BOOT.img.zst` …, `LICENSES.TXT.zst`, `freedos-1.4-floppy-src.tar.zst` and
`metadata.json`, or `bios-unemu-at.bin.zst`, its source archive's `.tar.zst`
and `README.md`, and `zstd -d` gives back each fixture under its own name),
which replace them under the names above before the first public release
(decision-32, decision-35, decision-37, decision-38). `image media import` reproduces each
set from the command line (Local machine images, above).

`image template build` imports the three packages it binds from the firmware
fixtures first, as local copies under the tags above: the same packages a
pull gives, so a store that already holds them keeps them. They get no output
line and no guard check of their own; the guard judges their ROMs and source
archives inside each template, and the package tags stay in the store even
when a template is refused. A package tag that names another image stops the
build before any template, and the dry run too; remove or rename that tag, or
build into a dedicated store. The installation-set fixtures are not read.

The local tags are the short form `image pull` writes for each repository
from `registry.unemu.dev`, so
`image push TAG` without a reference reaches the repository in the table. Each
source archive must be the one the catalogue pins (its fetch script builds it);
another archive, or a missing fixture, stops the build and names the script.
No operating system goes into a template machine; installation sets, such as
the FreeDOS ones above, are separate images. The output is one
`TAG<TAB>DIGEST<TAB>machine<TAB>created|present|moved:OLD|kept:CURRENT`
line per template tag, written as soon as its template is built, then
`guard:allowed`. A tag that already names the same image stays; a version tag
naming another image stops the build (give a changed template a new version
in `pc_image::template::VERSION`). `latest` moves by compare-and-swap, and only
away from an image that lower versions of the same repository name alone; it
stays (`kept:`) on a higher version or on an image no version of the
repository names. The same fixtures give the same digests on every run and
host: nothing depends on the clock, the host or the fixture paths; a test pins
the digests of the published generation.

Before any tag of a template is written, the build applies the push guard for
`unemu/` on the default registry, from the store's local marks and the firmware
catalogue: every blob open, every copyleft ROM with its linked source archive.
A refusal stops the build, for example in a store where another local tag
records the same ROM bytes as user-supplied, so build into a dedicated store.
The tag is written only for the manifest the guard judged. `--dry-run` builds
in a scratch store under the system temporary directory, removes it, and
reports what a build would do to each template tag of `--store` (which is
never written and need not exist): `new`, `present`, `conflict:DIGEST` (a
version tag naming another image), `moves:DIGEST` or `kept:DIGEST` (the
alias), then `dry run; no blobs or tag written`. When `--store` exists, the
guard judges each template with its marks as well as the scratch store's, so a
dry run refuses what the build would refuse.

The build never pushes; publishing is the maintainer's step. Log in once
(`image login registry.unemu.dev --user NAME`), review every dry run, then push
the version tags and the aliases:

```sh
S=/path/to/templates
cargo run --offline -p window -- image push open-at:1.2.0 unemu/open-at:1.2.0 --store "$S" --dry-run
cargo run --offline -p window -- image push open-at-games:1.2.0 unemu/open-at-games:1.2.0 --store "$S" --dry-run
cargo run --offline -p window -- image push open-at:latest unemu/open-at:latest --store "$S" --dry-run
cargo run --offline -p window -- image push open-at-games:latest unemu/open-at-games:latest --store "$S" --dry-run

cargo run --offline -p window -- image push open-at:1.2.0 unemu/open-at:1.2.0 --store "$S"
cargo run --offline -p window -- image push open-at-games:1.2.0 unemu/open-at-games:1.2.0 --store "$S"
cargo run --offline -p window -- image push open-at:latest unemu/open-at:latest --store "$S"
cargo run --offline -p window -- image push open-at-games:latest unemu/open-at-games:latest --store "$S"
```

Each dry run must end in `guard:allowed` with every blob `open`. A version tag
that already names another image on the registry is refused; push a new
version instead. Once `latest` names an older version there, its push stops
with both digests; repeat it with `--move` after checking them.

The desktop shell's **File › Get templates…** shows these templates in the
library's Remotes group, Remotes › the default registry › `unemu/` › Machine
images, where Pull… pulls the selected ones;
[`design/machine-images.md`](design/machine-images.md) §5.5 describes its
behaviour and §3.2 (Desktop library) the Remotes group, which lists the
other configured registries too.

## Shareware installation media (local only)

```sh
vendor/shareware/keen4/fetch.sh                # Commander Keen 4 1.4: keen4-sw14-1440.img, extras/
vendor/shareware/wolf3d/fetch.sh               # Wolfenstein 3-D 1.4: wolf3d-sw14-1440.img, extras/
vendor/shareware/duke2/fetch.sh                # Duke Nukem II 1.0: duke2-sw10-1440.img, extras/
SHAREWARE_ARCHIVE=/path/to/1wolf14.zip vendor/shareware/wolf3d/fetch.sh   # your own copy; no network
cargo run --offline -p window -- image media import vendor/shareware/keen4/keen4-sw14-1440.img --title "Commander Keen 4: Secret of the Oracle (shareware 1.4)" --tag private/media/ibm-pc/keen4-shareware:1.4-r1 --label '1=Disk 1 of 1 - Shareware v1.4 installation' --extra vendor/shareware/keen4/extras/README.DOC:README.DOC --extra vendor/shareware/keen4/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1991 --genre platformer --platform DOS --description 'Commander Keen 4: Secret of the Oracle, version 1.4, the shareware episode of a side-scrolling platform game in EGA graphics by id Software, distributed by Apogee Software. Keen searches the Shadowlands of Gnosticus IV for the eight missing Council Members. The disk holds the files of the original Apogee shareware archive 4keen.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-readme-keen4-v1.4 --license-text README.DOC --store /path/to/store --create
cargo run --offline -p window -- image media import vendor/shareware/wolf3d/wolf3d-sw14-1440.img --title "Wolfenstein 3-D: Episode 1 (shareware 1.4)" --tag private/media/ibm-pc/wolf3d-shareware:1.4-r1 --label '1=Disk 1 of 1 - Shareware v1.4 installation' --extra vendor/shareware/wolf3d/extras/VENDOR.DOC:VENDOR.DOC --extra vendor/shareware/wolf3d/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1992 --genre "first-person shooter" --platform DOS --description 'Wolfenstein 3-D, episode 1, Escape from Wolfenstein, version 1.4, the shareware episode of a first-person shooter with texture-mapped VGA graphics by id Software, published by Apogee Software. As B.J. Blazkowicz the player escapes from Castle Wolfenstein through ten floors, one of them secret. It needs a 286 or better. The disk holds the files of the original Apogee shareware archive 1wolf14.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-vendor-v11.01.92 --license-text VENDOR.DOC --store /path/to/store
cargo run --offline -p window -- image media import vendor/shareware/duke2/duke2-sw10-1440.img --title "Duke Nukem II: Episode 1 (shareware 1.0)" --tag private/media/ibm-pc/duke2-shareware:1.0-r1 --label '1=Disk 1 of 1 - Shareware v1.0 installation' --extra vendor/shareware/duke2/extras/LICENSE.DOC:LICENSE.DOC --extra vendor/shareware/duke2/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "Apogee Software" --year 1993 --genre platformer --platform DOS --description 'Duke Nukem II, episode 1, version 1.0, the shareware episode of a side-scrolling platform game by Todd Replogle and Apogee Software, in which Duke fights the alien Rigelatins. It needs VGA graphics and a 286 or better. The disk holds the files of the original Apogee shareware archive 4duke.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-license-v12.01.93 --license-text LICENSE.DOC --store /path/to/store
cargo run --offline -p window -- image push private/media/ibm-pc/wolf3d-shareware:1.4-r1 unemu/media/ibm-pc/wolf3d-shareware:1.4-r1 --store /path/to/store --dry-run   # guard verdict for the official path
```

Each script downloads the publisher's original shareware archive from a pinned
URL (FUNET's mirror of Apogee's file area, with a byte-identical 1994 CD-ROM copy
as fallback), checks its SHA-256 and member list, and caches it in `build/`, so
later runs are offline. It copies every archive member unmodified, in archive
order and with the archive's DOS timestamps, onto a 1.44M FAT12 floppy with a
fixed label, serial and non-bootable volume sector, checks the image's pinned
SHA-256, and writes the vendor and licence texts to `extras/`. Nothing from the
archive is run. Run the scripts inside `nix develop`, which provides mtools
4.0.49, Info-ZIP UnZip 6.00 and curl (the pinned images were measured on macOS
with the host's UnZip 6.00 and reproduce with the devShell's; Linux not measured).
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

Run in the Nix devShell. The workspace contains the front-end crates only;
the pinned cores build as git dependencies.

```sh
cargo build --locked --all-targets
cargo test --locked --workspace
cargo run --locked -p window --bin unemu -- --help
cargo run --locked -p window --bin unemu -- --open game.nes
cargo run --locked -p window --bin unemu --features platforms/test-core -- --open machine.utc
```

Never launch GUI windows in automated checks. For tests requiring PC fixtures,
use the [verified PC fixture setup](#pc-fixtures-for-application-commands)
above.
The checkout must match the core revision in `Cargo.lock`. The core's own
machine runners and hardware goldens are maintained in
[unemupc's commands](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/commands.md).

### PC machine-file command words

```sh
cargo run --offline -p window -- run --machine "$UNEMUPC_FIXTURE_ROOT/machines/at-freedos.toml" --max-steps 12000000 --idle-limit 1000000 --print mda
```

`run --machine FILE` uses `RunPolicy::natural`: `--max-steps` defaults to
20,000,000 completed-step equivalents and `--idle-limit` to 1,000,000 halted PIT clocks.
Both accept unsigned decimal integers (zero is allowed). It prints a
`stopped: ...` line to stderr. Halt, reset-request and budget-exhausted outcomes
exit 0; execution, configuration and persistence errors exit 1; usage errors
exit 2. `--allow-unlisted-firmware` accepts unknown firmware for development
without relaxing placement or board constraints. `--print CARD`
selects a display text dump or UART TX bytes on stdout. Without it, the first
display is chosen, then the first UART, or no output if neither is configured.
UART capture is bounded to 1 MiB; overflow is an error. Normal halt and budget
exhaustion finish journal persistence and print maintenance warnings on stderr.
Execution errors leave unsealed commands for recovery on the next mount.
When the CPU core refuses an operation (an unsupported system operation such
as `LegacyInstruction`, an instruction it does not lift, or a fault it
returns outward), or the guest is stuck delivering exceptions (`stopped: cpu
fault: the processor is stuck delivering exceptions: ...`, exit 1), `run
--machine` follows the `stopped: ...` line with a
multi-line CPU stop report on stderr: CS:IP, MSW, CPL, A20, the CS and SS
caches, GDTR, IDTR, LDTR and TR, the 16 bytes at CS:IP, and for a far
CALL/JMP, INT n or RETF/IRET the descriptors the instruction reaches with
their 8 bytes and the current 286 TSS's ring stacks. The window shows the same
report in the Machine error dialog (Copy puts the message and the report on
the clipboard) and writes it to `diagnostics/last-cpu-stop.txt` in the
machine's workspace: the image's workspace for a library session (discarding
the workspace removes it), `FILE.local/` beside a machine file; the status
line names the path. The format is described in
[observation.md §CPU stop report in unemupc](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/design/observation.md#cpu-stop-report).
`--diag-at N` writes the machine's diagnostic report to stderr when the run
reaches step N (the unit of `--max-steps`), then runs on to its usual stop:
the CPU's registers, flags and descriptor caches, the bytes at CS:IP and the
words at SS:SP, the CS:IP of the last run slices, the 8259s, the 8042, the
PIT, A20 and the CMOS shutdown byte and NMI mask, for a guest that hangs
without an error. A run that stops before step N prints no report. With
`--diag-at` the report also lists the last 64 entries of the exceptions and
interrupts the guest took, oldest first, consecutive identical deliveries
folded into one.

`--trace-exceptions` records every exception and interrupt the guest takes,
delivered in the guest and therefore invisible otherwise, and writes the
recording to stderr when the run ends, before the `stopped: ...` line: the
last 10,000 entries, oldest first, each with the retired count, the kind
(fault, trap, software, irq, nmi), the mnemonic (`#UD`, `#GP`, `INT 21h`,
`IRQ 0`, `NMI` …) and vector, the saved CS:IP and its linear address, the
error code, the CPL before and after, the 8 bytes at the saved CS:IP
(memory only, never a device, read at the end of the run slice), and the
registers of the interrupted code when the delivery began (`ax bx cx dx si
di bp ds es ss:sp flags`, the INT's inputs for a software interrupt);
consecutive identical deliveries fold into one entry with their retired
range and count (`delivery 103..600100 x100000 fault #UD …`), showing the
last one's registers, and a summary
gives the exact total, the dropped count and a count per mnemonic. The
desktop's Record exceptions writes the same text. Off, nothing changes; on,
the run's retired count and every guest effect are the same. The format is
described in [observation.md §Exception and interrupt deliveries in unemupc](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/design/observation.md#exception-and-interrupt-deliveries).

A configured card without printable output is an argument error (exit 2).
The core owns the firmware catalogue rules and report formats; see
[unemupc's commands](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/commands.md).

### Tools diagnostics

In the window, Tools › Diagnostics holds three items:

- **Save machine report** (Ctrl+Alt+Shift+D, ⌘⇧D on macOS, with the pointer
  captured or not) writes the running PC's report to `diag-N.txt` in
  `unemu/diagnostics` of the platform data directory (on macOS
  `~/Library/Application Support/unemu/diagnostics`), N counting on from
  the highest number already there, and the status line shows `Machine
  report saved to PATH; memory image saved to PATH`. It works whenever a PC
  runs, paused too, and is disabled without one. The machine keeps its code
  trail from every Start; the report's exception deliveries start with the
  first report (which says so and lists none) or with Record exceptions, because the CPU runs
  6 to 10 per cent slower on interrupt-heavy guests while it logs them.
  The chord's D never reaches the guest. The same action also writes guest RAM to `diag-N.mem`, with the same reserved
  number; either extension already in the folder advances the number, and
  existing files are never overwritten. The `.mem` file is flat: byte offsets
  equal guest physical addresses, gaps (including omitted ROM and device
  windows) are zero-filled, and its length is the end of the highest region.
  Regions and zero gaps are streamed without a second image-sized buffer.
  The complete text report is written and flushed before the image: it names
  the planned file and its size and lists each region's kind, physical base
  and byte length in hex. If reading the image fails, the report includes the
  reason and no image is written. If writing the image fails, its partial
  file is removed if possible and a line with the reason is appended to the
  report. The status line reports both outcomes. These files contain guest
  bytes for local diagnosis: keep them in the diagnostics folder and never
  add them to a repository. They are not restorable snapshots. The machine
  report is described in [observation.md §Diagnostic report of a running
  machine in unemupc](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/design/observation.md#diagnostic-report-of-a-running-machine).
- **Record exceptions**, a check item off at every launch, records the
  running PC and every PC started while it is checked: the last 10,000
  entries, folded as above, with exact totals. Each recording goes to one
  `trace-N.txt` in the same folder, created new and never written over
  another program's file, written at Save machine report (the report and
  the status line name it), at Stop, at unchecking and when the
  window closes or the app quits. Nothing is written while the guest runs:
  an app that is killed leaves no trace file.
- **Open diagnostics folder** shows that folder in the platform's file
  manager (`open`, `explorer` or `xdg-open`), creating it first.

The window reads no environment switch for these; `UNEMUPC_DIAG` and
`UNEMUPC_TRACE_EXCEPTIONS` are gone.

### Window operation and shortcuts

```sh
# Manual desktop use only:
UNEMUPC_MACHINES_DIR="$UNEMUPC_FIXTURE_ROOT/machines" cargo run --offline -p window --bin unemu
cargo run --offline -p window --bin unemu -- --machine "$UNEMUPC_FIXTURE_ROOT/machines/at-freedos.toml" --display mda
cargo run --offline -p window --bin unemu -- --machine /path/to/local.toml --allow-unlisted-firmware
cargo run --offline -p window --bin unemu -- --open FILE --core ID
cargo tree --offline -i window
```

The window binary requires a graphical desktop; PC machine files also need
their firmware and disk inputs. After the fixture setup above,
`UNEMUPC_MACHINES_DIR="$UNEMUPC_FIXTURE_ROOT/machines"` selects the core's
examples for a manual desktop run.
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
| Save machine report (Tools › Diagnostics) | ⌘⇧D | Ctrl+Alt+Shift+D |
| New / open machine file | ⌘N / ⌘O | Ctrl+N / Ctrl+O outside guest focus |
| Settings | ⌘, | Ctrl+, outside guest focus |
| Keyboard shortcuts | ⌘⇧/ | F1 outside guest focus |

On macOS, ⌘Q quits, ⌘W closes, ⌘H hides, ⌘M minimizes and ⌘Tab switches
applications. Help > Keyboard shortcuts and the menus show the platform's bindings.
New machine remains a placeholder. Enter starts the selection when stopped.

While a machine runs, File > Machine library, Open in File > Open machine
file… (after a path is entered; closing that dialog changes nothing), Exit,
⌘Q, ⌘W and the window's close button pause it first and ask
"unemu is running <machine>. Stop it and continue?" with Stop machine and
Cancel (Esc; Enter and Space do not answer). Cancel resumes the machine where
it paused, without catch-up, and hands back guest keyboard focus and pointer
capture if the guest had them; an explicit pause stays in effect. Exit, ⌘Q,
⌘W and the close button also work while a menu or another dialog is open:
they close or cancel it first, keeping only this question or the Gaming-mode
keep-or-discard question. One Escape closes only the top-most dialog. A
stopped, halted or failed machine is not asked about, and Machine > Stop and
Power cycle act at once. A Gaming-mode session asks Keep as new image or
Discard after stopping instead. On macOS the global menu bar holds only the
application menu: About unemu, Hide unemu (⌘H), Hide Others (⌥⌘H),
Show All and Quit unemu (⌘Q). Quit and ⌘Q reach this question like Exit.
Dock › Quit, logout, restart and shutdown wait for it as well: with no machine
running they quit at once after persistence; Stop machine quits once the stop
and persistence have finished; Cancel keeps the application running (logout
reports that unemu cancelled it). A stop that fails to finish its journals
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
use host pacing. See [observation.md in unemupc](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/design/observation.md#window-pacing)
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

Machine > Disk layers… and the Layers toolbar button show each fixed disk's
chain with short digests, sizes, sharing and working sectors, the floppy
catalogues, firmware and settings. Save as new image… asks for a new
repository:tag, offers Flatten disk chains into new bases (unchecked), and
stops the machine before publication. Gaming sessions offer Keep as new image…
instead, without Flatten. Discard changes lists writable media with changes
before confirmation.
For machine files the dialog offers Import image…. Edit > Delete tag… confirms
removal of that tag only; shared bases remain. These menu commands have no new
key chords; existing shell shortcuts retain their platform mapping.

The window lists the machine files of the library's machines directory and
selects its `at-freedos.toml` when that file exists (a fresh install selects
nothing): `UNEMUPC_MACHINES_DIR` when set; else, when cargo started the program,
a `machines/` directory under the front-end source tree (cargo names the
crate's directory in `CARGO_MANIFEST_DIR` at run time, so nothing of the tree is
compiled in; this repository ships no such directory); else
`unemu/machines` in the platform data directory (`~/Library/Application
Support` on macOS, `$XDG_DATA_HOME` or `~/.local/share` on Linux,
`%LOCALAPPDATA%` on Windows). `--help` prints the directory in use. The
desktop's ROM folders are its preferences, `--rom-dir`, `UNEMUPC_USER_ROMS_DIR`
and, under cargo, a `vendor/user-roms` directory under the source tree (not
shipped here). For core fixture ROMs, pass
`--rom-dir "$UNEMUPC_FIXTURE_ROOT/vendor/user-roms"`. A machine file outside any
`machines` directory keeps its Settings… child in the `local/` folder of the
library's machines directory; with no data directory and no
`UNEMUPC_MACHINES_DIR`, Settings refuses with a message instead of writing into
the working directory.
`--machine FILE` is relative to the current directory; image and inheritance
paths inside it are relative to their defining file. A machine named with
`--machine` that does not exist is listed and selected, so its error shows.
`--display CARD` chooses a display by id; the default is the first display in
file order (MDA in the shipped machine). Positional BIOS/floppy paths are removed;
use an extending file as shown in
[`machines/README.md` in unemupc](https://github.com/unemu-org/unemupc/blob/main/machines/README.md). From outside the
workspace, Cargo also needs `--manifest-path /path/to/workspace/Cargo.toml`.
`--help`, `--version` and `--about` include the embedded fonts' attribution and
the Nuked-OPL3 notice; `--version` and `--about` also print one line per
compiled-in core, `core unemupc 0.0.4 (unemupc), contract 0.5` and
`core unemunes 0.0.1 (unemunes), contract 0.5` in the release (a core
built for another contract ends its line with `, unusable: ` and the reason),
then the licence, the source URL and where the full licence texts are.
`unemu --version` (or `--about`) prints the same for the command line, the core
lines after its first two lines.
`--open FILE [--core ID]` starts a machine from a file as File › Open… does:
the file is hashed whole on a worker thread, the cores that list its
extension are asked first, then the others (only `ID` with `--core`), and a
file no core runs, or that several run, is refused with a message box.
`--core` without `--open`, and `--open` with `--machine`, are usage errors.
What such a machine leaves behind goes under `unemu/saves/CORE/aa/DIGEST/`
in the platform data directory (and saved states, later, under
`unemu/states/`), keyed by the content's SHA-256, not its name or place.
The release's cores are the PC, which opens no files, and the NES core of
unemunes, which opens NROM (mapper 0) cartridge images: File › Open…'s dialog
offers "NES cartridge images (*.nes)", and `--open game.nes` starts one, say
`cargo run --offline -p window --bin unemu -- --open /path/to/game.nes`. A build whose
cores open nothing greys File › Open… ("No core in this release opens
files"). `--features platforms/test-core` adds the scripted test core, whose
`.utc` scripts it runs (the [script grammar in unemu-host](https://github.com/unemu-org/unemu-host/blob/1692a1fcfd6c9900d3074786a54f51c9de3b30f0/crates/test-core/src/lib.rs) describes them, and
`unemu-test-core 1` on the first line is a whole script). With that feature
the `--about` pins of both programs fail by the test core's line.
On Linux the window requires the host's X11 or Wayland client libraries at run
time; a bare Nix devShell may not put them on the dynamic loader's search path.

## Affected tests and the bead gate

Run the plan before the runner in the devShell:

```sh
bash scripts/affected-tests.sh --plan
bash scripts/affected-tests.sh
bash scripts/affected-tests.sh --base origin/main --tier goldens
bash scripts/affected-tests.sh --self-test
bash scripts/bead-gate.sh
```

The planner maps changed front-end files to their crates and dependents.
Mockups and recipe corpus/schema inputs select their readers. Documentation
alone selects checks. `machine-view` and `unemu-test-core` come from the pinned
[unemu-org/unemu-host](https://github.com/unemu-org/unemu-host), outside this
workspace; changes land there before a re-pin here. Local host overrides use
the gitignored patches in [dev setup](dev-setup.md#host-libraries-unemu-host).
Workspace dependencies, the lock file, core and unemu-host pins and
unknown inputs select the full front-end golden tier. The core's fixtures
come from `UNEMUPC_FIXTURE_ROOT`, not from this repository's source tree.
Review the plan when it selects everything unexpectedly.

The runner defaults to cargo-nextest; `AFFECTED_RUNNER=cargo` selects cargo
test. `AFFECTED_CARGO` supplies the supervising cargo wrapper. Required PC
fixtures use `UNEMUPC_REQUIRE_FIXTURES=1`. Keep logs outside the repository,
and run cargo commands sequentially through the shared-machine wrapper.
Only run ignored tests named by the supervising brief; user-media tests are
opt-in. The bead gate adds formatting, guards, Clippy, rustdoc and dependency
policy checks; the supervisor runs cargo deny where its lock is available.

## Checking recipes

`unemu recipe check` runs the static checks of a recipe
([design/recipes.md](design/recipes.md) §6.11): it reads the recipe, the
work file it names and, for a pinned recipe, its lock, and no input.

```sh
cargo run --offline -p window -- recipe check backlog/docs/design/recipe-corpus crates/pc-image/data/recipes
cargo run --offline -p window -- recipe check my.media.toml other.firmware.toml
cargo run --offline -p window -- recipe check my.media.toml --deny-drift
```

A folder is checked with every `*.media.toml`, `*.firmware.toml`,
`*.machine.toml` and `*.work.toml` below it (hidden files and folders and
lock folders skipped); a file named on the command line is read whatever
its name. Each finding is one line, `FILE:LINE:COLUMN:
error[6.11.N] KEY.PATH: message`, with 1-based array indices
(`step[1].members[2].path`); every finding is reported, each once. Then one
line per file: `ok` or the number of findings, its kind, and for a recipe
the split of rule 5 (`required inputs: …; optional inputs: …`, the inputs
only an attachment uses). Exit status: 0 without findings, 1 with findings,
2 on a usage error or a file that cannot be read. This build has the
`ibm-pc` provider only: on the corpus the `ibm-pc` files pass and the NES
and Amiga recipes each get the one rule-7 finding that no provider of
their platform is in this build, with nothing behind it reported.

A pinned recipe (one with `digest`) is checked against its lock (rule 8,
points 1 to 3 of design/recipes.md §3.8, see Pinning recipes below): the
lock must hold the manifest the digest names and the config and metadata
that manifest names, each file's bytes must have the SHA-256 its name
says, and the lock's decoded fields must agree with the recipe: the
artifact type, the platform, the media in order (id, label, format by name,
file name), the extras (file name, kind and DOS name, a kind or DOS name the
recipe leaves out compared with the importer's default), the ROMs (id, role,
catalogue id, chips and their stated position, part number, lane and
offset), a package's fields (one the recipe leaves out must be empty in the
lock; boards and machine classes when stated) and its generated README
(regenerated from the lock's config and marks and compared byte for byte),
and the title record. The licence marks the push guard trusts are checked
too: without `[license]` no descriptor may be open or link terms or a source
archive and the manifest names no licences; with it, an open payload layer is
open under exactly the recipe's expression and an open generated layer
(config, metadata, README) under `CC0-1.0`, every terms or source link names
the recipe's own terms or source extra, a set's open payloads carry those
links, and the manifest's licence list is its open layers'. A lock may be
more restrictive than its recipe, never less. Which layers are generated text follows
from the manifest (its config, its metadata layer, the layer a package's
config names as its README); a lock that holds any other file is refused, as
a lock holds no payload byte. A package's provenance and each ROM's required
devices must be the firmware catalogue's. Each disagreement is a finding
at the recipe's `digest` (`the lock disagrees with the recipe:
medium[1].label: the lock has "The game", the recipe has "Another
label"`). A field the recipe or its work file no longer states is a finding
too (`title.year: the lock has 1993, the recipe has nothing`). A title field
whose value today comes from the work file and differs from the lock is not
a finding: the line after the file's line reads `  drift:
FIELD: the lock has …, NAME.work.toml has …; a build writes the lock's value
until a new revision is pinned`.

`recipe check --deny-drift FILE|FOLDER...` keeps those drift warnings and
returns 1 when any is present. Without the flag, drift alone still exits 0:
the work file is a description for the next revision, and the lock wins
(design/recipes.md §3.8). Use the flag in CI to require the work file and
lock to agree. Read and usage errors still exit 2.

The JSON Schema `design/recipe-schema-1.0.json` is written from the
parser's key tables and the providers; a test in `pc-image` compares it.
The 1.0 key set and step behaviour are frozen. Regeneration for a format
change (new keys, steps or formats) belongs to a new minor version under
design/recipes.md §9, with its own versioned schema; it must not add keys to 1.0.
To reproduce the existing 1.0 schema from its unchanged tables:

```sh
UNEMUPC_WRITE_RECIPE_SCHEMA=1 cargo test --offline -p pc-image --test recipe_check
```

`unemu recipe show FILE` prints a media recipe's title record merged with
the work file it names, by the rules of design/recipes.md §6.16, as the
build will write the set's title metadata:

```sh
cargo run --offline -p window -- recipe show backlog/docs/design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml
```

The first line is `NAME:TAG`, the digest or `draft`, and the platform; then
one line per value, a field name, a tab and the value (text quoted), with
the names and order of `unemu image media show`'s catalogue lines
(`publisher`, `developer`, `year`, `genre`, `platform`, `description`,
`version`, `edition`, `requirements`, `need`, `work`, `synopsis`,
`work-id`, `set-label`) and the title fields that command does not print
beside them (`title`, `sort-title`, `series`, `players MIN MAX`,
`language`, `region`, `source-media`). A value that came from the work file
ends in `# from NAME.work.toml`. The recipe must pass `recipe check`: if it
does not, `show` prints the check's report instead and exits 1. Exit 2 on a
usage error, a file that cannot be read, a folder, or a file that is not a
media recipe (a firmware recipe or a work file has no title record to
merge).

For a pinned recipe `show` prints the record of its lock, which a build
writes, not today's merge: the first line names the digest, the second is
`# locked: the record of DIGEST, which a build writes; the work file's
edits wait for a new revision`, then one `# drift: …` line per field where
the work file now differs from the lock, then the lock's values, with no
`# from` comments (the lock does not say which file a value came from).

## Planning recipes and libraries

`unemu recipe plan` resolves every input of a recipe without building
anything ([design/recipes.md](design/recipes.md) §3.4, §3.5): for each
input, where it was found or why not; the steps that would run; the outputs
that can be built. Libraries are folders where the user keeps files; the
plan finds an input in them by size and SHA-256, whatever the file is
called, and looks inside zip archives.

```sh
cargo run --offline -p window -- library add /path/to/games
cargo run --offline -p window -- library scan
cargo run --offline -p window -- library ls
cargo run --offline -p window -- recipe plan my.media.toml
cargo run --offline -p window -- recipe plan my.media.toml --json --library /path/to/more --store /path/to/store
cargo run --offline -p window -- library remove /path/to/games
```

- `library add DIR` and `library remove DIR` change the list of libraries,
  the key `libraries` of the shared preferences file (`--config FILE` for
  another file), absolute paths in search order. Other keys are kept.
- `library scan [DIR...]` lists the files of the libraries (or of the
  folders named), the central directory of each `*.zip` and the rows of
  each catalogue into the index, and hashes nothing. `library ls` prints
  each library and what its index holds (`missing` when the folder is not
  there, `not scanned` without an index).
- `recipe plan FILE` checks the recipe (a recipe with findings prints the
  check's report and exits 1), then looks for each value in the store
  (images whose layers hold the content, compressed or not), in the store's
  download cache (files a `recipe build --fetch` downloaded, named by their
  SHA-256), beside the recipe (an input with `bundle = true`, by its name),
  and in the libraries
  of the preferences followed by each `--library DIR`, in that order. The
  members of an `unpack` step that state size and SHA-256 are looked for
  before their archive, which is then needed only for the members still
  missing. For a pinned recipe, every output a step makes is looked for
  first by the payload its lock names, in the store, the download cache
  and the libraries, in that order (not beside the recipe); a
  found output needs none of the steps and inputs it is made from, which
  are then `not needed: the outputs made from it were found` (a step: `the
  outputs it makes were found`). The plan lists them under `outputs found
  instead of made:`, one line each, `KIND ID (FILE): found WHERE` or `not
  found, made from REFERENCE`. A pinned recipe whose lock is lost (nothing
  at all at `FILE.lock`) is planned as `recipe build` builds it, and the
  plan says so before its inputs: `lock: FILE.lock is lost; a build makes
  its files with this build's writers and accepts only the pinned digest`
  (`lock_lost` in the JSON); `recipe check`, the checker, still reports
  rule 8 for it. A file is hashed only when a value of its
  size is wanted, and the hash is kept in the index under the file's size
  and change times, so the next plan does not hash it again; a file whose
  size or times changed is hashed again, never trusted. A value not found
  lists its near misses: a file with its name and another size or hash,
  and one of its size with another hash. URLs are listed, never fetched.

Catalogues in a library are hints, never proof: a CSV whose header names
`path`, `size` and `sha256` (optionally `crc32`, `sha1`, `md5`), and
`SHA256SUMS` or `*.sha256` files (`<hex>  <path>`), paths relative to the
catalogue's folder. A row moves its file ahead of the others of that size;
a row whose file holds other bytes is printed as a stale catalogue row.

The index is a rebuildable cache in the store (`--store DIR`, else the
default store): `cache/libraries/<SHA-256 of the library's path>.json` per
library, and `cache/decoded/index.json` for the store's own content.
`library scan` creates the store when it is missing; `recipe plan` uses it
when it exists and otherwise keeps the index for the one run. The plan
writes nothing else.

The text form, for a recipe whose archive is missing, one of its members
found inside another copy of the archive, and its manual bundled beside
it:

```text
recipes/missing.media.toml: media recipe private/media/ibm-pc/demo
store: store
library lib: 2 files, 1 zip archives (2 members), 0 catalogue rows; hashed 2, kept 0, dropped 0
inputs:
  game (GAME.ZIP): missing: 290 bytes, sha256 448fcc77…
    near miss (same name, another size): library lib: games/GAME.ZIP, 289 bytes
    used for: Disk 1 of 1
    url: https://example.org/GAME.ZIP
  manual (manual.txt): found beside the recipe: recipes/manual.txt
members found instead of made:
  files/GAME/RUN.COM: not found, taken from the archive
    near miss (same name and size, another hash): library lib: games/GAME.ZIP!GAME/RUN.COM, 81 bytes, sha256 8cbc1ab5…
  files/GAME/LEVEL.DAT: found in library lib: games/GAME.ZIP!GAME/LEVEL.DAT
steps:
  files: unpack game: blocked: missing game (opens game for 1 of 2 members)
  disk: fat files: blocked: missing game
outputs:
  medium disk-1 "Disk 1 of 1": blocked: missing game
  extra manual "MANUAL.TXT": can build
missing 1 of 2 inputs
```

A file set prints one line, `found 561 of 561 files, read from 204 places
(library DIR: 204)`, and a line for each file it misses. The last line is
the summary: `can build: all N inputs found` (`or not needed` when an
archive is not needed), or `missing M of N inputs`, with `; K optional
inputs not found` for inputs only an attachment uses.

`--json` prints the same as one JSON object, keys sorted, two-space
indented; these keys are stable:

- top level: `recipe`, `kind` (`media`, `firmware`), `name`, `complete`
  (the exit status is 0 exactly when it is true), `summary`, `store` (path
  or null), `store_warnings` (tags of the store left out because their
  manifest cannot be read, a cache not kept), `libraries`, `inputs`,
  `members`, `steps`, `outputs`;
- `libraries[]`: `path`, `files`, `archives`, `members`, `catalogue_rows`,
  `hashed` (hashed in this run), `failed` (`[{path, error}]`, hash attempts
  that failed in this run), `kept` (hashes reused from the index),
  `dropped` (stale entries), `stale` (`[{catalogue, path}]`),
  `unreadable` (`[{path, error}]`: folders and entries the scan could not
  read, archives it could not list, members with ZIP64 fields or an
  ambiguous name, members that failed before);
- `inputs[]`: `id`, `name`, `form` (`file`, `files`), `optional`, `status`
  (`found`, `missing`, `not-needed`), `reason` (why not needed, or null),
  `used_by` (labels of the outputs that need it), `urls`, `ids`
  (`[{db, set, name}]`); a `file` adds `size`, `sha256`, `found`,
  `near_misses`; a file set adds `files` (each with `path`, `size`,
  `sha256`, `found`, `near_misses`), `found_files` and `places` (the
  distinct files or members they were found in);
- `found`: null, or `{source: "store", tags}`, `{source: "download-cache",
  path}`, `{source: "bundle", path}` or `{source: "library", library, path,
  member}` (`member` null for a
  file, the member's name inside the zip archive at `path` otherwise);
- `near_misses[]`: `kind` (`name-other-size`, `name-other-hash`,
  `size-other-hash`), `library`, `path`, `member`, `size`, `sha256` (null
  when not hashed);
- `members[]`: `step`, `path`, `size`, `sha256`, `found`, `near_misses`;
- `payloads[]`, only when a pinned recipe has an output it looked up by
  its locked payload: `kind` (`medium`, `rom`, `extra`), `id`, `part` (a
  ROM's chip, from 0), `name` (the file name), `from`, `size`, `sha256`,
  `found`;
- `lock_lost`, only for a pinned recipe whose lock is lost: the lock's
  path;
- `steps[]`: `id`, `verb` (`unpack`, `fat`, `text`), `operand`, `status`
  (`runs`, `blocked`, `not-needed`), `reason`, `missing` (input ids),
  `detail` (`opens ARCHIVE for N of M members`, or null);
- `outputs[]`: `kind` (`medium`, `rom`, `extra`, `attachment`), `id`,
  `label`, `from`, `optional`, `status` (`ready`, `blocked`), `missing`.

A recipe with findings prints `{recipe, complete: false, summary:
"refused: N findings", findings}` with `--json`. Exit status: 0 when every
input the artifact needs is found or not needed, 1 when one is missing or
the recipe has findings, 2 on a usage error, a file or a `--library`
folder that cannot be read, or a `--store` that is not a store.

Zip member names are printed as the archive stores them. A member whose
name is neither ASCII nor flagged UTF-8, or that another member shares, is
reported as ambiguous and never searched, as is one with ZIP64 fields. A
deflated member is decoded only up to the size the archive's directory
states: one that expands further fails, is reported, and is not tried
again until the archive changes.

The `library` commands exit 0 on success, 1 on a failure (a folder named
on the command line that is not there, a preferences file that cannot be
read or written, `remove` of a folder that is not a library, `scan`
without a store), and 2 on a usage error. A library of the preferences
that is not there is a warning for `library scan`, which scans the others,
and `library ls` prints it as `missing`.

## Building recipes

`unemu recipe build` builds a recipe into the store
([design/recipes.md](design/recipes.md) §3.1, §6.9, §6.12, §6.14): it plans
as `recipe plan` does, reads every value it found and checks it against its
size and SHA-256 again, runs the steps (`unpack`, `text`, `fat`) and hands
the outputs to the importers, the same plan-and-publish functions `image
media import` and `image firmware import` end in. The importer copies and
hashes every file and tags the artifact last.

```sh
cargo run --offline -p window -- recipe build my.media.toml --library /path/to/games
cargo run --offline -p window -- recipe build my.media.toml --fetch --store /path/to/store
cargo run --offline -p window -- recipe build my.media.toml --keep-as 1.0-mine-r1
cargo run --offline -p window -- recipe build my.media.toml --deny-drift
```

- The tag is `<name>:<release>-r<revision>` (`<name>:r<revision>` without a
  release), in the store of `--store DIR` or the default store, created
  when it does not exist yet. `--keep-as TAG` tags the result
  `<name>:TAG` instead, a revision tag of the recipe's own repository other
  than the recipe's tag. A recipe without `digest` is a draft: nothing is
  compared with a pin, and the output ends `draft`. A pinned recipe is
  built as Pinning recipes below says, and the output ends `locked`.
- `--fetch` is the consent to download (recipes §6.13). Without it a
  missing input is reported, as `recipe plan` reports it, and nothing is
  downloaded. With it, URLs of each missing required file input are
  tried in order until one succeeds; inputs with the same SHA-256 and size
  are downloaded once. The list of what will be downloaded is printed first
  (`fetch: downloading N inputs (BYTES bytes), as --fetch allows:`, then one
  line per input with its size, SHA-256 and URLs). Each missing input without
  a URL is reported before downloads: `fetch: input ID has no URL; will look
  for it by hash after downloads`. If none can be downloaded, the build says
  `fetch: nothing could be downloaded: no missing required file input has a URL`
  and reports the missing inputs without making requests. After downloads,
  the plan searches by hash again, including file-set entries, and reports
  any inputs still missing. Only `https` is fetched, redirects only to `https`,
  at most the pinned size; connecting may take 30 s, each read 60 s, and a whole
  download 120 s plus one second per 32 KiB of the pinned size (an average
  of at least 32 KiB/s). A download is written to
  `cache/downloads/.partial-<sha256>-<process>-<n>`, a file of its own, and
  renamed to `cache/downloads/<sha256>` only when its size and SHA-256 are
  the pinned ones; the plan then runs again and takes it from there, and so
  does every later plan and build.
- A value is read from where the plan found it: the store, the download
  cache, beside the recipe or a library (a zip member decoded no further
  than its stated size). The members of an `unpack` step found elsewhere
  stand for the archive; the archive is opened only for a member still
  missing, and then checked whole: no member name absolute, with a drive,
  `..` or an ambiguous spelling, and every stated size, hash and time
  equal to the archive's. A step reads only the bytes and keys it is
  given: no clock, no network, no host name or time.
- The outputs are written to `cache/recipe-build/<process>-<n>` in the
  store under their file names, handed to the importer and removed, also
  when the build fails. A file name that is not one path component on
  every host (a `/` or `\`, a `:`, `.` or `..`) is refused. When a build
  starts it removes the staging folders of processes that no longer run
  (on Unix) and `.partial-*` downloads older than a day. A media
  recipe's set takes the media ids, labels, formats and file names, the
  extras' DOS names and kinds, the `[license]` statement and the title
  record merged with the work file (`recipe show`), unchanged. A firmware
  recipe's ROM is identified in the firmware catalogue by its bytes, as
  `image firmware import` identifies it; `[package]` gives the metadata
  flags and the `[license]` source the `--source` archive. Attachments are
  not built in this version; the output names each one.

After a build, one line per input says how it is kept and in which of
the artifact's files, named as `image export` writes them (§3.10 of the
design), so that a person who means to delete the originals knows what
stays:

```text
  archive: members kept as files in program.img; members kept as stored in B.DAT; not kept: the archive itself, 1 member unused
  boot: kept as stored in boot.img
  game: kept as files in data-1.img, data-2.img
  notes: kept as text with DOS line ends in README.TXT
```

`kept as stored in FILE`: the input's bytes are that file unchanged.
`kept as files in FILE, …`: a `fat` step built them into the volumes of
these media, whose files `image export --files` writes back with the
recipe's names and times (never the host times the copies had). `kept as
text with DOS line ends in FILE`: a `text` step changed the line ends. For
an archive an `unpack` step opened, a way through that step keeps its
members (`members kept as …`), and unless the archive is also kept as
stored, `not kept: the archive itself` follows, with `N members unused` for
the file members the recipe did not take; a recipe that wants the archive
kept lists it as an extra (`kept as stored in GAME.ZIP; members kept as
…`). An input the build did not need
says `not needed: …` (an archive whose members were found). A pinned recipe's output found instead of made
prints `  KIND ID (FILE): found WHERE, not made`. The last line is `built
NAME:TAG<TAB>DIGEST<TAB>draft` (`locked` for a pinned recipe), or
`already built …` when the tag names this artifact already. A pinned
build whose work file has changed since the pin prints one `  drift: …`
line per field before it.

Exit status: 0 when the artifact is tagged or was already; 1 when the
recipe has findings (the check's report is printed), an input is missing
(the plan is printed, then `not built: missing M of N inputs; nothing was
tagged`), or a download, a read, a step or the importer fails (the reason on
standard error), or a pinned recipe's outputs differ from its lock (each
mismatch on standard error, then `FILE: not built: NAME differs from its
lock; nothing was tagged under its tag`); 2 on a usage error, a folder, a
recipe or work file that cannot be read, or a store that cannot be opened.
Nothing is tagged unless the exit status is 0, except the `--keep-as` tag of
a pinned build whose outputs differ.

## Pinning recipes

`unemu recipe pin FILE` is the author's build, once per revision
([design/recipes.md](design/recipes.md) §3.1, §3.8, §7.2): it builds the
draft as `recipe build` does (same options, without `--keep-as`), tags it,
then writes the lock beside the recipe and the manifest digest into it.

```sh
cargo run --offline -p window -- recipe pin my.media.toml --library /path/to/games
cargo run --offline -p window -- recipe pin crates/pc-image/data/recipes/vgabios/vgabios-0.9d-unemu.1-std.firmware.toml --library vendor
```

- **The lock** is the folder `FILE.lock` beside the recipe, holding
  `sha256/<hex>` files, each named by the lower-case SHA-256 of its bytes:
  the manifest the importer wrote, its config, and every other layer no
  step made (the title metadata of a set; the generated README of a
  firmware package), exactly the bytes the store holds. It holds no payload
  byte. Written again from the same artifact it is the same files; a new
  lock is written beside the old one and renamed into place.
- **The digest** goes into the recipe as `digest = "sha256:…"` on the line
  after `revision`, in the file's own line ends; nothing else in the file
  changes. The recipe is then checked again, lock included; if that fails,
  the file is restored and the lock removed.
- If the check after writing fails, the recipe file is restored, the new
  lock removed and any lock the draft had beside it put back. The tag the
  build wrote stays: it names exactly what `recipe build` makes of the
  draft, and a revision tag never moves or goes.
- A recipe that already has `digest` is not pinned again (exit 1, `already
  pinned to …`): an artifact that changes is a new revision, so raise
  `revision`, remove `digest` and pin. A recipe whose lock is lost (nothing
  at all at `FILE.lock`) fails its checks (rule 8), but `recipe build`
  still builds it (design/recipes.md §7.2): it makes the manifest, config
  and metadata with its own writers in a scratch store
  (`cache/recipe-lost-lock/<process>-<n>`, removed after). When they give
  the pinned digest, the build goes on as with the lock and prints `  lock:
  lost; this build's writers made the pinned digest DIGEST`; otherwise it
  fails with both digests (`FILE: the lock FILE.lock is lost; this build
  makes sha256:…, the recipe pins sha256:…: the recipe needs its lock; …`,
  whether its writers or a step write other bytes) and tags nothing, `--keep-as` included. No lock is written
  back; restore it from where the recipe was published. A lock folder that
  is there and broken is refused.
- The last line is `pinned NAME:TAG: digest DIGEST written into FILE, lock
  FILE.lock (N files)`. Exit status as `recipe build`'s.

**Building a pinned recipe.** `recipe build` checks the lock before it reads
any input (as `recipe check` does), looks up every output a step makes by
its locked payload before it plans the steps (design/recipes.md §3.4,
"found instead of made"; a found output is read and checked by its SHA-256,
never taken by its name, and the steps and inputs only it needed are not
run or read), runs the steps the other outputs need, then compares every
output with its descriptor in the locked manifest, found through the locked
config (media in order, extras by file name, ROMs and their chips): the
decoded SHA-256 and size, then the stored SHA-256 and size after encoding
it as the descriptor says (raw, or Zstandard encoder profile 1 for a
`+zstd` type). Every layer must be an output or a lock file. Only then is
anything written: the payloads, the lock files and the manifest go into the
store unchanged and the tag is written last. No writer of this build
writes a manifest, config or metadata, so a later release, whose writers
spell those files differently, publishes the digest the recipe was pinned
to. The title record is the lock's whatever the work file says today. A
mismatch names the output, the expected and the actual SHA-256, and tags
nothing; with `--keep-as TAG` the result this build's writers make is kept
under `NAME:TAG`, never under the recipe's tag. On a pinned build that
matches its lock, `--keep-as TAG` tags the locked artifact `NAME:TAG`
instead of the recipe's tag, as it does for a draft. The build compares the
lock with the recipe again after the steps, so a lock or recipe file
changed since the checks refuses it too.

`recipe build FILE --deny-drift` returns 1 on work-file drift before
reading inputs, fetching, opening the destination store or tagging anything,
including with `--keep-as`. Without the flag, the build still warns and
publishes the lock's values. A clean recipe, including a lost-lock rebuild,
keeps the existing build exit codes. `recipe pin` and `recipe show` are
unchanged.

## Recipe format 1.0 compatibility

The proposed freeze is [decision-43](../decisions/decision-43%20-%20Recipe-format-1.0-frozen.md).
Acceptance belongs to the maintainer; recipes are published only after it.
This developer reference carries the promises of [design/recipes.md §9](design/recipes.md#9-what-binds-forever)
until user documentation is written after the front-end split and takes them over.

- An input is always its SHA-256 and exact size; names and indexes are hints.
- `recipe = "1.0"` selects the TOML 1.0 subset, schema and exact step bytes.
  This release refuses experimental `0.1` and unknown versions. Future
  releases read every version from 1.0 through their own; an existing
  version never gains a key or changes interpretation.
- A new minor adds keys, steps, formats or corrected step bytes. The old
  behaviour and tests remain; the author opts in by raising `recipe`.
  Grammar or an existing meaning changes with a new major and new media types.
- Locked payload encoding is exact, including stored `+zstd` output under
  encoder profile 1. A later encoding needs a distinct profile marker.
- Old configs, metadata and manifests remain readable. Their writers may
  change: the lock preserves the author's bytes, annotations and output digest.
  Serialization, annotation order, default filenames and calculation of
  `unemu.min-app-version` are not frozen.
- Tag grammar, revision `rN` and the `recipes/` repository mapping stay fixed.
- Provider vocabularies (platforms, formats and their sizes, volume profiles,
  firmware roles and needs) are append-only; names never change meaning.
- Recipe artifact media types and annotation keys remain stable; another
  major uses new media types. Work files use the same recipe version and
  retain the merge and locked-metadata rules described above.
- Export layout is separately versioned by `EXPORT-VERSION` (currently `1`).

The plan/report JSON has its own version and may change. Library indexes
are caches; resolution order and download consent may evolve. Machinefiles
and later features of §13 stay open; the installation syntax corpus is
evidence, not an implementation. Attachments retain their declared schema
and meaning, but their build implementation remains deferred.

The editor schema is [recipe-schema-1.0.json](design/recipe-schema-1.0.json),
generated from the recipe tables and PC provider. `pc-image`'s
`the_published_schema_is_the_generated_one` test refuses drift; reproduce
the frozen 1.0 schema with `UNEMUPC_WRITE_RECIPE_SCHEMA=1 cargo test -p pc-image
--test recipe_check the_published_schema_is_the_generated_one`. The release
workflow attaches that same file as a checksummed release asset. A JSON
Schema cannot replace `recipe check` for TOML syntax, references, licences,
content hashes or provider size rules.

## Recipe conformance

`unemu recipe conformance FILE|FOLDER... [--library DIR]... [--store DIR]
[--config FILE] [--allow-drafts]` builds every pinned recipe of the files
and folders named into a scratch store, from the libraries only (the
preferences' first, then each `--library`; no download, no other store),
and compares the digest it
publishes with the recipe's `digest` ([design/recipes.md](design/recipes.md)
§6.12, §9). The scratch store is `cache/recipe-conformance/<process>` in
the store of `--store DIR` or the default store, and is removed when the
run ends; the store keeps only the libraries' index caches. Drafts are
listed and not built; work files are skipped. No output is found instead
of made: conformance proves that the steps still make the pinned bytes. A
pinned recipe whose lock is lost is built as `recipe build` builds it
(Pinning recipes): it conforms when this build makes the pinned digest,
and is `not built: …` with the lost-lock message, both digests in it,
otherwise.

```sh
# this repository: the shipped template recipes and the pinned Ken's Labyrinth recipe
bash scripts/core-checkout.sh
fixture=$(bash scripts/core-checkout.sh --print-root)
export "$fixture"
bash vendor/freedos/fetch-floppy-1.4.sh
bash vendor/kenslab/fetch-labfull.sh
cargo run --offline -p window -- recipe conformance \
  crates/pc-image/data/recipes \
  backlog/docs/design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml \
  --library vendor/freedos --library vendor \
  --library "$UNEMUPC_FIXTURE_ROOT/vendor"
# a repository of recipes, against a release of unemu and its own library
unemu recipe conformance recipes/ --library /path/to/library --store ./conformance-store
unemu recipe conformance recipes/ --allow-drafts --library /path/to/library --store ./conformance-store
```

One line per recipe: `FILE: conforms: NAME:TAG<TAB>DIGEST`, `FILE: differs:
…` (the mismatches), `FILE: not built: …` (an input missing, or why it
failed) or `FILE: draft, not built`; then `N pinned recipes: C conform, D
differ, M not built; K drafts skipped`. Exit status: 0 when every pinned
recipe conforms and no draft is present (or `--allow-drafts` was given),
1 when a draft is present without `--allow-drafts`, one differs or was not
built or when no pinned recipe was found (`unemu recipe conformance: no
pinned recipe found …` on standard error: a run that finds nothing proves
nothing), 2 on a usage error
or a store or file that cannot be opened. `--allow-drafts` only permits
drafts: a failed pinned build or a run with no pinned recipe still exits 1.
The test `the_pinned_corpus_builds_the_digests_it_pins` (`pc-image`,
`recipe_lock`) runs the same harness on the corpus's six pinned open recipes,
the three FreeDOS 1.4 floppy sets and the three open firmware packages.

The required release gate is `bash scripts/ci-gate.sh recipe-conformance`.
It runs the six pinned shipped recipes and the pinned
`design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml`, with
`--library vendor/freedos --library vendor --library "$UNEMUPC_FIXTURE_ROOT/vendor"`
and without `--allow-drafts`, after verifying the core checkout with
`bash scripts/core-checkout.sh --print-root`.
The other corpus files are unpinned design examples, not release inputs.
Every new pinned recipe must join this gate. Missing fixture inputs fail
release just as a digest difference does. Ken's original archive and licence
index must be provided with the exact sizes and hashes in its recipe;
changing the pinned recipe to fit available files is not conformance.
The release upload depends on both binary builds and this gate, including
the schema drift check. This developer command requires the Nix devShell.

## Scaffolding recipes and listing their inputs

`unemu recipe scaffold FILE|FOLDER...` prints the tables a recipe needs for
files the author has, measured instead of typed
([design/recipes.md](design/recipes.md) §6.8, §6.9, §6.15). It reads the
files and writes nothing; the author adds the identity, the description,
the outputs and whatever else the recipe states, and keeps or renames
what scaffold wrote. `unemu recipe inputs FILE --format
dat|csv|sha256sums` prints the inputs of a recipe that passes the checks,
for a ROM manager or `sha256sum -c`.

```sh
cargo run --offline -p window -- recipe scaffold LABFULL.ZIP AZURAMON
cargo run --offline -p window -- recipe inputs my.media.toml --format sha256sums
```

- **A plain file** gives `[input.ID]` with `name` (the file's own name),
  `size` and `sha256`.
- **A zip archive** (a name ending in `.zip`, any case) gives the same
  table and an `unpack` step, `ID-files`, with `complete = true` and every
  file member in the archive's own order: `path`, `size`, `sha256` and the
  `time` its central directory states (a member without a valid DOS time,
  such as a day its month lacks, has none). Folder entries are skipped. A build reads nothing but bytes
  from an archive, so its order and times are written here, once. An
  archive an `unpack` step would refuse is refused (ZIP64, a member name
  that is ambiguous, unsafe or holds a backslash, an encrypted member, a
  method other than stored and deflate).
- **A folder** gives a file-set input of every file below it, in every
  folder its files, then its folders, each group by the 8.3 name as a
  directory entry stores it: the 11-byte field, stem and extension padded
  with spaces, the order of a `fat` step's `order = "name"` (`A.TXT`
  before `A-B.TXT`, `AB.TXT` before `AB-C.TXT`). Names that are not valid
  8.3 names as stored (lower case, too long, other characters) come after
  them, by the bytes of the name; a `fat` step refuses them. Paths are
  relative to the folder, and there are no times: a host file's time is the
  time of the copy. Hidden files and
  folders (`.` first) are skipped, symbolic links followed, and a link back
  into a folder above is refused. At most 100,000 files over all folders
  named (the recipe's limit); the walk stops at the first one past it. As
  a recipe holds at most 4096 inputs and 64 steps, at most 4096 files and
  folders and 64 archives are named.
- Ids are the names in lower case, a file's last extension dropped, other
  characters than letters, digits, `_` and `-` written as `-`, a repeated
  id with `-2`, `-3`. Input tables come first, in the order named, then
  the steps. No host path is written.

```toml
[input.game]
name = "GAME.ZIP"
size = 423
sha256 = "89cd77b95c37c44993bb4192279315e55ef5cea95856be17480e1b5aa99cf58c"

[input.set]
name = "SET"
files = [
    { path = "A.TXT", size = 2, sha256 = "87428fc522803d31065e7bce3cf03fe475096631e5e07bbd7a0fde60c4cf25c7" },
    { path = "SUB/C.DAT", size = 3, sha256 = "aa9e9b5c907d50feb410f2a84e81ab72c5ae6724d57ccc53650f9361d33dc734" },
]

[[step]]
id = "game-files"
unpack = "game"
format = "zip"
complete = true
members = [
    { path = "README.TXT", size = 9, sha256 = "0378767b3aa93349bb79cdd0ba46bd5979bb7adfb8cbcfe2404097479935be50", time = 1993-03-27T13:58:22 },
]
```

Exit status of `scaffold`: 0 when the tables are printed (a warning on
standard error when they pass the 1 MiB a recipe may have), 1 when a file
or folder cannot be held by a recipe (a name or member it refuses, an
archive `unpack` refuses, more than 100,000 files, 4096 inputs or 64
archives, a folder without files), 2 on a usage error or a file that cannot
be read or changed while it was read.

`recipe inputs` describes one folder: each file input at its `name`, each
file-set entry at the set's `name`, `/` and its path. Inputs with
`bundle = true` travel with the recipe and are left out; members of an
`unpack` step are not inputs and are not listed. Inputs come in the order
of their ids, a file set's entries in the set's order. Two inputs at one
path with the same size and hash are one line; with other bytes, or a file
at a path another input needs as a folder, the folder cannot hold both,
and the command exits 1 naming the path and both inputs. Nothing but the
recipe is read.

- `--format sha256sums`: `<hex>  <path>` per line, as `sha256sum` writes;
  `sha256sum -c FILE` run in that folder checks it.
- `--format csv`: the header `path,size,sha256,crc32,sha1,md5`, a line per
  file, empty fields for the weaker hashes the recipe does not state, a
  field with a comma or a quote quoted (RFC 4180), LF line ends.
- `--format dat`: a Logiqx datafile (the XML the No-Intro, Redump and TOSEC
  DATs use) with a header naming the artifact (`NAME`, `NAME:TAG`, `TAG`,
  author `unemu recipe inputs`) and one game, `<last component>-<tag>`,
  whose `rom` entries carry `name`, `size`, `crc`, `md5` and `sha1` when
  stated, and `sha256`. A ROM manager that rebuilds that game makes the
  folder, which is then a library `recipe build` finds every input in.

```text
GAME.ZIP,423,89cd77b95c37c44993bb4192279315e55ef5cea95856be17480e1b5aa99cf58c,,,
<rom name="SET/A.TXT" size="2" sha256="87428fc522803d31065e7bce3cf03fe475096631e5e07bbd7a0fde60c4cf25c7"/>
87428fc522803d31065e7bce3cf03fe475096631e5e07bbd7a0fde60c4cf25c7  SET/A.TXT
```

Exit status of `inputs`: 0 when the list is printed, 1 when the recipe has
findings (the check's report is printed instead) or two inputs clash, 2 on a usage error, a
work file, a folder or a file that cannot be read. The tests are
`recipe_scaffold` in `cli-pc`; its two `user_rom_` tests scaffold the
user's own LABFULL.ZIP and AZURAMON folder
([`vendor/user-roms/README.md` in unemupc](https://github.com/unemu-org/unemupc/blob/main/vendor/user-roms/README.md))
and compare with the corpus's tables.

## Recipes on a registry

A pinned recipe is published as a registry artifact without any payload
([design/recipes.md](design/recipes.md) §7, decision-35): `unemu recipe
push FILE REF` sends it, `unemu recipe pull REF` writes it into a folder
where it plans and builds with no other file, and `unemu recipe plan REF`
judges it from its manifest and config alone. The registries, credentials
and `--registry` are those of `image push` and `image pull` (Registry push
and pull).

```sh
unemu recipe push my.media.toml unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1 --dry-run
unemu recipe push my.media.toml unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1 --store /path/to/store
unemu recipe pull unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1 --to recipes/freedos
unemu recipe plan unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1 --library /path/to/games
unemu recipe build recipes/freedos/freedos-1.4-720k.media.toml --library /path/to/games
```

- **The artifact.** Its `artifactType` is
  `application/vnd.unemu.recipe.media.v1` or
  `application/vnd.unemu.recipe.firmware.v1`; its config is the recipe file
  (`application/vnd.unemu.recipe.v1+toml`), so the config digest is the
  recipe's digest; its layers are the work file the recipe names
  (`application/vnd.unemu.recipe.work.v1+toml`), each input with `bundle =
  true` (`application/vnd.unemu.recipe.input.v1`) and each file of the
  lock (`application/vnd.unemu.recipe.lock.v1`), in that order. Every
  descriptor, the config's included, names its file in
  `org.opencontainers.image.title`: `NAME.media.toml`, `WORK.work.toml`,
  the input's `name`, `NAME.media.toml.lock/sha256/<hex>`. Each part is at
  most 1 MiB. The manifest carries `unemu.platform`, `unemu.recipe.output`
  (`name:tag`), `unemu.recipe.output.digest`, the title and description of
  the output, `org.opencontainers.image.licenses = "CC0-1.0"` and
  `unemu.min-app-version`, the oldest release that can build the recipe:
  the newest row of its recipe version, its steps, the artifact's media
  types (all 0.0.3 today) and the marker of its locked output.
- **`recipe push FILE REF [--dry-run] [--registry HOST] [--store DIR]
  [--config FILE]`.** The recipe must pass its checks, be pinned and have
  its lock beside it; its work file and bundled inputs are read from its
  folder and checked against the recipe (a bundled file with another size
  or hash is refused, exit 1). Two bundled inputs with the same bytes or
  name, or one with the bytes of the work file, are a finding of rule 3:
  an artifact names each blob once. `REF` is the publisher's
  namespace, `recipes/` and the output's path after its first component,
  with the output's tag: `private/media/ibm-pc/windows` at `3.0-1.2m-r1`
  is published as `unemu/recipes/media/ibm-pc/windows:3.0-1.2m-r1`. The
  parts are staged as untagged blobs in `--store DIR` or the default store
  (created when missing) and uploaded as `image push` uploads; the manifest
  goes last. The guard refuses, before anything is sent: a part that is
  none of the recipe's work file, bundled inputs and lock files, or one
  they name that is missing; a lock file the lock does not need (rule 8);
  a bundled input that is not text (UTF-8 without NUL); a recipe without
  `[license]` that names an input or a file-set entry under 256 bytes not
  marked `disclosable = true`, or an `unpack` member under 256 bytes not
  marked so (the author marks what may be published; `recipe scaffold`
  never writes the key); another place than the output's; an
  annotation that does not say what the recipe says; a marker below what
  this release computes. The registry rules of `image push` apply to the
  target (a configured registry, a listed namespace); the parts count as
  open, as a recipe, its bundled texts and its lock are our own text. A tag
  that already names this artifact is `unchanged`. A tag that names a
  recipe of the same output digest is replaced, `moved TARGET OLD -> NEW
  (the same output DIGEST)`, without `--move`: a better recipe (a mirror
  URL, another database id) replaces the old one. A tag that names any
  other manifest is refused (`recipe tag … names …; a recipe tag is
  replaced only by a recipe that pins the same output digest, …`), and
  `--move` does not exist here. `--dry-run` prints `target:`, `image:`,
  `recipe:`, `output:NAME:TAG<TAB>DIGEST`, a line per part (`config` or
  `layer`, digest, size, media type, title) and `guard:allowed` or
  `guard:refused`, and sends nothing. Exit status: 0 sent or unchanged, 1
  the checks' findings (printed), a refusal or a registry failure, 2 a
  usage error, a file that cannot be read or a registry configuration
  error.
- **`recipe pull REF [--to DIR] [--registry HOST] [--config FILE]`.** The
  manifest is read first: one whose `unemu.min-app-version` this release
  does not meet is refused (`REF needs unemupc X or newer (this is Y)`)
  before the config or any part is fetched. Then the config and every part
  are fetched and verified, the artifact is read strictly (types, sizes,
  titles, the parts against the recipe) and checked laid out in a private
  scratch folder (a new hidden `.recipe-scratch-…` folder inside `DIR`,
  mode 0700, removed with its contents after), rule 8 included; only then
  are the parts written into `DIR` (the current folder by default) at
  their titles: the recipe, its work file, its bundled inputs and
  `NAME.lock/sha256/`. A file already there with the same bytes is kept;
  one with other bytes, a folder at a file's place or a symbolic link on
  the way to a part is refused before anything is written; when a write
  fails, what the pull wrote is removed again, and a `DIR` it created too.
  A push inspects its artifact the same way under `cache/recipe-push/` of
  its store. Lines: `fetched<TAB>DIGEST<TAB>TITLE` per part, then
  `pulled<TAB>REF<TAB>DIGEST<TAB>RECIPE`. Exit status: 0 written, 1 a
  refusal or a registry failure, 2 a usage error or a registry
  configuration error.
- **`recipe plan REF`.** An operand that is not a file, whose last
  component names a tag or a digest and that parses as a registry
  reference (`[HOST/]NAME:TAG` or `NAME@sha256:…`) is a registry
  reference; any other missing path is a file that cannot be read, exit 2. The row of
  the Recipes group (design/recipes.md §7.4) is judged from the manifest
  and the config alone, no part of the bundle fetched: the recipe text is
  checked with its work file and lock counted as held, and planned against
  the store, the download cache and the libraries as a file is; a bundled
  input the manifest lists with the recipe's size and hash is found
  (`beside the recipe: NAME`). A recipe whose marker this release does not
  meet is judged from its manifest alone: `REF: Needs unemupc X or newer;
  nothing but its manifest was fetched`, exit 1. `--json` gives the plan's
  JSON, or `{"file": REF, "needs": …}`.
- **Local tags built from a recipe.** `recipe build` and `recipe pin`
  record on the tag they create the digest of the recipe text that built
  it, `unemu.build.recipe = "sha256:…"` (for a pulled recipe its
  artifact's config digest), in the store's `index.json` only, beside
  `unemu.made-here`: no manifest and no image digest changes. A pin
  records the pinned text. A tag that was there already keeps what it
  had; moving the tag to another image, or a pull, drops the record.

The tests are `recipe_registry` in `cli-pc` (the mock registry of the
registry tests, no network) and the store test
`a_tag_records_the_recipe_that_built_it_until_it_moves`.

## Exporting an image

For a library machine's next-run medium, use [`image machine export`](#exporting-a-machines-next-run-medium).

`unemu image export NAME:TAG --to DIR` writes an installation set or a
firmware package of the store as plain files ([design/recipes.md](design/recipes.md)
§3.10, decision-37),
so the originals can be deleted and the files had back without the store:

```sh
cargo run --offline -p window -- image export private/media/ibm-pc/azuramoon:2026-r1 --to azuramoon --files
cargo run --offline -p window -- image export private/firmware/ibm-pc/opt-rom:r1 --to opt-rom --store /path/to/store
```

The layout, version 1 (`EXPORT-VERSION` holds `1` and a line feed):

```text
DIR/
  x86BOOT.img        every layer's decoded bytes under its file name: the
  x86DSK01.img       title on its descriptor, without .zst on a compressed
  LICENSES.TXT       layer (media, ROM images, extras, a package's README.md)
  metadata.json      a set's metadata layer, as stored
  config.toml        the config, as stored
  manifest.json      the manifest, as stored
  EXPORT-VERSION     1
  SHA256SUMS         sha256sum -c lines over all of the above, by the bytes
                     of the names; written last
  files/             with --files only: the disks' files
    FDAUTO.BAT
    freedos/bin/attrib.com
```

- An unmodified medium is the input byte for byte; so is an extra. No
  layer takes one of the names the export writes itself (machine-images
  §2, Export file names), so nothing in `DIR` is ambiguous.
- `--files` also writes the content of every diskette of a set that reads
  as a FAT12 volume into the folder `files`, the volumes merged into one
  tree in the set's order, as the volumes of a split set make up one tree.
  A name is the entry's long name when it has a valid one, else its 8.3
  name as stored, with the stem or the extension in lower case when the
  entry's case bits (byte 12, as Windows NT, Linux and mtools write them)
  say so; a file's and a folder's modification time is its entry's
  last-write time taken as UTC (a DOS time has no zone, so the export gives
  the same times on every host); attributes are not set. A folder that
  several volumes hold is one folder with the first one's time; a file that
  several hold must have the same bytes. Other bytes, a file on one volume
  and a folder on another, a damaged volume (a broken or looping chain, a
  cross-linked cluster), a name a host cannot take or longer than 255
  bytes, a folder deeper than the 64 characters DOS keeps for a path
  (`\A\B` in 8.3 names, at most 32 levels) or a path of host names longer
  than 260 characters (MAX_PATH) refuse the export; a
  medium that is not a FAT12 volume (an optical image, a disk of another
  format) is printed as `not read<TAB>NAME<TAB>REASON` and left out of the
  tree. `SHA256SUMS` does not list the tree: the images it lists hold it.
  A package has no disks, and `--files` adds an empty `files`.
- Nothing is written before every name has been checked: a file already in
  `DIR` with the same bytes is kept, and one with other bytes, a folder
  where a file goes, a file where a folder goes or a symbolic link refuses
  the export with nothing written. Each file is written under a temporary
  name beside it and renamed. No file of the export names a host path.
  One case the check cannot see: names are compared by their bytes, case
  aside, not by Unicode normalization (the program carries no
  normalization table). Two long names that differ only in composition,
  `é` as one code point or as `e` and a combining accent, are two files on
  Linux and one on macOS; there the second meets the first while the
  export writes, and the export stops and removes every file and folder it
  wrote, leaving `DIR` as it was (MEASURED on macOS, `cli-pc` test
  `names_one_host_takes_as_one_file_leave_nothing_behind`). Folders with
  such names merge on macOS.
- Output: a `not read` line per medium `--files` left out, `exported
  NAME:TAG<TAB>DIGEST<TAB>N files`, and with `--files` `files<TAB>N
  files<TAB>M folders<TAB>K volumes`. Exit status: 0 exported, 1 refused
  (the reason on standard error), 2 on a usage error. A machine image is
  refused: export writes sets and packages. Saving whole artifacts as OCI
  Image Layouts is other work (`upc-5pw.116` (unemupc)).

The same files without `unemu`: `scripts/export-layout.sh [--files] LAYOUT
REF DIR` reads any OCI Image Layout, such as the `images` folder of a store
or what `oras copy --to-oci-layout` saves, with a POSIX shell, `jq`,
`sha256sum` and `zstd`, and for `--files` mtools (`mdir`, `mcopy -s -m`
under `TZ=UTC0`). REF is the image's `org.opencontainers.image.ref.name` in
`index.json` (a store's `NAME:TAG`) or its manifest digest. It checks every
blob against its digest and every decoded layer against
`unemu.decoded.digest`, and checks the names as `unemu` does (no name the
export writes itself, no two layers' names equal ignoring case, no `files`
with `--files`) before it writes anything. It runs under a UTF-8 locale
(`C.UTF-8` or `en_US.UTF-8`; it stops without one), with mtools reading no
configuration (`MTOOLSRC=/dev/null`). Unlike `unemu` it writes only into an
empty or absent `DIR`; like it, it prints `not read<TAB>NAME` for an optical
image and a diskette it cannot read. All its tools are in the devShell;
the test that compares the two exports fails without one of them when the
fixtures are required (`UNEMUPC_REQUIRE_FIXTURES=1`) and skips otherwise.

```sh
scripts/export-layout.sh --files /path/to/store/images private/media/ibm-pc/azuramoon:2026-r1 azuramoon
cd azuramoon && sha256sum -c SHA256SUMS
```

MEASURED (`cli-pc` test `image_export`): the command and the script write
the same files, bytes and the unpacked files' and folders' times, for a
synthetic two-volume split set with an extra and a blank image that is no
FAT volume, a synthetic firmware package, and the FreeDOS 1.4 1.44M set (7
real disks with long names), and a disk mtools wrote with lower-case 8.3
names (case bits) and a mixed-case long name; `sha256sum -c` accepts
`SHA256SUMS`; the refusals above leave `DIR` as it was. With the maintainer's own files
(`UNEMUPC_REQUIRE_USER_ROMS=1`, ignored test
`user_rom_azuramoon_exported_with_files_is_the_users_folder`), Azuramoon
built from the user's folder and exported with `--files` is that folder,
561 files byte for byte under their own paths.

## Creating disk images

```sh
cargo run --offline -p window -- media create floppy --type 1.44M --format --label TRANSFER /path/to/transfer.img
cargo run --offline -p window -- media create floppy --type 1.44M --add /path/to/GAME --label GAME /path/to/game.img
cargo run --offline -p window -- media create floppy --type 1.44M --add /path/to/PLAY.EXE --add /path/to/DATA.DAT --top MYGAME /path/to/mygame.img
cargo run --offline -p window -- media create floppy --type 1.44M --add /path/to/BIGGAME --label BIGGAME --split /path/to/big.img
cargo run --offline -p window -- media create hd --size 32MiB /path/to/disk.img
```

`--add PATH` (repeatable; folders whole, symbolic links never followed) builds
a new FAT12 floppy from host files and implies `--format`. A named path that is a
symbolic link, host metadata or a device is refused, as are two paths reaching
the same host entry. `--top NAME` puts everything under one root folder. Names
become plain DOS 8.3 names; each change prints as `renamed HOST -> DOS-PATH`,
and host junk inside folders (`.DS_Store`, `._*`, `__MACOSX`, `Thumbs.db`,
`desktop.ini`), links, devices and the images being written print as
`skipped HOST: REASON`. A folder path on the disk may have at most 64
characters, counting the leading backslash and `--top`. Identical trees give
identical images: files before folders, each by DOS name, 1980-01-01 dates, no host times. Without
`--split` a misfit is an error naming the shortfall in bytes and nothing is
written. `--split` writes `<stem>-1ofN.<ext>`, `<stem>-2ofN.<ext>`… beside `OUT`
(or `OUT` itself when one disk is enough), fills them in folder order (a
folder's own files before its subfolders), labels them `LABEL 1`, `LABEL 2`…,
repeats the folder path on every disk and refuses a single file larger than a
disk. One line per disk gives its file count and free bytes. Outputs are never
replaced without `--force`. To install a split set, everything must sit under
one folder (one folder `--add`, or `--top` with several sources): run
`MD C:\BIGGAME`, then `XCOPY B:\BIGGAME C:\BIGGAME /S /E` once per disk. Full
rules: [`machines/README.md` (unemupc)](https://github.com/unemu-org/unemupc/blob/main/machines/README.md) § Building a floppy from host files.



## Typecheck / lint / format

```sh
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo clippy --locked -p window --bin unemu -- -D warnings
cargo fmt --check
cargo deny --all-features check advisories licenses bans sources
```

`deny.toml` covers the resolved graph, including pinned cores. New sources and
licence exceptions need review. The PC core owns its font and OPL3 notices;
`ui-kit` owns the logo and DejaVu subset notices. Run dependency checks in an
environment that can acquire the advisory database lock.

## CI

```sh
bash scripts/ci-gate.sh shell
bash scripts/ci-gate.sh fmt
bash scripts/ci-gate.sh guard
bash scripts/ci-gate.sh clippy
bash scripts/ci-gate.sh deny
bash scripts/ci-gate.sh tests
bash scripts/ci-gate.sh fixtures
bash scripts/ci-gate.sh goldens
```

The front-end fixture lane uses `scripts/core-checkout.sh` to build the pinned
PC core's fixtures in that checkout's devShell, then builds this repository's
FreeDOS 1.4 and recipe fixtures. The golden lane requires fixtures and verifies
the locked core revision; missing fixtures fail with `UNEMUPC_REQUIRE_FIXTURES=1`.
CI uses the external core's verified fixture root and never copies core source
or built fixtures into the tracked tree.

### Publication guard

`bash scripts/publication-guard.sh` checks tracked and non-ignored untracked
text for private paths, hosts, addresses and disallowed licence headers.
Everything committed follows [the publication rule](conventions.md#publication).

### Crate boundaries

`bash scripts/crate-boundaries.sh` checks manifest layers and source includes,
including the neutral/core boundary and one resolved copy of `unemu-core-api`.
Use `--self-test` for planted violations. Front-end harness support is imported
through `pc-harness/test-support`, never included from another repository's source.

### Third-party licences

`bash scripts/third-party-licenses.sh [OUTPUT [TARGET]]` generates the release's
`THIRD-PARTY-LICENSES.md` from the resolved dependency graph and its notices,
including both pinned cores. The output is gitignored. `--about` reports the
front-end source revision and each locked core's source revision.

### User-supplied ROM tests

This local opt-in lane is never given proprietary firmware by CI. Only run it
when the supervising brief authorizes it; the PC core owns hardware ROM runners.
The front end retains user-media tests for its import, export and recipe paths.

## Releases

The front end publishes `unemu` from [unemu-org/unemu](https://github.com/unemu-org/unemu).
The core publishes `unemupc` separately. A front-end release tag `v<version>`
matches `[workspace.package].version`; raise front-end compatibility
`NEXT_RELEASE` values past it without changing historical rows. Each core pin
must match a release tag with the package's version; development may pin a green
commit. The release workflow validates this before packaging.

The archives are `unemu-<version>-<target>.tar.gz`, containing `unemu`,
`LICENSE`, `README.md`, `THIRD-PARTY-LICENSES.md` and the corresponding notices.
Release targets are `x86_64-unknown-linux-gnu` (`ubuntu-22.04`),
`aarch64-unknown-linux-gnu` (`ubuntu-22.04-arm`), `aarch64-apple-darwin`
(`macos-latest`) and `x86_64-apple-darwin` (`macos-15-intel`). Both Linux
archives require glibc 2.35 or newer. The Intel macOS runner is supported
until about August 2027; cross-building from arm64 macOS is the fallback.
`SHA256SUMS` verifies downloads. Release notes name the front-end commit and
both locked core revisions. The maintainer authorizes tags, pushes and publication.
Before a release, run the full gate with the authorized fixture and user-media lanes.

To build and check all four targets from an existing tag without changing the
release, run `.github/workflows/release.yml` manually with that `tag` and
`upload=false`. Recipe conformance and builds still run and retain their workflow
artifacts; the upload job is skipped. Manual runs default to `upload=true`,
and publishing a release always uploads.

```sh
sha256sum --ignore-missing -c SHA256SUMS
shasum -a 256 --ignore-missing -c SHA256SUMS
```

### Using unemu in other repositories' CI

The composite action `.github/actions/setup-unemu` installs a released
`unemu` on Linux X64, Linux ARM64, macOS ARM64 or macOS X64 runners, so the
media and firmware repositories import and push with this project's own CLI
and push guard.
Linux runners need the ALSA runtime first (`libasound2` on Ubuntu 22.04).
Its `README.md` has the inputs, the output and an example job. A consumer
pins the action by commit and names the release:

```yaml
- uses: unemu-org/unemu/.github/actions/setup-unemu@<unemu commit SHA>
  with:
    version: v0.0.4
    # commit: <40 hex>   # optional; must match unemu --version
```

The action downloads the archive with curl, checks it against the release's
`SHA256SUMS` before it unpacks anything (a check of the download, not a pin), puts `unemu`, `LICENSE` and `THIRD-PARTY-LICENSES.md` in a
new directory under `$RUNNER_TEMP` and on `PATH`, and prints `unemu
--version`; its `path` output names the binary. Its inputs reach its script,
`install.sh`, only through the environment. Registry credentials stay with
the consumer: `UNEMU_REGISTRY_USER`, `UNEMU_REGISTRY_PASSWORD` and
`UNEMU_REGISTRY_HOST`, on the push step only.

`.github/workflows/setup-unemu.yml` tests the action when its files change:
on `ubuntu-22.04`, `ubuntu-22.04-arm`, `macos-latest` and `macos-15-intel`
it builds the current `unemu`, packages a local test archive and supplies it
through a substituted curl. The real
action checks and installs that archive, then runs `unemu --version` and
`media create`; a missing release is refused without changing PATH. This
checks the renamed binary before a public release exists; hosted release
downloads remain a separate release check. To run
the script by hand, give it the inputs and the runner's variables:

```sh
mkdir -p /tmp/setup-unemu && : > /tmp/setup-unemu/path && : > /tmp/setup-unemu/output
UNEMU_VERSION=v0.0.4 RUNNER_OS=macOS RUNNER_ARCH=ARM64 \
  RUNNER_TEMP=/tmp/setup-unemu GITHUB_PATH=/tmp/setup-unemu/path GITHUB_OUTPUT=/tmp/setup-unemu/output \
  bash .github/actions/setup-unemu/install.sh
gh run list --repo unemu-org/unemu --workflow setup-unemu.yml
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
