# Commands

Everyday commands for this project. Keep this file **flat and copy-pasteable** — agents and humans both grep it.

## Local machine images

```sh
cargo run --offline -p upcli -- image import machines/at-freedos.toml --store /path/to/store --tag dos-lab:1.0 --create
cargo run --offline -p upcli -- image import /path/to/machine.yaml --store /path/to/store --tag dos-lab:recovered --recover medium-0=tip
cargo run --offline -p upcli -- image import /path/to/machine.toml --store /path/to/store --tag dos-lab:checkpoint --recover medium-0=generation:1
cargo run --offline -p upcli -- image import /path/to/machine.toml --store /path/to/store --tag dos-lab:1.1 --min-app-version 0.0.3
cargo run --offline -p upcli -- image compress dos-lab:1.0 --tag dos-lab:compressed --store /path/to/store
cargo run --offline -p upcli -- image ls --store /path/to/store
cargo run --offline -p upcli -- image sweep --store /path/to/store
cargo run --offline -p upcli -- image run dos-lab:1.0 --store /path/to/store --workspace play --max-steps 1000000
cargo run --offline -p upcli -- image run dos-lab:1.0 --store /path/to/store --workspace disposable --new --max-steps 1000000
cargo run --offline -p upcli -- image workspace ls --store /path/to/store
cargo run --offline -p upcli -- image workspace abandon-publication play --store /path/to/store
cargo run --offline -p upcli -- image publish --from-workspace play --store /path/to/store --tag dos-lab:flat --flatten
cargo run --offline -p upcli -- image publish --from-workspace play --store /path/to/store --tag dos-lab:1.1
cargo run --offline -p upcli -- image workspace discard play --store /path/to/store --yes
cargo run --offline -p upcli -- image machine ls --store /path/to/store
cargo run --offline -p upcli -- image machine new dos-lab:1.0 --store /path/to/store --name "DOS games"
cargo run --offline -p upcli -- image machine rename dos-games "Keen" --store /path/to/store
cargo run --offline -p upcli -- image machine duplicate dos-games --store /path/to/store --name "Keen copy"
cargo run --offline -p upcli -- image machine settings dos-games --store /path/to/store --set memory.extended_kib=3072 --dry-run
cargo run --offline -p upcli -- image machine settings dos-games --store /path/to/store --set memory.extended_kib=3072
cargo run --offline -p upcli -- image machine settings dos-games --store /path/to/store --clear
cargo run --offline -p upcli -- image machine run dos-games --store /path/to/store --max-steps 1000000
cargo run --offline -p upcli -- image machine commit dos-games --store /path/to/store --tag dos-games:1.0.0
cargo run --offline -p upcli -- image machine commit dos-games --store /path/to/store --tag dos-games:1.1.0 --title "DOS games" --keep-on-old-image
cargo run --offline -p upcli -- image machine reset dos-games --store /path/to/store --yes
cargo run --offline -p upcli -- image machine rm dos-games --store /path/to/store --yes
cargo run --offline -p upcli -- image media import /path/to/disks --title "Installation set" --tag private/media/ibm-pc/install:1.0-r1 --store /path/to/store --create
cargo run --offline -p upcli -- image media import /path/to/disk1.img /path/to/disk2.img --title "Installation set" --tag private/media/ibm-pc/install:2.0-r1 --label '1=Setup' --extra /path/to/manual.txt:MANUAL.TXT --store /path/to/store
cargo run --offline -p upcli -- image media import /path/to/disc.iso --title "Optical set" --tag private/media/ibm-pc/optical:1.0-r1 --format 1=iso-2048 --store /path/to/store
cargo run --offline -p upcli -- image media import /path/to/game.img --title "Game" --tag private/media/ibm-pc/game:1.0-r1 --need cpu=286+ --need memory=640K --need video=ega,vga --need sound=adlib,pc-speaker,none --requirements "IBM AT or compatible; 640K; EGA or VGA" --work game --synopsis "What the game is." --work-id mobygames=game --store /path/to/store
cargo run --offline -p upcli -- image media import vendor/shareware/wolf3d/wolf3d-sw14-1440.img --title "Wolfenstein 3-D: Episode 1 (shareware 1.4)" --tag private/media/ibm-pc/wolf3d-shareware:1.4-r1 --extra vendor/shareware/wolf3d/extras/VENDOR.DOC:VENDOR.DOC --extra vendor/shareware/wolf3d/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1992 --genre "first-person shooter" --platform DOS --description 'Wolfenstein 3-D, episode 1, Escape from Wolfenstein, version 1.4, the shareware episode of a first-person shooter with texture-mapped VGA graphics by id Software, published by Apogee Software. As B.J. Blazkowicz the player escapes from Castle Wolfenstein through ten floors, one of them secret. It needs a 286 or better. The disk holds the files of the original Apogee shareware archive 1wolf14.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-vendor-v11.01.92 --license-text VENDOR.DOC --store /path/to/store --create
cargo run --offline -p upcli -- image media ls --store /path/to/store
cargo run --offline -p upcli -- image media show private/media/ibm-pc/install:1.0 --store /path/to/store
cargo run --offline -p upcli -- image push private/media/ibm-pc/wolf3d-shareware:1.4-r1 unemu/media/ibm-pc/wolf3d-shareware:1.4-r1 --store /path/to/store --dry-run
```

Machines (machine-images §3.2) are named local instances of images: the
desktop's Machines list on the command line. Each has a fixed id (its
workspace, a slug of the first name; never starting `regular-`, `gaming-`,
`disposable-` or `fresh-`, which get `m-` in front) and a free-text name,
1–64 characters, unique ignoring case; `new` defaults the name to the image
title, then `title (2)`. Every `image machine` command first adopts older
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
cargo run --offline -p upcli -- image media import vendor/freedos/floppy-1.4/144m/x86BOOT.img vendor/freedos/floppy-1.4/144m/x86DSK01.img vendor/freedos/floppy-1.4/144m/x86DSK02.img vendor/freedos/floppy-1.4/144m/x86DSK03.img vendor/freedos/floppy-1.4/144m/x86DSK04.img vendor/freedos/floppy-1.4/144m/x86DSK05.img vendor/freedos/floppy-1.4/144m/x86DSK06.img --title FreeDOS --tag unemu/media/ibm-pc/freedos:1.4-r1 --label '1=Disk 1 of 7 - boot disk and installer (x86BOOT.img)' --format 1=1.44M --label '2=Disk 2 of 7 - packages (x86DSK01.img)' --format 2=1.44M --label '3=Disk 3 of 7 - packages (x86DSK02.img)' --format 3=1.44M --label '4=Disk 4 of 7 - packages (x86DSK03.img)' --format 4=1.44M --label '5=Disk 5 of 7 - packages (x86DSK04.img)' --format 5=1.44M --label '6=Disk 6 of 7 - packages (x86DSK05.img)' --format 6=1.44M --label '7=Disk 7 of 7 - packages (x86DSK06.img)' --format 7=1.44M --extra vendor/freedos/floppy-1.4/LICENSES.TXT:LICENSES.TXT --extra vendor/freedos/floppy-1.4/freedos-1.4-floppy-src.tar:FD14SRC.TAR --publisher 'The FreeDOS Project' --developer 'The FreeDOS Project' --year 2025 --genre 'operating system' --platform DOS --description 'The official FreeDOS 1.4 Floppy Edition on seven 3.5-inch 1.44M diskettes, unmodified: the boot disk with the floppy installer, then six disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.' --version 1.4 --edition 'Floppy Edition' --set-label freedos --set-label open-source --set-label operating-system --open --license 'GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND LicenseRef-FreeDOS-other' --license-text LICENSES.TXT --source freedos-1.4-floppy-src.tar --store /path/to/store --create
cargo run --offline -p upcli -- image media import vendor/freedos/floppy-1.4/120m/x86BOOT.img vendor/freedos/floppy-1.4/120m/x86DSK01.img vendor/freedos/floppy-1.4/120m/x86DSK02.img vendor/freedos/floppy-1.4/120m/x86DSK03.img vendor/freedos/floppy-1.4/120m/x86DSK04.img vendor/freedos/floppy-1.4/120m/x86DSK05.img vendor/freedos/floppy-1.4/120m/x86DSK06.img vendor/freedos/floppy-1.4/120m/x86DSK07.img --title FreeDOS --tag unemu/media/ibm-pc/freedos:1.4-1.2m-r1 --label '1=Disk 1 of 8 - boot disk and installer (x86BOOT.img)' --format 1=1.2M --label '2=Disk 2 of 8 - packages (x86DSK01.img)' --format 2=1.2M --label '3=Disk 3 of 8 - packages (x86DSK02.img)' --format 3=1.2M --label '4=Disk 4 of 8 - packages (x86DSK03.img)' --format 4=1.2M --label '5=Disk 5 of 8 - packages (x86DSK04.img)' --format 5=1.2M --label '6=Disk 6 of 8 - packages (x86DSK05.img)' --format 6=1.2M --label '7=Disk 7 of 8 - packages (x86DSK06.img)' --format 7=1.2M --label '8=Disk 8 of 8 - packages (x86DSK07.img)' --format 8=1.2M --extra vendor/freedos/floppy-1.4/LICENSES.TXT:LICENSES.TXT --extra vendor/freedos/floppy-1.4/freedos-1.4-floppy-src.tar:FD14SRC.TAR --publisher 'The FreeDOS Project' --developer 'The FreeDOS Project' --year 2025 --genre 'operating system' --platform DOS --description 'The official FreeDOS 1.4 Floppy Edition on eight 5.25-inch 1.2M diskettes, unmodified: the boot disk with the floppy installer, then seven disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.' --version 1.4 --edition 'Floppy Edition' --set-label freedos --set-label open-source --set-label operating-system --open --license 'GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND LicenseRef-FreeDOS-other' --license-text LICENSES.TXT --source freedos-1.4-floppy-src.tar --store /path/to/store --create
cargo run --offline -p upcli -- image media import vendor/freedos/floppy-1.4/720k/x86BOOT.img vendor/freedos/floppy-1.4/720k/x86DSK01.img vendor/freedos/floppy-1.4/720k/x86DSK02.img vendor/freedos/floppy-1.4/720k/x86DSK03.img vendor/freedos/floppy-1.4/720k/x86DSK04.img vendor/freedos/floppy-1.4/720k/x86DSK05.img vendor/freedos/floppy-1.4/720k/x86DSK06.img vendor/freedos/floppy-1.4/720k/x86DSK07.img vendor/freedos/floppy-1.4/720k/x86DSK08.img vendor/freedos/floppy-1.4/720k/x86DSK09.img vendor/freedos/floppy-1.4/720k/x86DSK10.img vendor/freedos/floppy-1.4/720k/x86DSK11.img --title FreeDOS --tag unemu/media/ibm-pc/freedos:1.4-720k-r1 --label '1=Disk 1 of 12 - boot disk and installer (x86BOOT.img)' --format 1=720K --label '2=Disk 2 of 12 - packages (x86DSK01.img)' --format 2=720K --label '3=Disk 3 of 12 - packages (x86DSK02.img)' --format 3=720K --label '4=Disk 4 of 12 - packages (x86DSK03.img)' --format 4=720K --label '5=Disk 5 of 12 - packages (x86DSK04.img)' --format 5=720K --label '6=Disk 6 of 12 - packages (x86DSK05.img)' --format 6=720K --label '7=Disk 7 of 12 - packages (x86DSK06.img)' --format 7=720K --label '8=Disk 8 of 12 - packages (x86DSK07.img)' --format 8=720K --label '9=Disk 9 of 12 - packages (x86DSK08.img)' --format 9=720K --label '10=Disk 10 of 12 - packages (x86DSK09.img)' --format 10=720K --label '11=Disk 11 of 12 - packages (x86DSK10.img)' --format 11=720K --label '12=Disk 12 of 12 - packages (x86DSK11.img)' --format 12=720K --extra vendor/freedos/floppy-1.4/LICENSES.TXT:LICENSES.TXT --extra vendor/freedos/floppy-1.4/freedos-1.4-floppy-src.tar:FD14SRC.TAR --publisher 'The FreeDOS Project' --developer 'The FreeDOS Project' --year 2025 --genre 'operating system' --platform DOS --description 'The official FreeDOS 1.4 Floppy Edition on twelve 3.5-inch 720K diskettes, unmodified: the boot disk with the floppy installer, then eleven disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.' --version 1.4 --edition 'Floppy Edition' --set-label freedos --set-label open-source --set-label operating-system --open --license 'GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND LicenseRef-FreeDOS-other' --license-text LICENSES.TXT --source freedos-1.4-floppy-src.tar --store /path/to/store --create
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
cargo run --offline -p upcli -- image firmware import /path/to/roms --store /path/to/store --create --dry-run
cargo run --offline -p upcli -- image firmware import /path/to/even.bin /path/to/odd.bin --store /path/to/store
cargo run --offline -p upcli -- image firmware import /path/to/roms --rescan --for ibm-5170-bios-1985-06-10 --extra /path/to/manual.pdf --store /path/to/store
cargo run --offline -p upcli -- image firmware import /path/to/roms --for kiselev-xi8088-1.0.2-atreset --source vendor/kiselev-bios/kiselev-bios-a4af458-atreset-src.tar --label open-source --label xi8088 --store /path/to/store
cargo run --offline -p upcli -- image firmware import /path/to/rom.bin --unlisted my-option-rom --role option-rom --open --license MIT --built-from "example/option-rom" --revision r7 --store /path/to/store
cargo run --offline -p upcli -- image firmware edit private/firmware/ibm-pc/kiselev-xi8088-1.0.2:r1 --source vendor/kiselev-bios/kiselev-bios-a4af458-atreset-src.tar --store /path/to/store
cargo run --offline -p upcli -- image firmware ls --store /path/to/store
cargo run --offline -p upcli -- image firmware show private/firmware/ibm-pc/ibm-5170-bios-1985-06-10:r1 --store /path/to/store
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

A change that adds a feature a reader reads (an optional config key, a
vocabulary value, a payload media type, a firmware catalogue entry, a wider
accepted range) adds its row in the same change, with the version
`image_store::compat::NEXT_RELEASE` names (0.0.3 now) written as a literal,
`AppVersion::new(0, 0, 3)`, and `NEWEST_ROW` raised to it. A writer refuses
to stamp a value above the build's level, which points at a missing
`NEWEST_ROW` update. Vocabulary enums are matched exhaustively, so a new value
does not compile without a row; the completeness tests refuse a key, payload
type or catalogue entry without one:

```sh
cargo test --offline -p image-store compat
cargo test --offline -p pc-image compat
```

## Registry push and pull

```sh
cargo run --offline -p upcli -- image push dos-lab:1.0.0 --store /path/to/store --dry-run
cargo run --offline -p upcli -- image push dos-lab:1.0.0 private/dos-lab:1.0.0 --store /path/to/store
cargo run --offline -p upcli -- image push dos-lab:latest private/dos-lab:latest --store /path/to/store --move
cargo run --offline -p upcli -- image pull private/dos-lab:1.0.0 --store /path/to/store
cargo run --offline -p upcli -- image pull nas.example:5000/private/games:1 --store /path/to/new-store --create --tag games:1
cargo run --offline -p upcli -- image push private/firmware/ibm-pc/kiselev-xi8088-1.0.2:r1 nas.example:5000/private/firmware/ibm-pc/kiselev-xi8088-1.0.2:r1 --store /path/to/store --dry-run
cargo run --offline -p upcli -- image pull nas.example:5000/private/firmware/ibm-pc/kiselev-xi8088-1.0.2:r1 --store /path/to/store
cargo run --offline -p upcli -- image pull unemu/media/ibm-pc/freedos:1.4 --store /path/to/store
cargo run --offline -p upcli -- image remote ls private/dos-lab
cargo run --offline -p upcli -- image remote ls registry.unemu.dev
cargo run --offline -p upcli -- image login registry.unemu.dev --user NAME
printf '%s\n' "$TOKEN" | cargo run --offline -p upcli -- image login nas.example:5000 --user NAME
cargo run --offline -p upcli -- image registry set registry.unemu.dev --namespace unemu --namespace private:private
cargo run --offline -p upcli -- image registry set nas.example:5000 --private
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

Registries are listed in the shared preferences file (`unemupc/preferences.toml`
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
bash vendor/unemu-at-bios/fetch.sh
bash vendor/vgabios/fetch.sh
bash vendor/xtide-universal-bios/fetch.sh
cargo run --offline -p upcli -- image template build --store /path/to/templates --dry-run
cargo run --offline -p upcli -- image template build --store /path/to/templates
```

`image template build` builds the two template machines and nothing else. It
is a command rather than a script around `image firmware import` and New
machine…: the template machines come from the create API (`pc_image::create`),
which has no command of its own, and the push guard must judge each manifest
before its tags are written. It reads the open firmware fixtures under
`vendor/` in the working directory (`--vendor DIR` for another copy) and
writes into `--store` (created when missing), in this order (design:
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
upcli image pull unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1 --store /path/to/store
upcli image pull unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1 --store /path/to/store
upcli image pull unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1 --store /path/to/store
upcli image pull unemu/media/ibm-pc/freedos:1.4 --store /path/to/store
upcli image pull unemu/media/ibm-pc/freedos:1.4-1.2m --store /path/to/store
upcli image pull unemu/media/ibm-pc/freedos:1.4-720k --store /path/to/store
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
cargo run --offline -p upcli -- image push open-at:1.2.0 unemu/open-at:1.2.0 --store "$S" --dry-run
cargo run --offline -p upcli -- image push open-at-games:1.2.0 unemu/open-at-games:1.2.0 --store "$S" --dry-run
cargo run --offline -p upcli -- image push open-at:latest unemu/open-at:latest --store "$S" --dry-run
cargo run --offline -p upcli -- image push open-at-games:latest unemu/open-at-games:latest --store "$S" --dry-run

cargo run --offline -p upcli -- image push open-at:1.2.0 unemu/open-at:1.2.0 --store "$S"
cargo run --offline -p upcli -- image push open-at-games:1.2.0 unemu/open-at-games:1.2.0 --store "$S"
cargo run --offline -p upcli -- image push open-at:latest unemu/open-at:latest --store "$S"
cargo run --offline -p upcli -- image push open-at-games:latest unemu/open-at-games:latest --store "$S"
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
cargo run --offline -p upcli -- image media import vendor/shareware/keen4/keen4-sw14-1440.img --title "Commander Keen 4: Secret of the Oracle (shareware 1.4)" --tag private/media/ibm-pc/keen4-shareware:1.4-r1 --label '1=Disk 1 of 1 - Shareware v1.4 installation' --extra vendor/shareware/keen4/extras/README.DOC:README.DOC --extra vendor/shareware/keen4/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1991 --genre platformer --platform DOS --description 'Commander Keen 4: Secret of the Oracle, version 1.4, the shareware episode of a side-scrolling platform game in EGA graphics by id Software, distributed by Apogee Software. Keen searches the Shadowlands of Gnosticus IV for the eight missing Council Members. The disk holds the files of the original Apogee shareware archive 4keen.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-readme-keen4-v1.4 --license-text README.DOC --store /path/to/store --create
cargo run --offline -p upcli -- image media import vendor/shareware/wolf3d/wolf3d-sw14-1440.img --title "Wolfenstein 3-D: Episode 1 (shareware 1.4)" --tag private/media/ibm-pc/wolf3d-shareware:1.4-r1 --label '1=Disk 1 of 1 - Shareware v1.4 installation' --extra vendor/shareware/wolf3d/extras/VENDOR.DOC:VENDOR.DOC --extra vendor/shareware/wolf3d/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "id Software" --year 1992 --genre "first-person shooter" --platform DOS --description 'Wolfenstein 3-D, episode 1, Escape from Wolfenstein, version 1.4, the shareware episode of a first-person shooter with texture-mapped VGA graphics by id Software, published by Apogee Software. As B.J. Blazkowicz the player escapes from Castle Wolfenstein through ten floors, one of them secret. It needs a 286 or better. The disk holds the files of the original Apogee shareware archive 1wolf14.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-vendor-v11.01.92 --license-text VENDOR.DOC --store /path/to/store
cargo run --offline -p upcli -- image media import vendor/shareware/duke2/duke2-sw10-1440.img --title "Duke Nukem II: Episode 1 (shareware 1.0)" --tag private/media/ibm-pc/duke2-shareware:1.0-r1 --label '1=Disk 1 of 1 - Shareware v1.0 installation' --extra vendor/shareware/duke2/extras/LICENSE.DOC:LICENSE.DOC --extra vendor/shareware/duke2/extras/ORDER.FRM:ORDER.FRM --publisher "Apogee Software" --developer "Apogee Software" --year 1993 --genre platformer --platform DOS --description 'Duke Nukem II, episode 1, version 1.0, the shareware episode of a side-scrolling platform game by Todd Replogle and Apogee Software, in which Duke fights the alien Rigelatins. It needs VGA graphics and a 286 or better. The disk holds the files of the original Apogee shareware archive 4duke.zip, unmodified; INSTALL.BAT installs the game onto a chosen drive.' --open --license LicenseRef-apogee-license-v12.01.93 --license-text LICENSE.DOC --store /path/to/store
cargo run --offline -p upcli -- image push private/media/ibm-pc/wolf3d-shareware:1.4-r1 unemu/media/ibm-pc/wolf3d-shareware:1.4-r1 --store /path/to/store --dry-run   # guard verdict for the official path
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

```sh
cargo build                                  # build the workspace
cargo run -p upcli -- --boot                   # Real16 timer demo; no args print usage
cargo run --offline -p upcli -- --bios vendor/kiselev-bios/bios-xi8088.bin
cargo run --offline -p upcli -- --bios /path/to/bios.bin --allow-unlisted-firmware
cargo run --offline -p upcli -- machine check machines/at-freedos.toml
cargo run --offline -p upcli -- run --machine machines/at-freedos.toml --max-steps 12000000 --idle-limit 1000000 --print mda
cargo run --offline -p window --bin unemupc -- --machine machines/at-freedos.toml --display mda
cargo test --workspace                       # unit + golden + integration
cargo test --release -p devices              # release-only device behaviour (CI runs it too)
cargo test --workspace -- --ignored          # slow goldens: BIOS POST, FreeDOS boot, DOS sessions
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --workspace -- --include-ignored # CI goldens lane: every fixture test and the goldens; fail if a fixture is missing
# Corrected per-clock oracle versus production deadlines (no environment gate):
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p pc-harness --test scheduler_ab -- --include-ignored --nocapture
# Opt-in host timing only, BIOS POST and FreeDOS prompt, median of three per engine:
UNEMUPC_REQUIRE_FIXTURES=1 UNEMUPC_SCHEDULER_WALL_TIMES=1 cargo test --offline --release -p pc-harness --test scheduler_ab scheduler_engine_wall_times -- --ignored --nocapture
# DOS sessions and their final console screens (requires fetched fixtures):
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p pc-harness --test dos_interactive_golden -- --ignored --nocapture
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p pc-harness --test dos_write_golden -- --ignored --nocapture
# FreeDOS lists and types a B: floppy built by media create --add, then enters a
# two-disk --split set with CD and rebuilds it with XCOPY:
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p cli-pc --test media_files_golden -- --ignored --nocapture
# FreeDOS boots A: with the open AT BIOS and XT+ ROM, then lists readonly C:
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p pc-harness --test ide_freedos_golden -- --ignored --nocapture
# Open AT protected-mode test floppy: INT 15h 88h/87h, PM entry, 8042/triple-fault resets, A20 (design/at-pm-test.md)
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p pc-harness --test at_pm_test_golden -- --include-ignored --nocapture
# Open AT LOADALL test floppy: real-mode moves above 1 MiB through LOADALL caches (design/at-loadall-test.md)
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p pc-harness --test at_loadall_test_golden -- --include-ignored --nocapture
# Open AT privilege-level test floppy: IRET to ring 3, INT, #GP and a call gate back to ring 0 on SS0:SP0 (design/at-privilege-test.md)
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p pc-harness --test at_privilege_test_golden -- --include-ignored --nocapture
cargo test -p devices                        # one crate's tests
cargo build --offline -p window --bin unemupc
cargo run --offline -p window --bin unemupc # lists machines/, selects at-freedos.toml
cargo run --offline -p window --bin unemupc -- --machine machines/local.toml --allow-unlisted-firmware
cargo run --offline -p window --bin unemupc -- --open FILE [--core ID] # a file, run by whichever compiled-in core opens it
# Trying File › Open… and --open by hand before a core that opens files is in
# the release: the scripted test core (it emulates nothing), never in a release.
cargo run --offline -p window --bin unemupc --features platforms/test-core -- --open machine.utc
cargo tree --offline -i window               # window is a leaf frontend
```

The emulation goldens (BIOS POST, FreeDOS boots and sessions, floppy, IDE,
VGA, EMS, the sound cards, the scheduler A/B tests) live in
`crates/pc-harness/tests/`, beside the machine runners they drive, with the
test support other crates include by `#[path]` in
`crates/pc-harness/tests/support/`: run them with `-p pc-harness`. The
goldens that need the command line or the image store (`at_games`,
`boot_sector_golden`, `freedos_install_golden`, `ide_journal_golden`,
`machine_freedos_golden`, `media_files_golden`) and the PC command tests
live in `crates/cli-pc/tests/`: run them with `-p cli-pc`. The tests that
run the `upcli` binary (`firmware_catalogue`, `timing_adoption`,
`machine_commands`, `image_registry_commands`) and the pin of the whole
command line (`command_line_pin`) live in `crates/upcli/tests/` (`-p
upcli`), and the journal tools' tests (`media_tools`, `media_fifo`) with the
neutral command line in `crates/cli-core/tests/` (`-p cli-core`).

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
design/observation.md §CPU stop report.
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
described in design/observation.md §Exception and interrupt deliveries.

In the window, Tools › Diagnostics holds three items:

- **Save machine report** (Ctrl+Alt+Shift+D, ⌘⇧D on macOS, with the pointer
  captured or not) writes the running PC's report to `diag-N.txt` in
  `unemupc/diagnostics` of the platform data directory (on macOS
  `~/Library/Application Support/unemupc/diagnostics`), N counting on from
  the highest number already there, and the status line shows `Machine
  report saved to PATH`. It works whenever a PC runs, paused too, and is
  disabled without one. The machine keeps its code trail from every Start;
  the report's exception deliveries start with the first report (which
  says so and lists none) or with Record exceptions, because the CPU runs
  6 to 10 per cent slower on interrupt-heavy guests while it logs them.
  The chord's D never reaches the guest. The
  report is described in design/observation.md §Diagnostic report of a
  running machine.
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
"unemupc is running <machine>. Stop it and continue?" with Stop machine and
Cancel (Esc; Enter and Space do not answer). Cancel resumes the machine where
it paused, without catch-up, and hands back guest keyboard focus and pointer
capture if the guest had them; an explicit pause stays in effect. Exit, ⌘Q,
⌘W and the close button also work while a menu or another dialog is open:
they close or cancel it first, keeping only this question or the Gaming-mode
keep-or-discard question. One Escape closes only the top-most dialog. A
stopped, halted or failed machine is not asked about, and Machine > Stop and
Power cycle act at once. A Gaming-mode session asks Keep as new image or
Discard after stopping instead. On macOS the global menu bar holds only the
application menu: About unemupc, Hide unemupc (⌘H), Hide Others (⌥⌘H),
Show All and Quit unemupc (⌘Q). Quit and ⌘Q reach this question like Exit.
Dock › Quit, logout, restart and shutdown wait for it as well: with no machine
running they quit at once after persistence; Stop machine quits once the stop
and persistence have finished; Cancel keeps the application running (logout
reports that unemupc cancelled it). A stop that fails to finish its journals
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
the tree's `machines/` (cargo names the crate's directory in
`CARGO_MANIFEST_DIR` at run time, so nothing of the tree is compiled in); else
`unemupc/machines` in the platform data directory (`~/Library/Application
Support` on macOS, `$XDG_DATA_HOME` or `~/.local/share` on Linux,
`%LOCALAPPDATA%` on Windows). `--help` prints the directory in use. The
desktop's ROM folders are its preferences, `--rom-dir`, `UNEMUPC_USER_ROMS_DIR`
and, under cargo, the tree's `vendor/user-roms`. A machine file outside any
`machines` directory keeps its Settings… child in the `local/` folder of the
library's machines directory; with no data directory and no
`UNEMUPC_MACHINES_DIR`, Settings refuses with a message instead of writing into
the working directory.
`--machine FILE` is relative to the current directory; image and inheritance
paths inside it are relative to their defining file. A machine named with
`--machine` that does not exist is listed and selected, so its error shows.
`--display CARD` chooses a display by id; the default is the first display in
file order (MDA in the shipped machine). Positional BIOS/floppy paths are removed;
use an extending file as shown in `machines/README.md`. From outside the
workspace, Cargo also needs `--manifest-path /path/to/workspace/Cargo.toml`.
`--help`, `--version` and `--about` include the embedded fonts' attribution and
the Nuked-OPL3 notice; `--version` and `--about` also print one line per
compiled-in core, `core unemupc 0.0.2 (unemupc), contract 0.2` and
`core unemunes 0.0.1 (unemunes), contract 0.2` in the release (a core
built for another contract ends its line with `, unusable: ` and the reason),
then the licence, the source URL and where the full licence texts are.
`upcli --version` (or `--about`) prints the same for the command line, the core
lines after its first two lines.
`--open FILE [--core ID]` starts a machine from a file as File › Open… does:
the file is hashed whole on a worker thread, the cores that list its
extension are asked first, then the others (only `ID` with `--core`), and a
file no core runs, or that several run, is refused with a message box.
`--core` without `--open`, and `--open` with `--machine`, are usage errors.
What such a machine leaves behind goes under `unemupc/saves/CORE/aa/DIGEST/`
in the platform data directory (and saved states, later, under
`unemupc/states/`), keyed by the content's SHA-256, not its name or place.
The release's cores are the PC, which opens no files, and the NES core of
unemunes, which opens NROM (mapper 0) cartridge images: File › Open…'s dialog
offers "NES cartridge images (*.nes)", and `--open game.nes` starts one, say
`cargo run --offline -p window --bin unemupc -- --open /path/to/game.nes`. A build whose
cores open nothing greys File › Open… ("No core in this release opens
files"). `--features platforms/test-core` adds the scripted test core, whose
`.utc` scripts it runs (`crates/test-core/src/lib.rs` has the grammar, and
`unemu-test-core 1` on the first line is a whole script). With that feature
the `--about` pins of both programs fail by the test core's line.
On Linux the window requires the host's X11 or Wayland client libraries at run
time; a bare Nix devShell may not put them on the dynamic loader's search path.

The ignored set includes firmware/DOS goldens and corrected PerClock-vs-Deadline
comparisons, including final RAM/register/device state. The per-clock engine is
available through the non-default `reference-scheduler` feature enabled by CLI
dev-dependencies, never through `MachineConfig`. Legacy comparisons and
the scheduler A/B switch are retired. Step budgets include REP iterations; the
command budget is calibrated to the final DOS file-write listing. No arguments
print usage to stdout with status 0; unknown or extra arguments print usage to
stderr with status 2. The informative
`scheduler_engine_wall_times` test skips with one line unless
`UNEMUPC_SCHEDULER_WALL_TIMES=1` is set, including in ordinary slow/CI runs.

## Affected tests and the bead gate

Run only what a change can affect (in the devShell, from any directory):

```sh
bash scripts/affected-tests.sh --plan        # what would run, and why; builds nothing
bash scripts/affected-tests.sh               # changes since the merge base with main, working tree included
bash scripts/affected-tests.sh --base origin/main --tier goldens
bash scripts/affected-tests.sh --all --tier all   # everything, every tier: before a release
bash scripts/affected-tests.sh --self-test   # the planner on its fixtures, the rules against the sources
AFFECTED_RUNNER=cargo bash scripts/affected-tests.sh   # cargo test instead of cargo-nextest
bash scripts/bead-gate.sh                    # fmt, guard, clippy, tests, rustdoc, deny: only what the change affects
AFFECTED_CARGO='nice -n 10 cargo' bash scripts/bead-gate.sh --log-dir "${TMPDIR:-/tmp}/gate-logs"
```

The runner takes the changed files (the merge base of `--base`, default
`main`, against the working tree, plus untracked files) and maps each to the
workspace crate whose directory holds it. Untracked files count: a stray
file in the tree widens the plan, and one that no rule knows selects
everything, so keep logs and scratch files outside the repository. Files
outside the crates map to the crates that read them: shipped machine files,
fixture scripts and pins under `vendor/`, the test support that several
crates include with `#[path]` from `crates/pc-harness/tests/support/`, the
repository's `assets/`, the approved mock-ups and the recipe corpus. Every crate that depends
on a changed crate (normal, build or dev dependency, from `cargo metadata
--no-deps`) is affected too. The root `Cargo.toml`, `Cargo.lock`, the flake
and toolchain files, a `[patch]` section in `.cargo/config.toml` (the local
CPU-core override of [`dev-setup.md`](dev-setup.md), which means the tests do
not run against the pinned core) and any path no rule knows select the whole
workspace with the goldens. Every run starts with the checks that need no
build: the publication guard (unless `--no-guard`), `git diff --check` of the
changed documentation (untracked files included), `bash -n` of every changed
shell script (`scripts/`, `vendor/*/fetch.sh` and `build.sh`) and, when the
runner or its fixtures changed, its self-test. Documentation, script,
repository and tracker configuration (`.github/`, `.beads/`) changes run only
these. A Rust change anywhere under `crates/` also runs image-store's
`platform_ids`, which reads every crate's sources, and an affected `devices`
also runs its release-profile tests. The plan lists every file with the rule
that mapped it.

Tiers select among the tests of the affected crates by libtest's ignore flag
and the `user_rom_` name prefix; no test attribute changes for them:

| Tier | Runs | Chosen |
| --- | --- | --- |
| fast | the affected crates' tests that are not ignored, plus some ignored ones by name (below) | by default |
| goldens | plus every ignored test except `user_rom_*` | automatically when an emulation crate (`assembly`, `audio`, `bus`, `chipset`, `cpu`, `device`, `devices`, `machine`, `machine-file`, `media`, `testkit`, `video`), the machine runners (`crates/pc-harness/src/`), the command line the front-end goldens drive (`crates/cli-core/src/`, the neutral command line whose journal tools a golden runs, and `crates/cli-pc/src/`, every PC command), the disk builder (`crates/pc-image/src/media_create.rs`), a shipped machine file, a fixture under `vendor/`, the shared golden test support, the workspace manifest, the lock file, the toolchain, a `[patch]` override or a path no rule knows changed; or `--tier goldens` |
| user-roms | auto plus the ignored `user_rom_*` tests | `--tier user-roms`, with `UNEMUPC_USER_ROMS_DIR` set |
| all | goldens and user-ROM tests | `--tier all`; before a release with `--all` |

In the fast tier the runner still runs some ignored tests, by name and in
the same build: every ignored test of a crate that changed directly (a
window change runs the window's three boots), and those in files of an
affected crate that name a changed crate as `crate_name::` (an `image-store`
or `pc-image` change runs `machine_freedos_golden`). `--tier fast` turns
that off too. A crate in the affected set only through its dependencies does
not select the goldens: an `image-store` change runs the fast tests of
`image-store`, `image-registry`, `machine-file`, `pc-image`, `pc-harness`,
`platforms`, `cli-core`, `cli-pc`, `upcli` and `window`, and the ignored
tests that name `image_store::`. A change to a test of the PC command line
(`crates/cli-pc/tests/image_commands.rs`, say) runs the tests of `cli-pc`
and `upcli` and, by name, the seven ignored tests of the front-end goldens;
a change to the program alone (`crates/upcli/src/about.rs`) runs `upcli`'s
tests only; the emulation goldens in `pc-harness` are not rebuilt. The goldens need
the fetched fixtures; the bead gate sets `UNEMUPC_REQUIRE_FIXTURES=1` so a
missing one fails instead of skipping.

The tests run under cargo-nextest when the devShell provides it, otherwise
under `cargo test`; the runner says which. Nextest runs each test in its own
process and several test binaries at once, which cargo runs one after
another; it selects the same tests (`--run-ignored` and a filter expression
on `user_rom_` and the names), and doctests, which nextest does not run, go
through `cargo test --doc`. Both runners run at most `AFFECTED_TEST_THREADS`
tests at once, default `RUST_TEST_THREADS`, else 4, never unbounded.
`AFFECTED_RUNNER=cargo` forces `cargo test`. Each failed test is listed by
name in the summary.

`AFFECTED_CARGO` is the command that runs cargo (default `cargo`), so a wrapper
that bounds parallelism on a shared machine goes there; `AFFECTED_LOG_DIR`
keeps the runner's logs. The runner prints each step's time and the slowest
tests (nextest) or test binaries (`cargo test`, from libtest's `finished in`).
`--files FILE` plans from a list of paths instead of git, and `--metadata
FILE` from saved `cargo metadata` output; the self-test uses both with
`scripts/testdata/affected-tests/` (a snapshot of the workspace graph and of
the sources' ignored tests, the scenarios and their expected plans).

`scripts/bead-gate.sh` gates one change: `cargo fmt --check`, the publication
guard and the core host-interface self-test and check, clippy with `-D warnings` on the affected crates (`--all-targets
--all-features`, plus the production `upcli` binary when `upcli` is affected),
the affected tests in their tier, rustdoc with `-D warnings` for the
affected crates, and `cargo deny` only when `Cargo.lock`, a manifest or
`deny.toml` changed. Every step runs after a failure; the summary gives each
step's result and seconds. It takes the runner's `--base`, `--tier`,
`--all` and `--files` and `--log-dir DIR`.

CI (`.github/workflows/ci.yml`) still runs every lane on the whole workspace
for pushes to `main` and pull requests, and the release checklist
([Releases](#releases)) runs `--all --tier all`. Measured build and test times, and what keeps a change
from gating within five minutes, are in [`test-timings.md`](test-timings.md).
When the rules miss a path a crate reads, add it to the planner's
`data_rules` in `scripts/affected-tests.sh`; the self-test fails until a
path a crate source names outside its directory is mapped to that crate, and
until every crate whose sources build paths from the repository root reads
both `machines/` and `vendor/`.

## Checking recipes

`upcli recipe check` runs the static checks of a recipe
([design/recipes.md](design/recipes.md) §6.11): it reads the recipe, the
work file it names and, for a pinned recipe, its lock, and no input.

```sh
cargo run --offline -p upcli -- recipe check backlog/docs/design/recipe-corpus
cargo run --offline -p upcli -- recipe check my.media.toml other.firmware.toml
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

The JSON Schema `design/recipe-schema-0.1.json` is written from the
parser's key tables and the providers; a test in `pc-image` compares it.
After a change to the format or a provider vocabulary, regenerate it:

```sh
UNEMUPC_WRITE_RECIPE_SCHEMA=1 cargo test --offline -p pc-image --test recipe_check
```

`upcli recipe show FILE` prints a media recipe's title record merged with
the work file it names, by the rules of design/recipes.md §6.16, as the
build will write the set's title metadata:

```sh
cargo run --offline -p upcli -- recipe show backlog/docs/design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml
```

The first line is `NAME:TAG`, the digest or `draft`, and the platform; then
one line per value, a field name, a tab and the value (text quoted), with
the names and order of `upcli image media show`'s catalogue lines
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

`upcli recipe plan` resolves every input of a recipe without building
anything ([design/recipes.md](design/recipes.md) §3.4, §3.5): for each
input, where it was found or why not; the steps that would run; the outputs
that can be built. Libraries are folders where the user keeps files; the
plan finds an input in them by size and SHA-256, whatever the file is
called, and looks inside zip archives.

```sh
cargo run --offline -p upcli -- library add /path/to/games
cargo run --offline -p upcli -- library scan
cargo run --offline -p upcli -- library ls
cargo run --offline -p upcli -- recipe plan my.media.toml
cargo run --offline -p upcli -- recipe plan my.media.toml --json --library /path/to/more --store /path/to/store
cargo run --offline -p upcli -- library remove /path/to/games
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

`upcli recipe build` builds a recipe into the store
([design/recipes.md](design/recipes.md) §3.1, §6.9, §6.12, §6.14): it plans
as `recipe plan` does, reads every value it found and checks it against its
size and SHA-256 again, runs the steps (`unpack`, `text`, `fat`) and hands
the outputs to the importers, the same plan-and-publish functions `image
media import` and `image firmware import` end in. The importer copies and
hashes every file and tags the artifact last.

```sh
cargo run --offline -p upcli -- recipe build my.media.toml --library /path/to/games
cargo run --offline -p upcli -- recipe build my.media.toml --fetch --store /path/to/store
cargo run --offline -p upcli -- recipe build my.media.toml --keep-as 1.0-mine-r1
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
  downloaded. With it, every URL of an input the plan reports missing is
  tried in order, and only when every missing input has a URL; the list of
  what will be downloaded is printed first (`fetch: downloading N inputs
  (BYTES bytes), as --fetch allows:`, then one line per input with its
  size, SHA-256 and URLs). When a missing input has no URL, nothing is
  downloaded and the build says `fetch: nothing downloaded: input ID has no
  URL`. Only `https` is fetched, redirects only to `https`, at most the
  pinned size; connecting may take 30 s, each read 60 s, and a whole
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

`upcli recipe pin FILE` is the author's build, once per revision
([design/recipes.md](design/recipes.md) §3.1, §3.8, §7.2): it builds the
draft as `recipe build` does (same options, without `--keep-as`), tags it,
then writes the lock beside the recipe and the manifest digest into it.

```sh
cargo run --offline -p upcli -- recipe pin my.media.toml --library /path/to/games
cargo run --offline -p upcli -- recipe pin backlog/docs/design/recipe-corpus/vgabios/vgabios-0.9d-unemu.1-std.firmware.toml --library vendor
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

## Recipe conformance

`upcli recipe conformance FILE|FOLDER... [--library DIR]... [--store DIR]
[--config FILE]` builds every pinned recipe of the files and folders named
into a scratch store, from the libraries only (the preferences' first, then
each `--library`; no download, no other store), and compares the digest it
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
# this repository: the pinned open recipes of the corpus, from the fixtures
cargo run --offline -p upcli -- recipe conformance backlog/docs/design/recipe-corpus --library vendor/freedos --library vendor
# a repository of recipes, against a release of upcli and its own library
upcli recipe conformance recipes/ --library /path/to/library --store ./conformance-store
```

One line per recipe: `FILE: conforms: NAME:TAG<TAB>DIGEST`, `FILE: differs:
…` (the mismatches), `FILE: not built: …` (an input missing, or why it
failed) or `FILE: draft, not built`; then `N pinned recipes: C conform, D
differ, M not built; K drafts skipped`. Exit status: 0 when every pinned
recipe conforms, 1 when one differs or was not built or when no pinned
recipe was found (`upcli recipe conformance: no pinned recipe found …` on
standard error: a run that finds nothing proves nothing), 2 on a usage error
or a store or file that cannot be opened. The test
`the_pinned_corpus_builds_the_digests_it_pins` (`pc-image`, `recipe_lock`)
runs the same harness on the corpus's six pinned open recipes, the three
FreeDOS 1.4 floppy sets and the three open firmware packages.

## Scaffolding recipes and listing their inputs

`upcli recipe scaffold FILE|FOLDER...` prints the tables a recipe needs for
files the author has, measured instead of typed
([design/recipes.md](design/recipes.md) §6.8, §6.9, §6.15). It reads the
files and writes nothing; the author adds the identity, the description,
the outputs and whatever else the recipe states, and keeps or renames
what scaffold wrote. `upcli recipe inputs FILE --format
dat|csv|sha256sums` prints the inputs of a recipe that passes the checks,
for a ROM manager or `sha256sum -c`.

```sh
cargo run --offline -p upcli -- recipe scaffold LABFULL.ZIP AZURAMON
cargo run --offline -p upcli -- recipe inputs my.media.toml --format sha256sums
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
  author `upcli recipe inputs`) and one game, `<last component>-<tag>`,
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
user's own LABFULL.ZIP and AZURAMON folder (vendor/user-roms/README.md)
and compare with the corpus's tables.

## Recipes on a registry

A pinned recipe is published as a registry artifact without any payload
([design/recipes.md](design/recipes.md) §7, decision-35): `upcli recipe
push FILE REF` sends it, `upcli recipe pull REF` writes it into a folder
where it plans and builds with no other file, and `upcli recipe plan REF`
judges it from its manifest and config alone. The registries, credentials
and `--registry` are those of `image push` and `image pull` (Registry push
and pull).

```sh
upcli recipe push my.media.toml unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1 --dry-run
upcli recipe push my.media.toml unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1 --store /path/to/store
upcli recipe pull unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1 --to recipes/freedos
upcli recipe plan unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1 --library /path/to/games
upcli recipe build recipes/freedos/freedos-1.4-720k.media.toml --library /path/to/games
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

`upcli image export NAME:TAG --to DIR` writes an installation set or a
firmware package of the store as plain files ([design/recipes.md](design/recipes.md)
§3.10, decision-37),
so the originals can be deleted and the files had back without the store:

```sh
cargo run --offline -p upcli -- image export private/media/ibm-pc/azuramoon:2026-r1 --to azuramoon --files
cargo run --offline -p upcli -- image export private/firmware/ibm-pc/opt-rom:r1 --to opt-rom --store /path/to/store
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
  Image Layouts is other work (`upc-5pw.116`).

The same files without `upcli`: `scripts/export-layout.sh [--files] LAYOUT
REF DIR` reads any OCI Image Layout, such as the `images` folder of a store
or what `oras copy --to-oci-layout` saves, with a POSIX shell, `jq`,
`sha256sum` and `zstd`, and for `--files` mtools (`mdir`, `mcopy -s -m`
under `TZ=UTC0`). REF is the image's `org.opencontainers.image.ref.name` in
`index.json` (a store's `NAME:TAG`) or its manifest digest. It checks every
blob against its digest and every decoded layer against
`unemu.decoded.digest`, and checks the names as `upcli` does (no name the
export writes itself, no two layers' names equal ignoring case, no `files`
with `--files`) before it writes anything. It runs under a UTF-8 locale
(`C.UTF-8` or `en_US.UTF-8`; it stops without one), with mtools reading no
configuration (`MTOOLSRC=/dev/null`). Unlike `upcli` it writes only into an
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
cargo run --offline -p upcli -- media create floppy --type 1.44M --format --label TRANSFER /path/to/transfer.img
cargo run --offline -p upcli -- media create floppy --type 1.44M --add /path/to/GAME --label GAME /path/to/game.img
cargo run --offline -p upcli -- media create floppy --type 1.44M --add /path/to/PLAY.EXE --add /path/to/DATA.DAT --top MYGAME /path/to/mygame.img
cargo run --offline -p upcli -- media create floppy --type 1.44M --add /path/to/BIGGAME --label BIGGAME --split /path/to/big.img
cargo run --offline -p upcli -- media create hd --size 32MiB /path/to/disk.img
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
rules: `machines/README.md` § Building a floppy from host files.

## Offline persistent media

```sh
cargo run -p upcli -- media generations --base disk.img --journal disk.img.upfj
cargo run -p upcli -- media verify --base disk.img --journal disk.img.upfj
cargo run -p upcli -- media verify --base disk.img --journal disk.img.upfj --repair-tail --backup before-repair.upfj
cargo run -p upcli -- media rollback --base disk.img --journal disk.img.upfj --to 1 --backup before-rollback.upfj
cargo run -p upcli -- media commit --base disk.img --journal disk.img.upfj --to 1 --output restored.img
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
build output stay in a new temporary directory; `cargo build -p upcli` uses the
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
cargo build --offline -p upcli
(
  cd "$demo"
  "$repo_dir/target/debug/upcli" media generations --base disk.img --journal disk.upfj
  "$repo_dir/target/debug/upcli" media verify --base disk.img --journal disk.upfj
  "$repo_dir/target/debug/upcli" media rollback --base disk.img --journal disk.upfj --to 1 --backup before-rollback.upfj
  "$repo_dir/target/debug/upcli" media commit --base disk.img --journal disk.upfj --to 1 --output restored.img
  "$repo_dir/target/debug/upcli" media compact --base disk.img --journal disk.upfj --to 1 --output restored-alias.img
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
cargo clippy --locked -p upcli --bin upcli -- -D warnings # production, no reference-scheduler
cargo fmt --all
cargo fmt --all -- --check                   # CI check
cargo deny --all-features list               # inspect dependency licences
cargo deny --all-features check advisories licenses bans sources
```

The `deny.toml` allowlist covers the all-features graph: AGPL-3.0-or-later for
workspace crates, MIT, Apache-2.0, Unicode-3.0, BSD-2-Clause, BSD-3-Clause
and ISC for dependencies. CC-BY-SA-4.0 is allowed only for the `unemu-pc-core`
crate (a crate-scoped exception), whose text rasteriser embeds the font assets
that carry it; see `crates/pc-core/assets/fonts/NOTICE.md`. CC0-1.0 and Bitstream-Vera are
allowed only for the `ui-kit` crate, which embeds the brand logo and the
DejaVu Sans symbol subset (`assets/brand/README.md`,
`crates/ui-kit/assets/fonts/dejavu/NOTICE.md`). New licences
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

GitHub Actions runs `.github/workflows/ci.yml` on pushes to `main`, pull requests
and manual dispatches, on a GitHub-hosted x86-64 `ubuntu-latest` runner. Read the
runs with the GitHub CLI:

```sh
gh run list --repo unemu-org/unemupc --workflow ci.yml
gh run view <run-id> --repo unemu-org/unemupc --log-failed
gh run download <run-id> --repo unemu-org/unemupc   # ci-logs-attempt-<n>: ci-logs/<step>.log
```

From the repository root, run the same gates locally:

```sh
bash scripts/ci-gate.sh devshell
bash scripts/ci-gate.sh fmt
bash scripts/ci-gate.sh guard                    # publication, crate boundaries and core host interfaces (self-tests and checks)
bash scripts/ci-gate.sh clippy
bash scripts/ci-gate.sh deny                     # advisories, licences, bans, sources; all features
bash scripts/ci-gate.sh tests
bash scripts/ci-gate.sh fixtures                 # build/hash-check BIOSes, FreeDOS, test386, XTIDE, HDD, VGA BIOS, open AT BIOS, at-pm-test, at-loadall-test, at-privilege-test, source archives
bash scripts/ci-gate.sh goldens                  # slow tests in the test profile; requires fixtures
```

Fixture-dependent tests print a skip message when a fetched vendor file is missing.
Set `UNEMUPC_REQUIRE_FIXTURES=1` to make a missing fixture fail with its path and
fetch script instead. The `goldens` gate always sets this variable, so a failed or
incomplete fixture fetch cannot silently pass CI. The µPD765 FAT12 test follows
the same policy when `mformat` is missing and points to `nix develop`.

### Publication guard

`bash scripts/publication-guard.sh` (the `guard` gate and a pre-commit hook)
scans tracked and untracked, non-ignored text with plain bash, grep, sed and
awk. It fails on private filesystem paths (home directories, macOS volumes and
temporary directories, Windows user profiles, the root user's home and mounts
under mnt; the script lists the exact forms), e-mail addresses outside
`example.org`, `example.com`, `.example`
hosts and the maintainer's and agent trailer addresses (an SSH login such as
`git@github.com:org/repo` is not an address), RFC 1918 and carrier-grade NAT
IPv4 addresses outside the registry tests' `10.0.0.1` (CIDR ranges are not
addresses), `Cargo.lock` sources other than crates.io and github.com git
repositories, LGPL headers outside the OPL3 port and the project's former name.
Private hostnames never appear in the repository: list them, comma-separated,
in `PUBLICATION_GUARD_PRIVATE_HOSTS`, for example
`PUBLICATION_GUARD_PRIVATE_HOSTS=nas.example,lab.example bash scripts/publication-guard.sh`.
That check runs only locally, where someone sets the variable, never in CI, and
a hit prints only the file and line.

### Crate boundaries

`bash scripts/crate-boundaries.sh` (in the devShell: it needs cargo and jq)
reads `cargo metadata --no-deps` and fails when a workspace crate has no
`[package.metadata.unemu] layer`, or depends, through normal, build or test
dependencies and directly or through other workspace crates, on a layer its
own may not use ([core contract](design/core-contract.md) §7.3), or when a
window crate depends on a CLI crate or the other way round. It reads the
dependencies the manifests declare (optional and target-specific ones
included), follows a dependency's normal and build dependencies but not its
test dependencies, and reports the first forbidden crate on each path once:
the two crates, their layers, the dependency kind, the path and the rule.
Known violations are listed in `scripts/crate-boundaries.allow`, one declared
dependency `FROM → TO (KIND): REASON` per line; the walk never crosses a
listed dependency, and one that is gone or no longer forbidden fails, so the
list only shrinks.

The same check limits the crates from outside the workspace that the core
reaches ([core contract](design/core-contract.md) §12.1, §12.3).
`scripts/crate-boundaries.external` lists them for the `contract` and
`emulation` layers, one `LAYER: CRATE (run|build|test): REASON` per line; the
other layers are not limited. From each workspace package of those layers the
check walks the resolved graph (`cargo metadata --offline --locked
--all-features`, every target): the package's own dependencies of every kind,
then the normal and build dependencies of what it reaches, workspace crates of
any layer included. That graph is cargo metadata's resolve, not the build of
one package: it holds dependencies of weak optional features (`dep?/feature`),
edges under a cfg no target matches (`cfg(any())`) and the features the whole
workspace unifies. So a line may name a crate the core's own build does not
compile (today `indexmap`, `hashbrown`, `equivalent`, `foldhash` and `memchr`,
which `cargo tree -e normal,build` of the emulation packages leaves out), and
a feature change in a front-end crate can move the core's list. A crate is
`run` when normal dependencies alone reach it without a procedural macro,
`build` when it is reached through a build dependency or a procedural macro,
and `test` when only a test dependency of a package of the layer reaches it;
the lowest kind of any path counts. An external crate the list does not name,
or names with another kind, fails with the package, the crate, the kind, the
path and the rule, once per crate; a line whose crate the layer no longer
reaches fails too, so the list is exact and shrinks as crates leave. A new
line is a reviewed decision: a crate that touches the host (windows, graphics,
audio, input, network) has no acceptable reason.

`--self-test` runs the rules over the fixtures in
`scripts/crate-boundaries-fixtures/`, each with one planted case and its
expected outcome (`resolved/` for the rule that the build holds one copy of
`unemu-core-api`, `external/` for the external-crate list, each fixture there
with its list `NAME.external`); `--metadata FILE [--allow FILE]` checks any
`cargo metadata --no-deps` output, `--resolved FILE` the one-copy rule and
`--external FILE [--list FILE]` the external crates in any full `cargo
metadata` output. The `guard` gate runs the publication guard and then, in the
devShell, the self-test and the check; the publication guard and those two run
even when the other part failed, and the check does not run when the self-test
fails. The check takes well under a second. The check resolves the full graph
offline, so the gate runs `cargo fetch --locked` before it: a runner's empty
Cargo cache, or one restored from an older lock file, lacks the pinned git
dependencies.

### Core host interfaces

```sh
bash scripts/core-host-guard.sh              # exact host-interface exceptions in the core layers
bash scripts/core-host-guard.sh --self-test  # planted host uses and allow-list changes
bash scripts/core-host-guard.sh --allow FILE # check a candidate exception list
```

The script needs cargo and jq in the devShell. It reads the workspace's
`cargo metadata --offline --no-deps` layer tags, as the crate-boundary guard
does, and scans `src/**/*.rs` in `contract` and `emulation` packages. It
skips separate `tests/`, `tests.rs` and `*_tests.rs` files and stops each other
file at its first real `#[cfg(test)]` attribute ([core contract](design/core-contract.md)
§12.2–12.3). Line, doc and block comments (including nested and multiline
blocks) are ignored before detecting that cutoff or matching host access.
This is a spelling check: it catches common grouped and renamed imports,
including standard-stream handles, and short paths, but does not resolve
aliases at their later uses or scan past that cutoff.
It checks host filesystem, environment, network, process, threads, clocks,
OS modules and cfgs, streams and printing, mutable statics, thread-locals,
lazy caches, foreign functions, random sources and host-seeded `HashMap`s;
in emulation crates it also rejects `libc::` and `extern crate libc`.
Unsafe spellings are scanned too; `#![forbid(unsafe_code)]` in each emulation
crate makes the compiler enforce that rule throughout the crate, including tests.

`scripts/core-host-guard.allow` lists `FILE: MATCHED SOURCE LINE: REASON`.
Matches are literal full source lines with indentation and trailing whitespace
removed, keyed by repository-relative file. Every match must be listed and
every entry must still match; malformed and duplicate entries fail too. One
entry covers repeated identical lines in the same file. The initial list has
41 entries for 42 matched lines. Review and remove exceptions as host code
leaves the core. The self-test reports its case and planted-host counts. It
plants Instant in `devices`, both libc spellings and unsafe in `device`,
other host families, grouped and renamed imports, and production uses after
comments containing `#[cfg(test)]`. It checks comment filtering, separate
`tests/`, `tests.rs`, `*_tests.rs`, benches and examples, guest identifiers,
missing and stale entries, and a moved use, while excluding host-layer packages.
`--metadata FILE --root DIR` runs the same scanner on a fixture tree and its
`cargo metadata --no-deps` document. `AFFECTED_CARGO` selects a cargo wrapper,
as for the affected-tests runner. Both the bead gate's `guard` step and the
CI guard lane run the self-test and the check; CI already invokes that lane
through `scripts/ci-gate.sh`.

### Third-party licences

`bash scripts/third-party-licenses.sh [OUTPUT [TARGET]]` (in the devShell or with any cargo, jq and SHA-256 tool),
writes `THIRD-PARTY-LICENSES.md` (gitignored) for the release binaries: the
repository's own third-party components (Nuked-OPL3, the fonts, the logo) and
every crate `unemupc` and `upcli` link for the exact `Cargo.lock`, with the
licence files of each published crate (and, for a git dependency, of its
repository root), identical texts written once. A crate that publishes no
licence file is listed with its `authors` as copyright holders, and the
standard texts of its licences come from `LICENSES/` (MIT, Apache-2.0, Zlib).
`OUTPUT` is relative to the calling directory. `TARGET` (a target triple)
limits the crates to one platform's build; without it every platform's crates
are listed. It needs only cargo and jq; set `CARGO_NET_OFFLINE=true` to read
the dependency sources from cargo's cache only. The release build sets
`UNEMUPC_SOURCE_REV` to its tag or commit, so `--about` names the exact source
(`<repository>/tree/<rev>`); without it, the repository and the version.

### User-supplied ROM tests

This local opt-in lane is never called by CI; CI never has proprietary firmware
or the user's own files. The CI `goldens` gate runs every ignored test
(`--include-ignored`), these among them, so each must find its files before it
reads, builds or downloads anything, and skip with its SKIPPED line when they
are absent:

```sh
bash scripts/ci-gate.sh user-roms
# Equivalent command inside the devShell:
cargo test --locked --profile test --workspace --no-fail-fast -- --ignored user_rom_
```

These tests are `#[ignore]` with names starting with `user_rom_`. The shared
`assembly::require_user_firmware!(entry_id, board.firmware_catalogue())` helper
is available through assembly's `test-support` feature. It searches recursively
in `vendor/user-roms/` first (found from the `CARGO_MANIFEST_DIR` that `cargo
test` sets), then the optional directory
`UNEMUPC_USER_ROMS_DIR`. Set it to an absolute path to a folder of unpacked,
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
SKIPPED: user-supplied firmware <entry id> not found (set UNEMUPC_USER_ROMS_DIR or see vendor/user-roms/README.md)
```

If some parts match, the same line appends counts for each partial layout, such
as `; layout 0: 1 of 2 parts found`, without hashes or paths.

`UNEMUPC_REQUIRE_USER_ROMS=1` makes absence panic with that same text;
other values leave skipping enabled. `UNEMUPC_REQUIRE_FIXTURES` does not change
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
cargo test --offline -p pc-harness --test landmark -- --ignored user_rom_landmark_at_real_mode_prefix --nocapture
```

```sh
UNEMUPC_REQUIRE_USER_ROMS=1 bash scripts/ci-gate.sh user-roms
```

See `vendor/user-roms/README.md` for catalogue hashes and the proprietary golden rule.

The CI `fixtures` gate runs the Kiselev, FreeDOS, test386, XTIDE, hard-disk,
open VGA BIOS, open AT BIOS, at-pm-test, at-loadall-test and at-privilege-test scripts below; each checks pinned
output SHA-256 hashes. The gate prints `== fixtures: <script>` before each one
and stops at the first failure, so the last header names the failing script.
Downloads use the devShell's `curl` and verify against its pinned CA bundle,
not the host's (see [`dev-setup.md`](dev-setup.md#stack-specific-toolchain-rust)).
All but the two Nix-built BIOSes rebuild offline when
their pinned source commits and release archives are cached (Nix caches the
VGA and AT BIOS sources and store paths); `vendor/at-pm-test/build.sh`, `vendor/at-loadall-test/build.sh` and `vendor/at-privilege-test/build.sh` always build offline, each assembling its
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
| `vendor/vgabios/vgabios-0.9d-unemu.1-src.tar` | the fork at the pinned revision, `NOTICE` | LGPL-2.1-or-later |
| `vendor/unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar` | the fork at the pinned revision (without upstream's prebuilt `binaries/` and `xtide_r625/`), `NOTICE` | GPL-3.0-or-later |

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
`UNEMUPC_REQUIRE_FIXTURES=1`, so a missing archive fails there:

```sh
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p pc-harness --test media_fixtures source_archive -- --include-ignored
```

The XTIDE Universal BIOS option-ROM fixtures:

```sh
vendor/xtide-universal-bios/fetch.sh           # r638 AT binary, padded/checksummed to 8 KiB
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p assembly --lib xub
```

It checks both the download and output SHA-256, reusing
`vendor/xtide-universal-bios/build/ide_at.bin` and the source export
`build/trunk-r638/` for offline rebuilds. The output is
`vendor/xtide-universal-bios/ide_at-r638-8k.bin`. Its catalogue entry is `Listed`;
building it does not add an ATA device or hard-disk boot profile. See
`vendor/xtide-universal-bios/README.md` for provenance, padding and checksum rules.

The open VGA BIOS and its INT 10h probe floppy are built with Nix (in the
devShell) from the fork's public repository,
<https://github.com/unemu-org/vgabios>:

```sh
vendor/vgabios/fetch.sh                        # vgabios-std.bin and int10-probe-1200.img
UNEMU_VGABIOS_CHECKOUT=../vgabios vendor/vgabios/fetch.sh   # pinned revision from a local clone
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p assembly --lib open_vga
cargo test --offline -p pc-harness --test open_vga_golden -- --include-ignored
```

The script builds release `v0.9d-unemu.1` at its pinned revision, read
anonymously over HTTPS, and checks both output SHA-256 values before replacing
anything, then writes the source archive of the tree it built from.
`UNEMU_VGABIOS_FLAKE` overrides the whole flake reference (a mirror that
carries the pinned revision) and `--print-flake` shows the one it would build.
The CI `fixtures` gate needs HTTPS access to GitHub and no credentials; a
failed build names the repository, the tag and the revision and both
overrides. The open VGA golden
`open_vga_bios_teletype_bel_sounds_the_speaker_for_31_64_seconds` reads the
teletype BEL tone from the headless speaker trace
(`ObservationPolicy::speaker_trace`): one start from the video BIOS with
counter 2 in mode 3 and divisor 1331, and its end 31/64 s later within 1%.
The open VGA tests then do not skip: the `goldens` gate sets
`UNEMUPC_REQUIRE_FIXTURES=1`, so a missing ROM or probe fails them. Locally,
without that variable, they print a skip line. See `vendor/vgabios/README.md`
for pins, licence and the later registry delivery. `machines/at-games-open.toml`
needs the ROM.

The open AT BIOS, the unemu fork of Kiselev 8088_bios, is built the same way
from its public repository, <https://github.com/unemu-org/at-bios> (a GitHub
fork of `skiselev/8088_bios`, renamed from `unemu-at-bios`):

```sh
vendor/unemu-at-bios/fetch.sh                  # bios-unemu-at.bin and its source archive
UNEMU_AT_BIOS_CHECKOUT=../at-bios vendor/unemu-at-bios/fetch.sh   # pinned revision from a local clone
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline -p assembly --lib unemu_at_bios
UNEMUPC_REQUIRE_FIXTURES=1 cargo test --offline --release -p pc-harness --test xms_warm_boot_golden -- --ignored --nocapture
```

The restart golden `xms_warm_boot_golden` boots the pinned ROM; with
`UNEMUPC_OPEN_AT_BIOS=/path/to/bios-unemu-at.bin` it boots that image instead,
so a release candidate of the BIOS can be checked before it is pinned.

It builds release `v1.0.2-unemu.3` at its pinned revision (`#unemu-at`),
checks the ROM's SHA-256, then writes the source archive. The overrides are
`UNEMU_AT_BIOS_FLAKE` and `UNEMU_AT_BIOS_CHECKOUT`, and `--print-flake` shows
the reference; in CI it needs HTTPS access to GitHub and no credentials.
`machines/at-games-open.toml`,
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

It uses mtools 4.0.49 with a fixed serial, label and FAT-epoch timestamps, and
runs mtools with an empty `HOME` so no `~/.mtoolsrc` applies.
See `vendor/hdd/README.md` for the IBM AT type-2 geometry, partition/BPB layout,
hash and licence. The CI `fixtures` gate builds it.

Each gate uses `nix develop --no-update-lock-file --command` (except `guard`, which
runs directly), preserves its exit status, and tees combined output to the gitignored
`ci-logs/<step>.log`. Set `CI_LOG_DIR` to choose another log directory. Cargo builds/tests
use `--locked`; the clippy gate lints all features and then the production CLI
binary without dev-dependency feature unification; formatting uses `cargo fmt --check`. CI runs every gate even after failures,
saves the Cargo cache, uploads `ci-logs/` as a run artifact, and names failing
steps in its final Gate summary step; any failed gate fails the job.
See [`dev-setup.md`](dev-setup.md#ci) for the runner and cache setup.

## Releases

A release is a tag `v<version>` that matches `version` under
`[workspace.package]` in `Cargo.toml`; bump that first. The same commit raises
`NEXT_RELEASE` in `crates/image-store/src/compat.rs` past the new version
(`next_release_is_above_the_package_version` fails until it does); rows already
written for the release keep their literal version. The same commit updates
the sentence in Minimum release of an image that names this build's level and
package version. The maintainer pushes the
tag and creates and publishes the GitHub release for it. Publishing starts
`.github/workflows/release.yml`, which builds `upcli` and `unemupc` and
attaches them to that release; nothing goes to a package registry.
Before tagging, run every test tier on the whole workspace, including the
user-ROM tests CI never has:
`UNEMUPC_REQUIRE_FIXTURES=1 UNEMUPC_USER_ROMS_DIR=/path/to/roms bash scripts/affected-tests.sh --all --tier all`.

```sh
git tag -s v0.1.0 -m 'unemupc 0.1.0' && git push origin v0.1.0
gh release create v0.1.0 --repo unemu-org/unemupc --verify-tag --title 'unemupc 0.1.0' --notes-file NOTES.md
gh run list --repo unemu-org/unemupc --workflow release.yml
gh workflow run release.yml --repo unemu-org/unemupc -f tag=v0.1.0   # build again, replace the assets
```

The release gets `unemupc-<version>-x86_64-unknown-linux-gnu.tar.gz` (built
on Ubuntu 22.04, so it needs glibc 2.35 or newer),
`unemupc-<version>-aarch64-apple-darwin.tar.gz` (macOS on Apple silicon,
unsigned) and `SHA256SUMS`. Each archive unpacks to
`unemupc-<version>-<target>/` with both binaries, `LICENSE`, `README.md`,
`THIRD-PARTY-LICENSES.md` from `scripts/third-party-licenses.sh` for that
target, and `notices/`: the repository's own notices that file quotes and
`LICENSES/`, at their repository paths. The workflow appends download notes
to the release text once, including how to clear the macOS quarantine flag
(`xattr -d com.apple.quarantine unemupc upcli`).

The binaries are built outside Nix, with rustup at the devShell's Rust
(`RUSTUP_TOOLCHAIN` in the workflow; a step checks it against
`rust-version`), `--locked --release`, `--remap-path-prefix` for the
workspace, cargo's registry sources and git checkouts and the toolchain, and
`UNEMUPC_SOURCE_REV` set to the tag, so `--about` names
`<repository>/tree/<tag>`. A check step fails the job when `strings` finds
the runner's home, the workspace, cargo's home or the toolchain in a binary,
or, on macOS, when `otool -L` lists a library outside `/usr/lib` and
`/System/Library`.

Verify a download in the folder that holds it and `SHA256SUMS`:

```sh
sha256sum --ignore-missing -c SHA256SUMS         # Linux
shasum -a 256 --ignore-missing -c SHA256SUMS     # macOS
```

### Using upcli in other repositories' CI

The composite action `.github/actions/setup-upcli` installs a released
`upcli` on a Linux X64 or macOS ARM64 runner, so the media and firmware
repositories import and push with this project's own CLI and push guard.
Its `README.md` has the inputs, the output and an example job. A consumer
pins the action by commit and names the release:

```yaml
- uses: unemu-org/unemupc/.github/actions/setup-upcli@<unemupc commit SHA>
  with:
    version: v0.0.2
    # commit: <40 hex>   # optional; releases after v0.0.1: must match upcli --version
```

The action downloads the archive with curl, checks it against the release's
`SHA256SUMS` before it unpacks anything (a check of the download, not a pin), puts `upcli`, `LICENSE` and `THIRD-PARTY-LICENSES.md` in a
new directory under `$RUNNER_TEMP` and on `PATH`, and prints `upcli
--version`; its `path` output names the binary. Its inputs reach its script,
`install.sh`, only through the environment. Registry credentials stay with
the consumer: `UNEMU_REGISTRY_USER`, `UNEMU_REGISTRY_PASSWORD` and
`UNEMU_REGISTRY_HOST`, on the push step only.

`.github/workflows/setup-upcli.yml` tests the action when its files change:
on `ubuntu-latest` and `macos-latest` it checks that a release that does not
exist fails and installs nothing, then installs v0.0.1 and runs `upcli --version`. To run
the script by hand, give it the inputs and the runner's variables:

```sh
mkdir -p /tmp/setup-upcli && : > /tmp/setup-upcli/path && : > /tmp/setup-upcli/output
UPCLI_VERSION=v0.0.1 RUNNER_OS=macOS RUNNER_ARCH=ARM64 \
  RUNNER_TEMP=/tmp/setup-upcli GITHUB_PATH=/tmp/setup-upcli/path GITHUB_OUTPUT=/tmp/setup-upcli/output \
  bash .github/actions/setup-upcli/install.sh
gh run list --repo unemu-org/unemupc --workflow setup-upcli.yml
```

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
