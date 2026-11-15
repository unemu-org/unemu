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
cargo run --offline -p cli -- image media ls --store /path/to/store
cargo run --offline -p cli -- image media show install:1.0 --store /path/to/store
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
Tags are `firmware/<catalogue-id>:1`; identical packages report already imported,
while different digests conflict before copying, with CAS at publication. `--rescan` bypasses reusable persistent ROM hashes. `--dry-run`
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
the same ROM and document blobs; the old tag keeps the old package. `show`
prints every layer's class and licence and, from the push guard's own
classification (local marks and the catalogue included), where the package
may be pushed.
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
private = true      # may receive user-supplied blobs outside unemu/
user = "NAME"       # Keychain account; never a password here
```

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
- Add and Edit change the private flag and the loopback plain-HTTP opt-in.
  Only changed keys are written, an entry that changed since the dialog opened
  is refused with "reopen it", and un-marking private asks for confirmation.
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
  verdict while the destination is typed. A non-version tag moves only after
  a confirmation that shows both digests, and only from the digest shown. A
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
user-supplied, and machine import marks everything so). An image with a
user-supplied blob goes only to a registry marked private, never into `unemu/`;
`unemu/` requires every blob to be open; a restricted push also refuses a
repository that answers anonymous reads, and one whose anonymous answer is a
Bearer token challenge (inconclusive; Bearer authentication is not supported
yet). There is no override. `--dry-run` prints the target, the image digest, one
`role digest size media-type class source` line per blob (source `this image`,
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
bash vendor/kiselev-bios/fetch.sh
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
| `unemu/firmware/kiselev-xi8088-1.0.2-atreset:1.0.1`, `:latest` | `unemu/firmware/kiselev-xi8088-1.0.2-atreset` | Kiselev AT-reset BIOS package with its source archive |
| `unemu/firmware/vgabios-0.8a-unemu.1-std:1.0.1`, `:latest` | `unemu/firmware/vgabios-0.8a-unemu.1-std` | unemu LGPL VGA BIOS package with its source archive |
| `unemu/firmware/xub-r638-xtp-ata1f0:1.0.1`, `:latest` | `unemu/firmware/xub-r638-xtp-ata1f0` | XTIDE Universal BIOS r638 package with its source archive |
| `open-at:1.0.1`, `:latest` | `unemu/open-at` | IBM 5170-class 80286, the three ROMs, 640 KiB + 1 MiB, VGA, COM1 without a mouse, one empty 1.2M drive, a blank zero-base 504 MiB (1024/16/63) ATA disk at 1F0h, no sound card |
| `open-at-games:1.0.1`, `:latest` | `unemu/open-at-games` | `open-at` plus SB16 (220h, IRQ 5, DMA 1/5), a Microsoft mouse on COM1 and a 2 MiB Lo-tech EMS board (260h, frame D000h) |

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
cargo run --offline -p cli -- image push unemu/firmware/kiselev-xi8088-1.0.2-atreset:1.0.1 unemu/firmware/kiselev-xi8088-1.0.2-atreset:1.0.1 --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/vgabios-0.8a-unemu.1-std:1.0.1 unemu/firmware/vgabios-0.8a-unemu.1-std:1.0.1 --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/xub-r638-xtp-ata1f0:1.0.1 unemu/firmware/xub-r638-xtp-ata1f0:1.0.1 --store "$S" --dry-run
cargo run --offline -p cli -- image push open-at:1.0.1 unemu/open-at:1.0.1 --store "$S" --dry-run
cargo run --offline -p cli -- image push open-at-games:1.0.1 unemu/open-at-games:1.0.1 --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/kiselev-xi8088-1.0.2-atreset:latest unemu/firmware/kiselev-xi8088-1.0.2-atreset:latest --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/vgabios-0.8a-unemu.1-std:latest unemu/firmware/vgabios-0.8a-unemu.1-std:latest --store "$S" --dry-run
cargo run --offline -p cli -- image push unemu/firmware/xub-r638-xtp-ata1f0:latest unemu/firmware/xub-r638-xtp-ata1f0:latest --store "$S" --dry-run
cargo run --offline -p cli -- image push open-at:latest unemu/open-at:latest --store "$S" --dry-run
cargo run --offline -p cli -- image push open-at-games:latest unemu/open-at-games:latest --store "$S" --dry-run

cargo run --offline -p cli -- image push unemu/firmware/kiselev-xi8088-1.0.2-atreset:1.0.1 unemu/firmware/kiselev-xi8088-1.0.2-atreset:1.0.1 --store "$S"
cargo run --offline -p cli -- image push unemu/firmware/vgabios-0.8a-unemu.1-std:1.0.1 unemu/firmware/vgabios-0.8a-unemu.1-std:1.0.1 --store "$S"
cargo run --offline -p cli -- image push unemu/firmware/xub-r638-xtp-ata1f0:1.0.1 unemu/firmware/xub-r638-xtp-ata1f0:1.0.1 --store "$S"
cargo run --offline -p cli -- image push open-at:1.0.1 unemu/open-at:1.0.1 --store "$S"
cargo run --offline -p cli -- image push open-at-games:1.0.1 unemu/open-at-games:1.0.1 --store "$S"

cargo run --offline -p cli -- image push unemu/firmware/kiselev-xi8088-1.0.2-atreset:latest unemu/firmware/kiselev-xi8088-1.0.2-atreset:latest --store "$S"
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

