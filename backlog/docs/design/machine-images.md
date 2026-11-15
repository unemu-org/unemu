# Machine images, capabilities and desktop UX

Status: **`+zstd` binary blobs implemented** (§2), 2026-11-14: provider-declared payloads
are stored compressed (level 3, single-threaded) with decoded SHA-256/size annotations;
resolution verifies decoded bytes by streaming and writes nothing; only mounted file-backed
media are decoded into the rebuildable materialized cache, which `image sweep` trims;
imports and publication compress by default, `--no-compress` keeps raw fixtures, and
`image compress` preserves the machine digest;
**desktop UX and toolkit accepted**, 2026-11-13 (decision-24);
**installation media set codec/CLI/insert implemented**, 2026-11-14 (decision-26);
**firmware package codec/import/resolution implemented; UI pending**, 2026-11-14 (decision-27).
The installation-media library collection and Import dialog are implemented (2026-11-15); the transfer-medium generator remains.
§5.3 and its [Firmware mock-up](mockups/firmware.html) were approved by the maintainer on 2026-11-14.
§5.2 and its mock-up were approved by the maintainer on 2026-11-14. The local store core
(`upc-5pw.6`), metadata codec, local overlays and summary cache (`upc-5pw.12`)
are implemented in `crates/image-store`; PC machine-file import and bundle
resolution (`upc-5pw.7`), immutable disk layers, isolated workspaces and offline
publication (`upc-5pw.8`) are implemented in `crates/media`, `crates/image-store`
and `crates/pc-image`, with opt-in CLI commands. The desktop library, pinned launch,
regular/disposable workspaces with explicit keep/discard and startup recovery, and
Disk layers publication are implemented in `crates/window`. Registry push/pull (§5.1,
`upc-5pw.10`), the push guard (`upc-5pw.36`) and the CLI commands are implemented in `crates/registry`
(`image-registry`); bearer-token authentication, GUI login and Browse registry remain.
Provider launch compilation remains separate; those sections describe later contracts.

## 1. Scope and existing contracts

The approved [window mock-up](mockups/main-window.html) and
[decision-24](<../../decisions/decision-24 - Desktop shell is a classic in-window menu UI built with egui.md>)
define one classic application window, in-window menus on every platform and an
egui/eframe shell. The welcome/library client area becomes the emulated screen
on Start; settings, media and layer actions use modal dialogs. The HTML contains
sample data and illustrative interactions, not emulator wiring. Section 7
describes this accepted direction; implementation starts with the shell and
existing machine files, before the image store and layer dialog.

A machine image describes the **whole machine**, not just a hard disk: platform,
resolved hardware configuration, firmware bytes, every controller/slot and
ordered media catalogue, every disk base and layer, initial inserted media,
platform NVRAM and optionally a compatible saved state. For the PC, firmware
includes system BIOS, video BIOS and option ROMs, and NVRAM is CMOS.
A tag such as `dos-lab:1.0` names this complete graph. An image is
self-contained: running it never reads user ROM folders or any other host file
outside the store. A local tag is published only after the whole closure is
present and verified, so an interrupted pull or import never leaves a
half-present tagged image (§3.1).

Three identities are used consistently below:

| Identity | What is hashed | Changes when |
|---|---|---|
| **Blob digest** | The exact stored bytes of one object. | Never; different bytes are a different blob. |
| **Machine digest** | The config blob (§2): platform, hardware, firmware digests, media heads, slots, NVRAM and saved state. | Anything the guest can observe changes. |
| **Image digest** | The OCI manifest: config descriptor, closure list, metadata and artwork descriptors, annotations. | The machine digest changes, or only catalogue metadata, artwork or redistribution marks change. |

Workspaces, saved states and tests pin the machine digest. Tags and registry
transfer use the image digest. A metadata edit therefore gives a new image
digest for the same machine, and nothing that runs is affected. Library edits
remain local overlays until explicitly published as a new version or tag (§2.1).

READ: [Machine files](machine-files.md), §§Schema, extends and Removable floppy
catalogues; [Persistent media](persistent-media.md), §§Header, Media behind the
drive, Memory and hashing, and Media changes; [Deferred](../deferred.md),
§Layered disk images. The existing `SectorMedia` seam, immutable verified base,
sparse sector overlay, command batches and recovery generations remain the
foundation. A generation is a recovery point, not a disk layer or a filesystem
consistency guarantee. A machine-file inheritance layer is also not a disk layer.

READ: `crates/window/src/main.rs` queues floppy changes at execution boundaries;
`machines/README.md` documents Ctrl+Alt+F/E (Shift for B:), Ctrl+Alt+M mute,
click-to-capture and Ctrl+Alt+G/focus loss release. The approved shell makes these
controls visible. CD, whole-machine save/restore and new hardware remain future
capabilities, even when their reserved positions appear in a mock-up.

### Platform neutrality

The image design serves the unemu family: this PC emulator, a future x80 emulator
for 8080/Z80 systems (CP/M, MSX, ZX Spectrum), and later C64 and Atari emulators.
The store, registry client, metadata, library index, summary cache and classic
shell widgets belong in shared crates. Store layout, tags, disk layers and image
identities are platform-neutral. A required machine `platform` routes to an
emulator provider; it identifies a machine family, not a host OS or CPU alone.
Examples are `ibm-pc-at`, `zx-spectrum`, `c64` and `atari-800`; the list is extensible.

Each emulator supplies its machine-config schema, validator capabilities (§6),
devices/core, supported media kinds and launch-command mechanics (§2.1). The PC
uses today's machine-file schema. Shared crates never depend on a specific
emulator: platform providers implement the shared interfaces, and the application
composes them. Shared code routes config validation and execution by platform;
it does not embed PC hardware or guest-command rules. Start stays disabled with
a reason when the selected provider or a required capability is unavailable.

The config, metadata blob and manifest annotation carry the same machine
platform (§2). Per-title catalogue platforms (§2.1) describe software environments
such as DOS or CP/M separately. PC-specific schemas, media and UI examples below
illustrate one provider rather than requirements on every emulator.

## 2. Immutable manifest and identity

Use a standard **OCI image manifest** as an artifact, following the
[OCI image-spec](https://github.com/opencontainers/image-spec) version 1.1,
Image Manifest, Content Descriptors and Image Layout sections. The JSON envelope
has `schemaVersion = 2`, `mediaType = "application/vnd.oci.image.manifest.v1+json"`
and `artifactType = "application/vnd.unemu.machine.v1"`. Its `config`
descriptor points to the version-1 machine configuration TOML. `layers` lists
exactly the closure of that configuration: every firmware blob, medium, disk
base, sealed layer descriptor, sector pack, platform NVRAM and optional saved state
reachable from it, including catalogue media that are not initially inserted,
followed by the metadata blob and its artwork (§2.1). A manifest with a missing
or an unreferenced descriptor is rejected. These are artifact payloads, not
container filesystem tar layers.

| Media type | Role |
|---|---|
| `application/vnd.oci.image.manifest.v1+json` | Standard OCI manifest envelope. |
| `application/vnd.oci.image.index.v1+json` | Standard OCI index for local image references. |
| `application/vnd.unemu.machine.v1` | Manifest `artifactType`; identifies a whole machine. |
| `application/vnd.unemu.machine.config.v1+toml` | Required platform and platform-owned resolved hardware, firmware/media bindings, catalogues, selections and policies. |
| `application/vnd.unemu.metadata.v1+json` | Required machine platform and versioned game/software catalogue, with independent metadata and launch commands per title. |
| `image/png`, `image/jpeg` | Cover art and screenshots referenced from the metadata blob. |
| `application/vnd.unemu.firmware.v1` | Exact ROM/chip/container payload bytes; also the firmware-package manifest `artifactType` (§5.3), distinguished by descriptor context. |
| `application/vnd.unemu.disk.base.v1` | Complete raw fixed-disk base. |
| `application/vnd.unemu.disk.zero.v1+toml` | PC zero base: a blank fixed disk recorded only as geometry and capacity; every sector is zero (§4). |
| `application/vnd.unemu.disk.layer.v1+toml` | Sealed fixed-disk layer descriptor from §4. |
| `application/vnd.unemu.disk.sectors.v1` | Sector pack referenced by a sealed layer. |
| `application/vnd.unemu.media.floppy.v1` | Complete raw floppy medium. |
| `application/vnd.unemu.media.optical.v1` | Complete read-only optical image with 2048-byte sectors; storable in media sets (§5.2), rejected as a machine binding until supported. |
| `application/vnd.unemu.cmos.v1` | PC-specific frozen 64-byte CMOS image. |
| `application/vnd.unemu.machine.state.v1` | Reserved opaque saved state; rejected until supported. |

The table is extensible: platform-specific media types for tape images,
cartridges and other media can be added under `application/vnd.unemu.*` when
their formats and supporting providers are defined. Shared storage and transfer
treat their payloads as opaque bytes; support is never inferred from the prefix.

A compressed binary payload appends `+zstd` to its media type. Codec support
must land before any phase C multi-gigabyte image is pushed; its multi-gigabyte
bases must use `+zstd` (§5.1). OCI
`digest` and `size` always describe the stored bytes, compressed when applicable;
the machine format must additionally declare and verify decoded length and full
SHA-256 before use. Compression never weakens full-base verification. Metadata
stays uncompressed in version 1. OCI treats payloads as opaque bytes; filesystem
inspection (for example FAT-aware tools), deduplication and compression belong
only in the appropriate media tools.

This is an abbreviated schema sketch, not a copyable image. `<...>` stands for
a full `sha256:` plus 64 lowercase hexadecimal characters; sizes are illustrative.
Every payload, including transitive layer parents and sector packs, needs its
own OCI descriptor so an ordinary registry client can copy the complete image.

```json
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "artifactType": "application/vnd.unemu.machine.v1",
  "config": {
    "mediaType": "application/vnd.unemu.machine.config.v1+toml",
    "digest": "<machine-config>",
    "size": 2048
  },
  "layers": [
    {
      "mediaType": "application/vnd.unemu.firmware.v1",
      "digest": "<system-bios>",
      "size": 65536,
      "annotations": {
        "org.opencontainers.image.title": "assets/system.bin",
        "unemu.redistribution": "user-supplied"
      }
    },
    {
      "mediaType": "application/vnd.unemu.disk.layer.v1+toml",
      "digest": "<sealed-disk-layer>",
      "size": 512,
      "annotations": {
        "unemu.redistribution": "user-supplied"
      }
    }
  ],
  "annotations": {
    "org.opencontainers.image.title": "DOS lab",
    "unemu.platform": "ibm-pc-at",
    "org.opencontainers.image.base.digest": "<previous-machine-image>"
  }
}
```

Annotations are string values and never bind hardware or media:

| Annotation | Meaning |
|---|---|
| `org.opencontainers.image.title` | On a descriptor: optional logical name for generic OCI tools. On the manifest: a preview title for registry browsing. Informational only. |
| `org.opencontainers.image.base.digest` | Manifest only: image digest this image was derived from. Provenance; never needed to run and never a reason to download an older image. |
| `unemu.platform` | Required on the manifest: machine platform, matching config and metadata; a routing hint for registry/library previews, never an independent authority. |
| `unemu.redistribution` | `open` or `user-supplied`; absent evidence defaults to user-supplied. |
| `unemu.license` | Reviewed SPDX licence expression for open content. |
| `unemu.provenance` | Publish-safe evidence supporting the open classification. |

The config is the single authority for bindings, slots, catalogue order and
media heads; layer descriptors (§4) are the single authority for parents.
The required config `platform` is the authority for routing. Its value must match
the metadata envelope and `unemu.platform` annotation before local tag publication;
a mismatch or missing platform is rejected. Metadata edits cannot change this
identity field. The annotation is a preview hint, not permission to launch without
validating the config.
Identical bytes appear once in `layers`, however many bindings name them.
Redistribution marks also apply to the config and metadata descriptors; a
blob's effective class is the most restrictive one recorded for its digest in
any local manifest. The writer emits `layers` in config traversal order, then
metadata and artwork, so the same content always gives the same manifest bytes.
`layers` enumerates the closure; per-medium heads and parents determine disk
reconstruction, not one global disk stack.

The config TOML has a common routing envelope with required `schema = 1` and
`platform`. Each platform owns the schema of its machine config and its versioned
validation. The PC bundle profile below puts flattened hardware in `[machine]`
and bindings in the remaining tables; other platforms need not model PC drives
or CMOS. The config holds the platform and only what the guest can observe: no
name, tag, history or catalogue metadata. This PC example uses:

```toml
schema = 1
platform = "ibm-pc-at"

[machine]
# Resolved schema-1 machine fields; hardware details omitted in this sketch.
schema = 1

[[firmware]]
key = "system-bios"
source = "assets/system.bin"
blob = "<system-bios>"
# Split chips use separate source/blob bindings; machine retains lane and offset.

[[media]]
id = "boot"
source = "assets/boot.img"
head = "<raw-floppy>"
policy = "readonly"
geometry = { cylinders = 80, heads = 2, sectors = 18, sector_bytes = 512 }

[[media]]
id = "tools"
source = "assets/tools.img"
head = "<raw-tools-floppy>"
policy = "journal"
geometry = { cylinders = 80, heads = 2, sectors = 18, sector_bytes = 512 }

[[media]]
id = "system"
source = "assets/system.img"
head = "<sealed-disk-layer>"
policy = "journal"
geometry = { cylinders = 66, heads = 16, sectors = 63, sector_bytes = 512 }

[[slots]]
controller = "fdc-765"
unit = 0
catalogue = ["boot", "tools"]
inserted = "boot"

[[slots]]
controller = "fdc-765"
unit = 1
catalogue = []
# Omitted inserted means empty, not an implicit medium zero.

[[slots]]
controller = "ide"
unit = 0
catalogue = ["system"]
inserted = "system"

[nvram]
mode = "journal"                 # image also allowed; omit for derived/blank
source = "assets/cmos.bin"
head = "<raw-64-byte-cmos>"
# RTC start stays in the resolved machine config, never the host clock.

# Optional future field; reject when machine save/restore is unavailable.
# [saved_state]
# blob = "<machine-state>"
# compatibility = "<emulator-cpu-device-config-media-identity>"
```

SHA-256 identifies exact stored bytes, including the JSON manifest and UTF-8 TOML;
never hash an unspecified TOML-to-JSON conversion. A versioned writer produces
stable key order, LF and no timestamps; a hand-edited equivalent config is a
different machine digest. Check each descriptor's digest and exact byte length
before decoding. Unknown fields or versions in the manifest, config or layer
descriptors fail closed; the selected provider validates its config schema. The
metadata blob degrades instead (§2.1). Bound each manifest, config, layer
descriptor and metadata blob to 1 MiB, each artwork blob to 4 MiB, chain depth to
64 and graph traversal to 10,000 objects before
fetching payload bytes.

### Resolved hardware and media bindings

For `platform = "ibm-pc-at"`, `config.machine` is a flattened schema-1 machine
TOML, with no `extends` or host endpoints. Use the current resolver first,
preserving board/card order, defaults, RTC start, CPU/FPU, memory, ROM
addresses/services, split-chip assembly and drive geometry.
Import materializes all lowering defaults in schema-1 config, so later default changes cannot alter an existing image without changing its digest.
Replace image paths with unique relative logical names (`assets/...`); these
names are looked up only in manifest bindings, never opened on the host.
Reject absolute paths, `..`, duplicate names, unresolved sources and unbound blobs.
This requires a future bundle loader; the current file CLI does not resolve it.

The config envelope separates hardware/firmware mapping in `machine` from media
content and initial selection in its bundle tables. Config drive entries keep
controller, unit/type/geometry but omit `image`, `sha256`, `media`, `empty`,
`write`, `journal` and `keep_generations`.
Machine NVRAM retains derived/blank without a bundle NVRAM table, or an image
with its logical name; image/journal policy belongs to the bundle table.
This **bundle-config profile** is not a standalone machine file: the loader
fills these holes from the bundle tables and OCI payload descriptors
before the existing lowering/validation path. No new hardware schema is invented.
ROM `image` names bind to `firmware.source`; hashes pin whole containers/chips,
with assembled BIOS pins retained when present. Catalogue compatibility and
firmware services still come from the existing catalogue and assembly checks.

Existing machine-file fields map to this envelope as follows:

| Machine-file field | Image representation |
|---|---|
| `schema`, board/CPU/memory, ordered `card` entries and RTC `start` | Resolved under `config.machine`; preserve defaults and order. |
| `extends` and source origins | Resolve at import; no runtime inheritance or source paths. |
| Firmware `image`/`sha256`, chip lanes/offsets, ROM mapping and services | Logical `assets/...` names in `machine`, exact firmware digest in bindings/OCI descriptors; preserve mapping and assembly pins. |
| Drive controller/unit/type/geometry | Hardware stays in `machine`; matching `slots` tables bind content. |
| Drive `image`/`sha256` and ordered `media` catalogue | `media` entries point to content heads; `slots.catalogue` preserves order and `inserted` records the initial choice. |
| Drive `empty` | Omit `slots.inserted`; never infer medium zero. |
| `write`, `journal`, `keep_generations` | Image policy is `readonly`, `volatile` or `journal`; journal paths and retention settings stay local in the workspace. |
| `[board.rtc].nvram` | Derived/blank remains hardware config; image/journal forms bind a CMOS blob in `nvram`, with workspace-local writes. |

No host paths, registry endpoints or credentials are stored in the image.

Require exact agreement between slot bindings and config drive positions. Units
mean controller-local FDC unit or IDE master=0/slave=1; array order cannot change
wiring. Unremovable IDE drives accept exactly one inserted medium; a slave still
requires a master. A medium cannot be mounted writable in two slots. Every floppy
catalogue entry must match the drive's current exact geometry, not merely fit.
Unused catalogue media remain part of the image graph. The bundle loader derives
write modes and local journal paths from policy/session metadata; there is no
second write-policy field in config. Standalone export reconstructs a complete
ordinary machine file and new raw images; it never exposes profile holes.
Media policies are `readonly`, `volatile` or `journal`, matching today's modes.
Volatile writes stay workspace-local in memory and are lost on close; only
journaled workspace media offer crash recovery. Do not silently upgrade policy.

### 2.1. Game and software metadata

Status: metadata codec, local overlay and summary cache implemented by
`upc-5pw.12` in `crates/image-store`. Launch-profile compilation and input scripts,
as well as GUI wiring, are not implemented here.

Rich catalogue metadata belongs in version 1, even for a purely local library.
Each machine manifest lists one JSON blob of type
`application/vnd.unemu.metadata.v1+json`. It never reaches the emulator core
and is not part of the machine digest. Metadata and artwork are ordinary blobs
in the image manifest. Materializing an edit writes new small metadata/artwork
blobs and a manifest with a new image digest; the machine digest and existing
config, firmware and disk blobs stay unchanged.

Library edits stay in a **local overlay** in `local/library.toml`, keyed by the
source image digest, until the user publishes them as a new version or tag.
The original image and any pushed version tag stay unchanged (§5.1); nothing is
automatically pushed. New artwork blobs referenced by an overlay are GC roots.
Publishing applies the overlay, verifies the new closure and creates the new
image/tag; active workspaces still pin the same machine digest.

The JSON envelope is
`{"schema": 1, "platform": "ibm-pc-at", "labels": [...], "titles": [...]}`.
The required top-level `platform` is the machine routing id from the config (§2),
not a per-title software environment. `labels` are the image-level labels that
travel with the image. A machine may contain zero, one or several titles; every
record has a stable unique `id` and required `title`. Other fields are optional;
absent means unknown, and arrays retain all values. A reader that meets a newer `schema` still runs the image, shows what it
understands read-only and never rewrites that blob.

| Per-title fields | Schema v1 form |
|---|---|
| `title`, `sort_title`, `series` | Strings; sort title permits ignoring articles without changing the displayed title. |
| `genres`, `developers`, `publishers` | Arrays of strings; multiple genres and credited companies are supported. |
| `platform` | Extensible controlled vocabulary: `DOS`, `Windows 3.x`, `Windows 9x`, `CP/M`, `MSX`, `ZX Spectrum`, `C64`, `Atari 8-bit`, `Atari ST`, `other`; distinct from the envelope machine platform. Preserve unknown future values for display and indexing. |
| `release_year`, `players` | Integer year; players is `{ "min": 1, "max": 2 }` with positive bounds and min <= max. |
| `languages`, `regions` | Arrays of strings. |
| `description`, `version`, `edition`, `source_media` | Strings; source media may read `4 x 1.2M floppies`. |
| `tags` | Array of free-form tags that travel with the image. |
| `cover`, `screenshots` | One artwork blob digest and an ordered array of them; each is listed in the manifest `layers`. |
| `launch` | Optional provider-defined object selected by `profile`; the PC command profile uses a guest `program` path, `arguments` string array and `working_directory` path. |

Personal `user_rating` (optional integer 0–5) and free-text `notes` live only in
`local/library.toml`, associated with the library entry and stable title id.
They are absent from the metadata blob, archives and pushes; editing them never
creates an image revision or triggers a push.

Suggested genres: action, adventure, RPG, strategy, simulation, sports, racing,
puzzle, platformer, shooter, educational, demo and utility. This is a suggestion
list, not a closed vocabulary; free genres are allowed.

Each title can offer its own **Play “Title”** action while sharing one image and
workspace. A launch is **host input, not machine content**: it compiles to a
deterministic input script, the same kind of input as recorded keystrokes, so it
never edits a disk and never changes the machine digest. A run is a function of
the machine digest, the workspace's media state and the input script; a test
that uses a launch pins the script bytes, not the metadata blob.

`launch.profile` names a boot profile supplied by the selected emulator. The
shared metadata/launch widgets carry the request; the provider validates its
fields and compiles the input script using its guest paths and input mechanism.
The `program`, `arguments` and `working_directory` fields illustrate the PC
command profile; future profiles may define platform-specific launch fields.
The initial PC launch proposal is **DOS only**, through `dos-prompt`; storing
Windows or other platform metadata does not enable launching it.
A profile waits for a guest-observable condition, for example the keyboard
buffer being polled while empty with a prompt on the text cursor row, sampled at
retired-instruction boundaries through the observation seam. It then types the
drive change, `CD` and the command line as scancodes at fixed instruction
spacing. There are no wall-clock sleeps and no fixed boot delays, so a workspace
whose disk has changed boots along a different path and still reaches the same
condition. If the condition does not appear within a fixed instruction budget,
scripting stops, the machine keeps running and the status bar says the launch
was not performed. Paths are guest paths, never host commands, limited to
characters the profile can type on the US layout. The command names files in
this image's heads; a diverged workspace may no longer have them, which shows as
an ordinary guest error. Phase C Windows launches use typed input too, such as
Start > Run keystrokes, through a Windows-specific profile at deterministic
input boundaries. There is no guest-side helper device. Without a supported
profile, offer ordinary boot; this design does not claim launch support already
exists.

**Artwork travels inside the image.** Covers and screenshots are PNG/JPEG blobs
listed in the manifest and referenced from title records by digest. An ordinary
push, pull or archive carries them, with no dependency on a registry's Referrers
API. The library and the registry browser fetch them lazily, without the disks;
missing optional artwork never blocks Run. Cover edits follow the local-overlay
and explicit-publication rule above; publishing gives a new image digest for
the same machine digest. Artwork and metadata follow the same private/public
blob policy as other content.
OCI referrers remain a possible later extension; version 1 does not use them.

Genre, Platform, Year, Developer, Publisher and Series become Explorer tree
facets. Index them per title in the local summary cache, together with a compact
search index covering **all text fields**, including description, notes and tags.
Match facets on the same title record, then group matching titles under their
repository/tag so a multi-title image remains one stored machine. The cache uses
the metadata digest plus local overlay/personal-field revisions for invalidation
and retains lazy bounded rebuilds (§7).
Optional eXoDOS catalogue and LaunchBox XML import is future work only.

## 3. Content-addressed store, tags and local workspaces

The local store follows the role of `/var/lib/docker`: one application-owned
root shared by the unemu family, selectable in Preferences. Defaults are:

- macOS: `~/Library/Application Support/unemu/`.
- Linux per-user: `$XDG_DATA_HOME/unemu/`, falling back to
  `~/.local/share/unemu/` when `XDG_DATA_HOME` is unset or empty.
- Linux optional future system service: `/var/lib/unemu/`.

The root must be on a local filesystem with working advisory locks, atomic
rename and `fsync`. The library probes locking when it opens a store and refuses
a root where it fails. A successful probe never permits a network filesystem:
the local store is always local. The registry may live anywhere and manages its
own storage, including an NFS backend (§5.1).

Inside that root, `images/` follows the OCI image-spec, Image Layout. Writable
workspaces and local metadata live beside the OCI layout, outside its
content-addressed area:

```text
<store>/
  store.toml                      store layout version; a newer one is refused
  images/
    oci-layout                    {"imageLayoutVersion":"1.0.0"}
    index.json                    OCI index: one manifest descriptor per local tag
    blobs/sha256/<hex>            immutable bytes; full 64-character digest
  workspaces/play-001/            mutable host state, never image content
    session.toml                  pinned machine and image digests, heads, policies
    workspace.lock                exclusive owner for runs and offline mutations
    media/system.upfj             current single-writer journal
    media/cmos.upfj               independent NVRAM journal
    transaction.toml              recoverable publication intent
  cache/materialized/<hex>        flattened chain heads (§4), and decoded payloads only
                                  for mounted file-backed media; rebuildable
  cache/summaries/                library summaries and search index; rebuildable
  local/blob-index.toml           build-time source hints, redistribution class, licence
  local/library.toml              metadata overlays, personal fields, last run, favourites
  local/pulls/<hex>.toml          unfinished pull or import: source and manifest digest
  locks/store.lock                shared operation leases; exclusive for GC
  locks/index.lock                short exclusive tag/index updates
  staging/                        temporary files; not GC roots
  quarantine/                     blobs that failed verification
```

Imported bytes are copied into the store through the publication sequence in
§3.1; never link a mutable user file into it or require an external descriptor
at run time. User source files remain unchanged. Base verification covers
**every byte, including large hard disks**; preserve mounted descriptor identity
and mutation checks from persistent media. For now, **every Start performs full
SHA-256 verification**, including every multi-gigabyte base and any materialized
chain head; a filename/digest cache cannot replace it.

A **planned change after phase C measurements** is verify-at-ingest plus an
explicit **Check store** command, replacing repeated full hashing at Start.
Ingest and Check store will still hash every byte, including large hard disks;
this optimization is not active now and does not change mutation checks.

Tags are mutable references in `index.json`, using
`org.opencontainers.image.ref.name` on manifest descriptors, for example
`dos-lab:1.0`. A local reference is `repository:tag` without a registry host;
which registry a repository is pushed to is host configuration (§5.1). Digests
are immutable identities. Resolve a tag once at launch and pin the result for
the session. Unpublished local tag replacement uses compare-and-swap under the
index lock (§3.1). References tracking pushed version tags retain their published digest;
metadata/artwork edits use the library overlay until publication under a new
version or tag. Moving remote aliases follow the force policy in §5.1.
Branching creates a new tag/workspace without editing the parent.
Workspace/media ids use `[a-z0-9][a-z0-9._-]{0,63}`; reject path separators and
duplicates. Tags are index metadata, never host paths; validated SHA-256 digests
alone determine blob paths. Explicit bundle archives preserve this OCI layout.

Deduplication is exact object reuse: identical raw bases and identical sector
packs occupy one object each, also across tags/bases. Sector packs can be reused
across different parents, while layer descriptors remain parent-bound. No claim
of deduplication for similar but nonidentical whole bases; chunking is deferred.
Library size is the distinct reachable byte total, excluding working journals;
savings compare it with the sum of each image's reachable size counted separately.
Show exclusive bytes and estimated reclaim separately from virtual disk capacity.

Deleting a tag removes a reference, not shared objects, and never a workspace.
GC roots are local tags, workspaces (their pinned image and current heads),
unfinished pulls, local metadata/artwork overlays, open transactions and
archives being written. A workspace
whose tag was deleted or moved keeps its pinned image alive and is shown as
detached. A materialized base is kept while a workspace uses it. Config references,
including a media-set catalogue source's medium, name decoded digests; a compressed
payload is stored under another digest, so GC marks through manifest descriptors.
GC is always
an explicit action; it marks, rechecks roots, then sweeps, under the exclusive
store lock.

### 3.1. Concurrent access and failure handling without a daemon

The GUI and CLI use the store library directly, including when several processes
share a store. Immutable blobs are published from unique temporary files on the
same filesystem: write, verify the complete digest/length, `fsync` the file,
rename atomically without replacing an existing blob, then `fsync` the directory.
If another writer has published the digest first, verify that blob and discard
the temporary file. Readers see complete immutable bytes; they never open staging
files. Downloads and hashing run outside the short index lock.

Tag/index changes take an advisory exclusive lock on `locks/index.lock`, reread
the current index, and compare-and-swap the target tag against its expected old
digest (or expected absence). A mismatch returns a conflict, not a lost update.
Publish the replacement index via temporary file, `fsync`, rename and directory
`fsync`, preserving unrelated tags. Updates to `local/` files use the same short
lock to avoid overwriting another process's changes.

Each writable workspace has a stable `workspace.lock` file. The process running
it holds an exclusive advisory lock for its entire run and journal finish; a
second window or CLI run must choose another workspace or wait. Mutating stopped
workspace tools take the same lock. The library also rejects a second owner
within one process, so multiple GUI windows cannot bypass workspace ownership.
Every store operation, including pull and push, first acquires a shared lease on
`locks/store.lock`; a workspace owner retains that lease until releasing its
workspace lock. GC takes the store lock exclusively, so it cannot run while any
run or transfer is active, and nothing new starts during GC. Lock order is store
lease, workspace lock if needed, then index lock; never wait for a workspace
while holding the index lock. Release shared leases before requesting exclusive
GC ownership.

Operating-system advisory-lock liveness determines ownership: the kernel
releases locks when the owning process exits. A leftover lock file does not mean
a live owner, and a PID alone is not evidence that a lock is stale. Never unlink
a live lock file or replace its inode to bypass a lock.

| Event | Behaviour |
|---|---|
| Pull or import interrupted | No tag is written. Verified blobs stay, rooted by the `local/pulls` record; the library lists the image as incomplete with Resume and Remove. Resume fetches only missing blobs and continues a partial blob by HTTP range where the registry allows. |
| Push interrupted | The remote tag is written last, so the registry holds the old tag or the complete new image, plus unreferenced blobs for its own GC. A retry uploads only what is missing. |
| Disk full | Check free space against the known descriptor sizes before pull, materialize, seal and export. A failed write removes its temporary file and publishes nothing. During a run a full disk is a journal host error: execution stops with the persistence error ([Persistent media](persistent-media.md), §Host errors and determinism). |
| Lock holder crashes | The next opener takes the stable lock first, then runs journal tail recovery and completes or rolls back `transaction.toml`, verifying staged data. GC removes `staging/` files, which no live lease can own while it holds the store lock. |
| Blob fails verification | Move it to `quarantine/`, mark every image and workspace that needs it as damaged and block their Start. Repair fetches the digest again from a registry or re-imports the source. Nothing is deleted automatically and no substitute is accepted. |
| Local tag race | Compare-and-swap conflict; the loser learns the current digest. Both manifests exist, so nothing is lost. |
| Remote tag race | OCI has no tag compare-and-swap; see §5.1. |
| Newer store or workspace schema | Refuse to write; never rewrite a format this build does not know. |

The local store, PC import, immutable layers, workspaces and offline publication
are implemented, with synthetic concurrency, interruption and process-exit tests.
Registry transport and image-store GUI integration remain prospective.

## 4. Disk chains and writable sessions

A raw base is immutable bytes with full SHA-256 and geometry. A **zero base** is
the PC provider's blank fixed disk: a canonical TOML descriptor of a few hundred
bytes (`schema`, `kind = "zero-base"`, `capacity_sectors`, `geometry`) whose
content is all zeros. Its decoded identity, the SHA-256 of that many zero bytes,
is never stored: it is computed deterministically from the length by streaming a
fixed zero buffer (and remembered per length), so no allocation or file grows
with the disk. It costs the descriptor only, in the store, in a registry and on
the host, on every filesystem; no sparse files or filesystem features are used.
It is open content, it mounts only through a journal policy, and it is never
compressed or materialized because it has no bytes. A layer over it names the
descriptor digest as `parent` and `base`. A sealed layer is
an immutable TOML descriptor: `schema`, `kind = "disk-layer"`, `parent` (raw base,
zero base or sealed layer digest), `base` (root digest: the raw or zero base), `geometry`, `sectors` (sector
pack digest), `sector_count` (number of replacements), and `result_sha256`
(digest of the complete reconstructed disk) and `capacity_sectors`, so capacity
never depends on a CHS translation. All sizes must agree with the root.
Layers apply to fixed disks only: a changed floppy or CMOS freezes to a new
complete raw blob. There are no sector layers for floppies.
A sector pack is ascending unique records of `u64le LBA + 512 data bytes`, no
header/padding; its exact length is `sector_count * 520`, bounded by capacity.
LBA bounds, digest and length are checked before use. Empty packs are allowed.
A future codec change needs a new schema, not a reinterpretation of these bytes.
Sealing emits only final sectors differing from the exact parent; command history
and recovery generation markers stay in the local journal, outside the layer.

Read order is writable sparse overlay -> newest sealed layer -> earlier layers
-> raw base, through **one `SectorMedia` adapter**, which merges the chain into
one LBA index at mount so read cost does not grow with depth. Controllers never
see a chain or host path. Resolve and verify parents before executing; reject cycles, missing
objects and wrong parents/geometry. `result_sha256` is verified by streaming the
complete reconstructed disk before mount or publication, independently of object
hash checks. Hashing is host preparation; it cannot grant guest instructions.

A layer binds to exactly one parent. Sibling layers cannot be merged by union:
filesystem allocation tables and other shared metadata may conflict. Installation
layers must be built serially on the chosen parent. Hardware/config changes make
a new complete machine manifest, even if all disk heads remain unchanged.

A workspace pins every media head and holds mutable journals separately. Current
version-1 journals bind a raw base's length/hash, not a chain: **do not relabel a
v1 header to mean a layer hash**. Initially materialize the chain head once as a
verified immutable raw file, `cache/materialized/<result_sha256>`, and use the
current v1 journal unchanged on top of it. Every workspace on that head,
including each disposable gaming-mode workspace, shares the one file, so the
materialization cost is one flat copy per distinct head, not per workspace or
per run. Full SHA-256 verification still runs at every Start (§3), including
for this shared cache file. A head that is itself a raw base needs no copy.
A head rooted in a zero base is never materialized: the machine reads the chain
straight through the overlay interface (unwritten sectors read as zeros), the
v1 journal binds the decoded length and SHA-256, and Start streams a layered
head once to check its `result_sha256`. Saving seals only the written sectors;
flattening such a chain seals one layer over the zero base with the sectors that
differ from zero, and a disk that is all zeros again flattens to the zero base. A later
versioned journal binding may name a chain head plus its full logical
digest/capacity; its scanner retains command/recovery guarantees. Both paths
use the same overlay interface and always leave the old base intact.

| Action | Proposed behavior |
|---|---|
| Start | Resume a selected local workspace, or create one from the pinned image. Readonly media reject writes; journal media get explicit private paths. |
| Gaming mode | Create an isolated disposable workspace for every writable disk/catalogue medium and CMOS; never reuse the regular journals. On exit ask keep as a new image or discard. No automatic discard after a crash. |
| Save as layer… (commit layer) | Stop the machine, finish all active media, seal each changed fixed-disk overlay into a parent-bound layer, freeze each changed floppy or CMOS to a new raw blob and publish a complete new manifest. Media without changes keep their head. The user names a new version/tag or moves an eligible unpublished local tag by compare-and-swap; pushed version tags remain immutable. Parents remain unchanged; the workspace then pins the new image and starts fresh journals. |
| Discard changes | Stop and release writers, then remove only this workspace's changes after showing affected media. The pinned image still exists. |
| New image from here… | Select a complete historical manifest and fork a workspace at all of its heads; picking only one disk is an explicit advanced operation with a mismatch warning. |
| Export new base | Stream the selected chain into a new raw file, verify it, and publish without replacing any original. This is the existing media-tool meaning of commit/compact. |

The Disk layers dialog labels commit layer **Save as layer…**, distinct from
**export new base**. An eject still closes a recovery generation; it does not
silently make a disk layer.
Preserve retention warnings: empty closes currently consume a generation too.

There is no atomic multi-medium checkpoint today. Initial image publication is
**offline only**: stop guest execution, finish and release all writers including
parked media, then freeze every head. Stage objects and an intent recording old
and proposed manifest digests, publish all objects, then atomically replace the
single tag pointer as the image's publication point. Updating the workspace pointer
and retiring its old journals is a separate idempotent transaction step; both
roots and the old journals remain until it is durable. Failure leaves an old or complete new image plus recoverable journals; startup
offers retry/inspect, never a silently mixed machine. Partial published objects
are harmless unreferenced data. This is a proposed transaction requiring crash
injection tests, not a measured power-loss guarantee or a guest-consistent save.

CMOS uses the existing 64-byte base/journal rules, including excluded transient
and clock fields. The 512-byte disk pack is **not** used for CMOS: freeze to a new
64-byte raw object. Derived/blank NVRAM remains config state. Saved states, when
supported, pin exact config/firmware/media identities, inserted selections,
platform, emulator/device snapshot versions and CPU-core revision (x86jit for
the PC provider). Hash this compatibility tuple without the enclosing
manifest/state pointer to avoid self-reference; incompatibility offers a cold
boot without the state, never an approximate restore. Saving media alone is not a snapshot.

## 5. Self-contained images and blob policy

A private image includes **everything needed to run**: firmware (BIOS, video
BIOS and option ROMs on PC), platform and resolved machine configuration, every
disk base/layer and sector pack, complete media catalogues and their media,
platform NVRAM and any selected saved state. The user's private store and private
registry may hold proprietary bytes. The repository still never contains
proprietary blobs, derived proprietary sectors, private
installation names, source paths, screen captures or credentials.

**Locate firmware** is a build-time action when creating or importing an image
from user ROM folders. Search is local and opt-in, with progress/cancel; hash
complete candidate files and match digest plus length, not filenames. Split
firmware is matched by individual chip/container identity, never guessed by
assembled size. Copy verified inputs into the store before completing the image.
Store source-folder hints only in the local index. Running a complete image
never searches those folders or depends on their continued existence.

Missing artwork never blocks Run: resolution reports missing artwork, while tag publication still requires it (§2.1, §3.1).

A tagged image is always complete (§3.1). A blob that later goes missing or
fails verification is store damage: the library reports its role and digest,
blocks Run and offers repair from a registry or by re-import. It never falls
back to searching user folders. A damaged saved-state blob permits an explicit
cold boot. New content
is classified user-supplied by default. Restricted inputs propagate that
classification to derived content unless separately reviewed; a layer containing
copied proprietary sectors stays restricted even if its parent is omitted.

Private push/pull and private OCI archive export preserve all blobs. Any future
**public export** instead traverses the complete graph and strips every non-open
blob, including derived layers and optional state. It records omitted digest,
length, type and binding as user-supplied references in a separate export recipe;
these references are not OCI payload descriptors claiming included bytes. Open
blobs require reviewed SPDX licence/provenance evidence. Public metadata must
itself be publish-safe. Preview omissions and explain that the public package is
an incomplete recipe: the recipient supplies inputs at image-build/import time
to create a complete private image. Never silently publish the private manifest
or turn ordinary private pull into a runtime folder-matching workflow. Mutable
journals, local indexes, source paths and credentials are never image payloads.
Hashes identify content, not ownership, licences or trusted publishers.

### 5.1. Private registry and optional daemon

Push and pull use the
[OCI distribution-spec](https://github.com/opencontainers/distribution-spec),
Pull and Push sections, against a configured registry; images with
user-supplied content go only to a **private registry**. The registry may run
anywhere: for example, on a Proxmox host with NFS storage behind the registry. This is separate from the always-local client store (§3):
the registry manages its own blob storage and locking. Chosen registry:
[zot](https://github.com/project-zot/zot), a small OCI-native registry with OCI 1.1
`artifactType` and referrers support. Harbor and distribution/registry are
alternatives; compatible Forgejo package registries remain usable too.

Tags work like Docker, for example `dos-lab:1.0.0`. Pull resolves a tag to a
manifest digest once, copies the complete descriptor closure, verifies every
size and SHA-256 and only then writes the local tag (§3.1). Push checks image
completeness and tag policy, uploads missing blobs by digest, then publishes
the manifest and tag last. The transport implementation must:

- Use `POST`/`PATCH`/`PUT` upload sessions with chunks of about 64 MiB and an
  8 MiB floor. Persist the session `Location` after each accepted chunk. On
  resume, query the server offset rather than trusting local progress; a `404`
  restarts the upload, as does a refused offset. Downloads resume by range: an
  interrupted blob continues with `Range: bytes=<have>-` into the same staged
  file and hash (up to three times); a `200` answer restarts the blob.
- Check blob presence with `HEAD` and upload only missing bytes. A
  cross-repository mount, falling back to uploading the same verified bytes, is
  available in the client library; the CLI does not try it yet.
- `HEAD` every referenced blob before the manifest `PUT`: zot does not verify
  blob existence for manifests with custom config media types.
- Check the target tag digest before the manifest `PUT`: skip the same digest
  and refuse a different digest for version tags. Moving aliases follow the
  explicit-force policy below.
- Write deterministic manifest bytes without a created timestamp, including
  `org.opencontainers.image.created`.
- Always pass `n` when listing repositories or tags and follow pagination
  `Link` headers.

**`+zstd` support is required before any phase C multi-gigabyte image
is pushed**, and its multi-gigabyte bases must be compressed; raw multi-gigabyte
bases are not a temporary phase C upload path. Metadata remains uncompressed.
The verification spike uses synthetic data and does not waive this gate.
Identical blobs deduplicate by digest locally. Physical
registry deduplication is implementation-dependent, and the registry never
interprets a filesystem. No registry service is bundled or deployed by this
proposal.

Registries follow
[decision-25](<../../decisions/decision-25 - Machine images are self-contained OCI artifacts in a local store.md>)
and its amendments. The rules guard against
publishing proprietary bytes by accident:

- `registry.unemu.dev` is the default registry, with Docker-style name
  resolution: `name:tag` is `registry.unemu.dev/unemu/name:tag`, `owner/name:tag`
  keeps its owner, and a first component with a dot, a colon or `localhost`
  names the host. Preferences, `--registry` and an environment variable override
  the default host. Local tags keep the short form.
- The registries list in preferences holds entries of a host, an optional
  `private` flag and an optional login user, never a secret. The default
  registry is known without an entry and is public until marked private.
- Push refuses a registry that is neither listed nor the default. A blob's
  effective class is the most restrictive one recorded for its digest in any
  local manifest (§2). An image with any user-supplied blob (the default
  classification) goes only to an entry marked private and never into the
  official `unemu/` namespace; pushes into `unemu/` require every blob to be
  open. Before such a restricted push, probe anonymous `tags/list` and catalog
  access for the target namespace, not just `/v2/`: `403`, or `401` with a Basic
  challenge, indicate private access; `200` or `404 NAME_UNKNOWN` indicate open
  access, and a `401` Bearer challenge is inconclusive; both refuse the push.
  There is no override.
- **TLS is mandatory** except for a loopback test registry (`plain_http`).
  Requests start anonymous and send credentials only after a `401` Basic
  challenge, so an anonymous push is attempted only where the registry accepts
  it, and the guard still applies. Credentials go only to the registry's own
  scheme, host and port, never through a redirect elsewhere. The OCI
  distribution-spec itself mandates neither TLS nor authentication; these are
  client policy.
- Authentication and access policy belong to the registry. Every registry has
  its own login. Secrets come from the operating system's credential store, or
  from environment variables bound to one host for CI, and never appear in
  images, the store, preferences, logs or diagnostics. The store backend is
  chosen at run time per operating system (the macOS Keychain now; Secret
  Service and Windows Credential Manager later). Without one, the desktop
  Preferences › Registries page keeps a login for the session only and `image
  login` refuses, pointing to the environment variables. A login is checked
  against the registry before it is kept. The CLI `image login` and the
  desktop page share one accounts implementation and one preferences lock, so
  with a system store a login in one is visible in the other. Secrets are not
  wiped from process memory after use; that is out of scope for now.
- **Version tags are immutable.** If a tag of semantic-version shape already
  names another image digest, refuse the push even with force; retrying the same
  digest is idempotent. Any other tag moves only after an explicit confirmation
  showing the old and the new digest. OCI has no tag compare-and-swap:
  preflight-read the current digest, but do not claim this closes a remote race.
  Enable registry-side version-tag immutability where available; a client
  preflight alone cannot enforce it against concurrent writers.
- Pull never rewrites a local tag that names another digest; the image is then
  pulled under another local tag. Workspaces are unaffected, because they pin
  digests.
- Push and pull can be cancelled. The transfer checks before each blob,
  between the chunks of a large upload and before it publishes. A cancelled
  push never writes the manifest or moves the tag, and its accepted chunks stay
  in their persisted session. A cancelled pull never writes the local tag, and
  its verified blobs stay in the store. A confirmed tag move is bound to the
  digest the user was shown: if the tag names another digest when the push
  starts, the confirmation is asked again.
- The desktop shell's Image › Push… and Image › Pull… follow the approved
  registry-accounts mock-up. Push… shows the dry run of the local image (every
  blob with its effective class and where it was recorded: this image, another
  local tag or the firmware catalogue) and the guard's verdict for the
  destination as it is typed, with no override. It pushes only the image the
  user reviewed. Both dialogs run on the library worker, never on the frame
  loop, and share the command line's push plan, client and providers list.

**No daemon is needed now**, for either a purely local library or registry
push/pull. GUI and CLI call the same in-process store library directly. Its API
owns image import/validation, store transactions, registry transfer, workspaces,
locks, progress/cancellation and structured errors; provider interfaces supply
config validation and machine lifecycle without emulator dependencies in the
shared crates. A registry is optional; keeping every image locally is a complete
supported workflow.

Revisit an optional `unemu-daemon` only when machines must run headless on a
server with remote viewing, remote control is wanted, or background GC is needed.
If justified then, it is a thin local-socket/service wrapper around the same
transport-neutral library API, with no duplicated storage or validation rules.
It is not a planned dependency of the image store, library UI or registry client.

#### Registry verification (2026-11-14)

The synthetic spike with zot v2.1.23 and oras v1.3.6 passed: byte-identical
round-trip of custom artifact/config/six layer media types and annotations,
a 3.5 GiB blob and digest-verified pull, catalog/tag paging, native referrers,
TLS with htpasswd authentication (anonymous `401`), detection of anonymous
access on a misconfigured public registry, and cross-repository mounting of the
large blob. **zot is chosen.** distribution/registry and Harbor are untested.

Weak spots: oras single-`PUT` uploads restart from zero; a client death during
`PATCH` makes zot discard the whole session, including earlier chunks. A clean
stop between chunks resumes at the server offset, including after zot restarts.
zot accepted a custom-config manifest naming a missing layer and has no default
size limit or timeout. oras adds a nondeterministic created annotation; tag
immutability rules are per repository pattern and user. The transport rules
above account for these findings; this spike does not waive the compression gate.
The spike's mount used oras; the unemu client offers mounts in its library API
only, and its CLI does not try them yet.

### 5.2. Installation media sets

[decision-26](<../../decisions/decision-26 - Installation media sets are their own OCI artifact type.md>)
accepts a second artifact beside the whole-machine image. The
[installation-media mock-up](mockups/install-media.html) follows the approved
library/window styling; the maintainer approved the section and mock-up on
2026-11-14. The extras amendment below records the accepted shared contract.
A set is portable installation input, with no hardware, firmware, NVRAM, saved
state or workspace journals of its own.

**Artifact and config.** Use the §2 OCI manifest envelope with
`artifactType = "application/vnd.unemu.media-set.v1"` and config media type
`application/vnd.unemu.media-set.config.v1+toml`. The neutral config envelope has
required `schema = 1`, `platform` and a nonempty ordered `media` array. Each entry
has a unique stable `id` (the §3 media-id grammar), a nonempty editable `label`,
a `blob` digest and a provider-declared `format`. Array position is authoritative;
filenames, labels and manifest layer order never infer disk order. This sketch
uses the PC provider's format vocabulary; shared code never owns CHS rules:

```toml
schema = 1
platform = "ibm-pc-at"

[[media]]
id = "disk-1"
label = "Disk 1 of 4 - Setup"
blob = "<raw-floppy>"
format = { kind = "floppy", geometry = { cylinders = 80, heads = 2, sectors = 18, sector_bytes = 512 } }

# Further entries retain the user's confirmed order.
# An optical entry uses format = { kind = "optical", encoding = "iso-2048" }.
```

The provider validates formats, exact byte lengths and drive compatibility.
Floppies use `application/vnd.unemu.media.floppy.v1`; reserved ISO images use
`application/vnd.unemu.media.optical.v1` (2048-byte sectors). The remaining layers
are optional extras (below), exactly one `application/vnd.unemu.metadata.v1+json`
blob and its PNG/JPEG artwork, using the **same §2.1 schema**, not a parallel
metadata format. The
metadata envelope's platform and `unemu.platform` annotation must match config.
Title records supply title, publishers, release year, cover and other §2.1 fields;
a set has no Play action and metadata launch fields do not execute on insertion.
Repeated identical disks may occupy several ordered entries but have one layer
descriptor. Emit unique medium descriptors in first-occurrence config order,
then extras, metadata and artwork. Reject missing/unreferenced descriptors,
invalid config fields/versions, duplicate ids and malformed digests under §2 bounds;
newer metadata keeps §2.1's read-only degraded display behavior.

**Extras (shared with firmware packages).** The accepted decision-26 extras
amendment adds an optional ordered `extras` array to config. Each entry has a
unique `id` (the §3 grammar), original basename `file_name`, exact stored `size`,
`blob` (full SHA-256 descriptor digest), `kind` (`document`, `archive`,
`executable` or `other`) and optional `dos_name` (DOS 8.3). For example:

```toml
[[extras]]
id = "manual"
file_name = "Technical reference.txt"
size = 4096
blob = "<reference-document>"
kind = "document"
dos_name = "TECHREF.TXT"
```

Extras are opaque payload layers, using the descriptor's content media type or
`application/octet-stream`; list unique extras after media, before metadata and
artwork. Verify digest and length, reject path components in names and duplicate
ids, and apply §5 rights to each extra independently. A ROM's open licence never
makes an accompanying manual open. **Open** on a document explicitly extracts a
verified copy to host temporary storage and invokes the host document handler;
archives and executables are not automatically opened or run. Export preserves
original names without allowing traversal or overwriting existing files.

Chosen extras can later be packed into a generated deterministic FAT transfer
medium: a 1.44M floppy when they fit, otherwise a FAT16 disk image. Validate or
assign collision-free 8.3 names in config order, with fixed timestamps and stable
allocation; pin the generated bytes for replay. The generated base is immutable,
fully SHA-256 verified, and inserted through the ordinary overlay interface;
writes stay in a separate workspace journal and export creates a new file.
There is no guest helper or host folder sharing. An ISO variant waits for CD-ROM
emulation. Codec/CLI import carry extras from the start; the transfer generator
and UI follow separately, with approved mock-ups before GUI implementation.

**Identity and rights.** A blob's identity is SHA-256 of its exact stored bytes;
the config digest identifies the ordered media, labels, formats, extras and platform.
The **set digest** is its OCI manifest digest, used by tags, transfers and source
provenance. Metadata/artwork edits change the set digest while retaining config
and medium digests; order, label or format edits change config too. A set is not
a machine and has no machine digest. Stable versioned serialization, LF, stable
key/descriptor order and no timestamps give byte-identical output for identical
inputs. Verify every descriptor's full SHA-256 and length before use, including
large optical blobs; compression uses §2's stored/decoded verification contract.
Import and hashing never advance guest time. An insertion replay pins the medium
bytes, format, selection, workspace state and event at a retired-instruction
boundary, never a mutable tag or host timing.

All descriptors default to `unemu.redistribution = "user-supplied"`, including
metadata/artwork, unless reviewed licence/provenance proves them open (§5).
The most restrictive local classification for a digest still wins. Restricted
sets stay in the private store/registry and private archives, never repository
fixtures or public exports. §5's future public recipe strips restricted bytes;
it never silently exports the original private manifest.

**Read-only bases, writable installations.** Media > Insert > From library…
resolves a set tag once, verifies it and offers loading the whole set into the
selected drive's catalogue **in config order** (default), initially inserting
the chosen entry. With that option cleared, add only the chosen entry. Validate
all entries being added against the selected provider/drive before changing the
catalogue: PC floppy geometry must agree exactly, and a mixed/incompatible set
cannot be partially loaded silently. Cancel or validation failure leaves the
catalogue and current insertion unchanged. Existing next/previous-disk actions
then traverse the catalogue. Add to machine… performs the same validation on a
stopped machine draft and publishes only after explicit confirmation.

The set and its stored medium blobs are always read-only. The receiving
workspace may explicitly select journal mode for installer writes; those go to
separate per-workspace, per-medium journals under the ownership/recovery rules
of §§3.1/4, even for parked catalogue disks. A write-protected selection rejects
writes rather than silently switching modes. Each workspace pins imported set
and medium digests as GC roots, so tag movement/deletion cannot change a mounted
base. Reads use the same `SectorMedia` overlay interface as §4; no controller
opens blobs or bypasses the overlay. Full base hashes remain mandatory. Saving
changed floppies freezes new complete raw blobs and a new machine image, never
modifies the set; export/commit writes a new base file and leaves the old base
intact. A later hash-linked chain can replace the base beneath that interface;
this proposal adds no floppy sector-layer format.

**Embedding and transfer.** A machine image may copy a set's ordered bindings
into a drive catalogue. Its config remains the authority for order, inserted
entry, format and write policy (§2); all media bytes enter the machine's own
closure. Identical medium digests reuse the same stored blobs across sets,
machines and tags. Record source manifest digests in the machine manifest's
`unemu.media-set.sources` annotation, a string containing a JSON array of unique
set digests in first-use order. This is provenance only: loading the machine
never fetches a source set or needs its tag. It pins the embedded media, not a
mutable external reference. §5.1 push/pull, private-registry rules, version-tag
policy, upload resumption, complete-closure verification and tag-last publication
apply unchanged to sets. Private OCI archives carry the same verified closure.

**Import and library.** Import installation media… accepts a folder or an
explicit file list. A folder preview enumerates candidate image files in stable
relative-filename order; a file-list preview preserves the supplied order. No
filesystem enumeration or picker ordering is accepted without a visible preview.
The user reorders entries and edits labels before publication; basenames provide
initial labels, never authoritative sequence numbers. Detect standard floppy
geometry from exact byte size through the provider. Offer an explicit per-file
format override, still requiring byte-length agreement; unknown/ambiguous sizes
require a choice, never silent truncation, padding or resizing. Optical detection
proposes ISO-2048, with provider validation before import. Copy and fully hash
sources using §3.1, leaving source files unchanged; host paths remain local hints
and never enter config, metadata or annotations. The metadata subset is title,
software platform, publisher, year, genre, description, tags and optional cover;
platform routing is selected separately. Publishing uses the §2.1 codec and
§3.1 atomic complete-tag contract.

The Explorer tree adds **Installation media** alongside machine collections.
Reuse §2.1's Genre, Platform, Year, Developer, Publisher and Series facets and
same-title matching, with media count/formats and insertion availability in set
rows. Summaries include artifact type to distinguish sets from machines and are
keyed by manifest/metadata digests plus overlay/personal revisions. Extend §7's
versioned summary cache, lazy bounded rebuilds and incremental all-text search;
warm startup/sorting never opens media payloads. Missing or damaged media block
insertion with repair actions; incomplete imports retain Resume/Remove (§3.1).
Library metadata edits stay local overlays until explicit new-version
publication, and personal notes/ratings never travel (§2.1).

The format permits storing, transferring, indexing and inspecting optical sets;
this reserves no emulated controller. Their rows show **CD-ROM emulation not
available yet** and disable Insert/Add to machine… while keeping details and
private Export available. Import validation for a known storage format is
separate from insertion capability. No CD-ROM emulation is part of this step.

**Approved:** the maintainer approved this section and the three mock-up views
(library/details, From library… and Import installation media…) on 2026-11-14.
The codec, CLI and From library dialog are implemented. The library collection
and Import installation media… followed on 2026-11-15 from the extras mock-up
(revision 2): rows, facets and the set pane read summaries only, which carry the
set's ordered media and extras; Insert hands the selected set to the running
drive's From library… view, which validates and loads it as above; Add to
machine…, Make transfer media… and Export stay disabled until their work lands.
The pane's Insert is reachable only once the library can be shown beside a
running machine; the shell draws the library only while no machine runs, so
today Media › Insert › From library… is the path into a drive.
The dialog and `image media import` share one plan-and-publish function and its
refusals. When the tag exists, the window writes nothing: it resolves the stored
set, refuses at once on another artifact type or a different layout, metadata
or title, then compares the sources' raw SHA-256 with the stored decoded
digests, so a set stored compressed or raw reports already imported and any
other content a tag conflict (the command line keeps refusing an existing tag
before reading). The transfer-medium generator remains separate work.

### 5.3. Firmware packages

Status: **codec/import/store-first resolution, the library Firmware collection, and per-layer licences, source archives, descriptive metadata, the generated README and metadata revisions (CLI, store, push guard and GUI) implemented**, 2026-11-15. The approved [Firmware collection mock-up](mockups/firmware.html) (revision 3) defines the UI; Use in machine…, Export, Open for documents, incomplete-import records, the Labels facet and GUI explicit additions of unmatched files and documents are still pending (the CLI adds one unlisted ROM with `--unlisted`).
[decision-27](<../../decisions/decision-27 - Firmware packages are their own OCI artifact type.md>)
accepts the artifact shape; codec, import and explicit store-first resolution
are implemented, and so is the library's Firmware collection with Import ROM
folder… (see **Library** below). A package holds reusable ROM inputs and their fit
information, not a machine, writable media, NVRAM or a saved state.

**Artifact and config.** Use the §2 OCI manifest envelope with
`artifactType = "application/vnd.unemu.firmware.v1"` and config media type
`application/vnd.unemu.firmware.config.v1+toml`. The same firmware string already
names raw ROM payloads in §2: a manifest's `artifactType` denotes the package,
whereas a layer descriptor's `mediaType` denotes ROM bytes; a ROM is never parsed
as a manifest. Config requires `schema = 1`, `platform`, nonempty `name` and a
nonempty ordered `roms` array. Optional `vendor`, `version`, `date` (ISO calendar
date) and `notes` describe the package; absent means unknown. `fits` carries
provider-owned board and machine-class ids. Catalogue ids belong to individual
logical ROMs, and the package summary indexes their unique union. This sketch
uses the existing IBM 5170 BIOS catalogue entry and the chip pins from
`machines/at-games.toml`; it contains identities only, never ROM bytes:

```toml
schema = 1
platform = "ibm-pc-at"
name = "IBM 5170 BIOS 1985-06-10 (2 chips)"
vendor = "IBM"
date = "1985-06-10"
notes = "U27 even / U47 odd; user-supplied ROMs in a private store."

[fits]
boards = ["ibm-5170"]
machine_classes = ["at"]

[[roms]]
id = "system-bios"
role = "system-bios"
catalogue_id = "ibm-5170-bios-1985-06-10"
sha256 = "6fb653b58e678cb69b0f943a105c6c803b18a6db23d13c0abccec5b66c6ee652"
size = 65536

[[roms.parts]]
position = "U27"
part_number = "6480090"
blob = "sha256:c8654ed7a922644db2b2cce54c540fcfa7cbbae8c514d7eb3dbfbb8187471bfd"
sha256 = "c8654ed7a922644db2b2cce54c540fcfa7cbbae8c514d7eb3dbfbb8187471bfd"
size = 32768
lane = "even"
offset = 0

[[roms.parts]]
position = "U47"
part_number = "6480091"
blob = "sha256:470ab30f9eb7d56c24f5ce10c7d307c539b9f923ea97dd536d2d5ba7e36d471b"
sha256 = "470ab30f9eb7d56c24f5ce10c7d307c539b9f923ea97dd536d2d5ba7e36d471b"
size = 32768
lane = "odd"
offset = 0
```

The single-image **IBM VGA BIOS 1987 (32 KiB)** package uses catalogue id
`ibm-vga-bios-1987-32k`, role `video-bios`, size 32768 and full SHA-256
`51b49761f72780a696c36f3d2e74a8168c1c99b865070f7f90b8e621a8b5ae17`.
It fits `ibm-5170`, class `at`, and remains user-supplied. As in
`machines/at-games.toml`, the option-ROM binding maps at `0xC0000` after skipping
`0x2000` container bytes; the stored/hash-identified input stays the full 32 KiB.
The mock-up's incomplete set uses `ibm-5170-bios-1985-11-15`, with U27 part
`61X9266` present and U47 part `61X9265` missing. These real catalogue names and
hashes may be documented publicly; the ROM bytes remain private. Documents such
as **IBM PC/AT Technical Reference** and **BIOS source listing (Technical Reference
appendix)** illustrate user-supplied extras, without including their contents.

Each logical ROM has a unique §3-grammar `id`, a provider-declared `role`, optional
`catalogue_id`, full lowercase `sha256` and exact decoded `size`. It has either
a single `blob` with optional physical `position`/`part_number`, or a nonempty
`parts` array, never both. PC roles `system-bios`, `option-rom` and `video-bios`
map to `FirmwareKind::{SystemBios, OptionRom, VideoBios}` in
`crates/assembly/src/firmware.rs`. `keyboard-controller` and `other` reserve roles
for storage and display, not executable support. Unknown roles need a provider
schema extension; shared code never guesses ROM mapping from their names.

**One BIOS, several chips.** `parts` mirrors machine-file `RomPart` in
`crates/machine-file/src/schema/v1.rs`: `blob` supplies `image` bytes, with the
same `lane`, assembled-byte `offset` (default zero) and per-chip `sha256` pin.
Every part also has exact `size` and optional socket `position` and `part_number`.
Its role and catalogue id are inherited from the logical ROM. `KnownPart` has
socket/part labels, hashes, sizes, lanes and offsets, not independent catalogue
ids; do not invent one id per socket. A catalogue ROM's top-level hash/size pin
the **assembled** image, as machine-file `Rom.sha256` does for `parts`.

Preserve the declared part array order when lowering/exporting, but placement
is determined by lane and offset, not by filename, socket label or array index.
For part byte `i`, even maps to `offset + 2*i`, odd to `offset + 2*i + 1`.
The existing `assembly::interleave_parts` requires equal-sized nonempty pairs,
one of each lane at each offset, and contiguous pairs from offset zero; gaps,
overlaps, duplicate lanes and missing partners fail validation. A four-chip set
records the lower pair first, then the upper pair with its explicit assembled
byte offset. Validate both chip identities and the reconstructed BIOS identity,
including the selected `FirmwareEntry.parts` alternative layout. Never mix
alternative layouts because they share a BIOS hash. An already assembled file
is the single-blob representation of that BIOS; it needs no fictional sockets.
Keep full EPROM/container bytes and their hash; the existing ROM-level `offset`
that skips an adapter container prefix is distinct from part placement and
remains provider/machine mapping, not a truncated payload identity.

`layers` contains precisely the unique ROM/chip blobs in first-use config/part
order, then extras under §5.2. There is no extra assembled payload unless it is
itself a single-blob ROM binding. Raw firmware uses §2's payload media type;
compressed variants obey its stored/decoded hash contract. For uncompressed
bytes, `blob` is `sha256:` plus the image/part's `sha256` and descriptor size equals
`size`. Reject mismatching identities, malformed digests, duplicate ids,
missing/unreferenced descriptors and unknown config fields/versions under §2's
bounds. The shared store handles manifests, descriptors and extras; the platform
provider owns roles, chip assembly, catalogue lookup and compatibility checks.

**Fit information and validation.** `platform` routes to a provider and must
match manifest `unemu.platform`. `fits.boards` and `fits.machine_classes` are
arrays of provider ids: alternatives within each array, conjunction between
nonempty arrays. Omitted/empty lists mean unspecified, never universal fit.
PC classes derive from board models (`BoardModel::class`); class `at` alone does
not establish fit for every AT board. The provider checks each declared catalogue
id against `FirmwareEntry` kind, full hash/size, chip layout and board list.
Package declarations can narrow that list but cannot broaden it. Mismatches are
errors; vendor/version/date and free notes cannot override the catalogue.

For a particular machine, the provider checks platform, board/class constraints,
all selected logical ROM roles, CPU minimum, required board parts, services,
ROM placement/scan windows and the ordinary assembly validator. A multi-ROM
package is usable only if every chosen binding passes together; it never
silently replaces unrelated machine firmware. Library check marks mean that a
named local machine draft passes these checks with verified bytes; pending byte
checks show “Checking…”, with no check mark. Return structured reasons for an
incompatible board, missing device, unsupported provider or incomplete chip set.
Fit is distinct from catalogue execution evidence (`Listed` versus `Tested`);
a check mark is not a new boot qualification. Reserved boards or unsupported
roles remain storable/listable, with Use disabled and a reason.

For an unlisted digest, fit declarations are user claims, shown as unverified.
Store/import may retain it only by explicit addition. Use still follows the
existing run-only `allow_unlisted_firmware` permission and validator; importing
a package never grants that permission or adds a trusted catalogue entry.

**Extras and redistribution.** Reuse §5.2's exact `extras` schema, blob ordering,
Open-on-host document action, deterministic transfer-medium proposal and safe
export rules. Manuals, technical references, source listings and errata stay
with their ROMs without becoming executable firmware or guest launch commands.
No second attachment format is introduced. Every descriptor, including config
and documents, defaults to `unemu.redistribution = "user-supplied"`; the rules
below decide when a layer is `open`. Proprietary
ROMs and restricted documents stay in private stores, registries and private
archives; they never enter repository fixtures or public exports. Reviewed open
firmware can be `open` with SPDX licence/provenance under §5, for example SeaBIOS,
Kiselev BIOS or XTIDE Universal BIOS. Matching an open project name alone proves
no licence. The most restrictive local classification per digest still wins;
public-export recipes omit restricted bytes as §5 requires.

**Licences, source archives and descriptive metadata** (decision-27, amendment
2026-11-15). Each descriptor carries an SPDX licence expression in
`unemu.license` next to its `unemu.redistribution` mark. A descriptor is `open`
only with both `open` and a valid expression (SPDX 2.3 Annex D: `AND`, `OR`,
`WITH`, parentheses, canonical non-deprecated identifiers from the program's
accepted list, or `LicenseRef-…`); a missing, deprecated or unparsable licence
counts as user-supplied, for the guard too. This is the safe reading of packages
written before licences existed. The rules live in `image_store::license`:

- *Rights follow the bytes.* Every ROM image and every chip is matched by
  SHA-256 and length against all catalogue entries and their chip layouts. A
  match with a user-supplied (proprietary) entry, such as an IBM or Landmark
  ROM, locks it to user-supplied whatever its `catalogue_id`, the candidate or
  any statement says, at import and in every edit. The same check refuses a
  licence for a document or source archive with such bytes, and refuses an
  unlisted package whose id is a catalogue id. The CLI push adds the same
  restriction to the guard's local marks for every stored or decoded digest in
  the closure, so a pulled package from a store whose catalogue did not know
  the ROM cannot carry it out either.
- *ROM images.* A ROM the catalogue lists as open (`FirmwareDistribution::Open`)
  is open under the catalogue licence; restating another licence is refused, and
  the user may still keep it user-supplied. Any other ROM is user-supplied
  unless the user states `open` with a named licence; the program never
  guesses. A package's ROM images share one licence. Under a GPL, LGPL, AGPL or
  MPL family licence a ROM is open only together with an open source archive,
  catalogue-known or not: a catalogue-open copyleft ROM imported without one is
  kept user-supplied with a note naming the fetch script that builds its
  archive, and an explicit `open` that cannot hold is refused. An expression
  needs the source when any of its identifiers is such a licence, even one
  `OR` alternative (`MIT OR GPL-2.0-only` and `MIT AND GPL-2.0-only` both do),
  as the approved Firmware collection design (mock-up revision 3) asks.
- *One switch.* `license::ACTIVE` is `Policy::STRICT` (source required for
  every copyleft ROM; the `OR` rule above). `Policy::ACCEPTED`, the first rule
  set, needs the source only when every choice an expression offers is
  copyleft; switching back is one line.
- *Source archive.* A zip or POSIX tar file, recognized by its signature and
  identified by SHA-256 only, becomes its own layer with media type
  `application/x-tar` or `application/zip`. Tar archives of at least 4 KiB are
  `+zstd` payloads like extras; zip archives stay raw, being deflate-compressed
  already. The config lists it in `[[sources]]` with the ids of the ROMs it
  covers; each covered ROM descriptor names the archive's decoded digest in
  `unemu.source`, which the provider checks against the config. The archive
  has its own class: it is open under the catalogue licence only when its
  SHA-256 equals the archive the project pins for that catalogue ROM
  (`source_archive_sha256`, the `SOURCE_ARCHIVE_SHA256` of the Kiselev,
  vgabios and XTIDE fetch scripts, which build them reproducibly); any other
  archive is open only by the user's statement (`--open --source-license
  SPDX`) and is otherwise user-supplied, which keeps a copyleft ROM private.
- *Documents.* Extras keep §5.2's schema and are user-supplied unless an
  explicit licence marks them open.
- *README and config.* Every import and edit writes `README.md`
  (`text/markdown`, UTF-8, LF, generated only from the config and the layer
  licences): title, description, platform, vendor, version and date, labels,
  fits and required devices, each ROM with its identity and chip markings
  (socket, part number, lane, offset, size, SHA-256), provenance, notes and
  known quirks, and a licence section per layer. The README and the config
  are open under `CC0-1.0` when every other layer is open, and user-supplied
  otherwise. The config references the README layer as `[readme]`; the text
  is generated without that reference, so it cannot depend on its own digest.
  A README the user writes by hand (the GUI's README editor; the statement's
  `readme`) replaces the generated text, with LF line ends, and keeps the same
  licence rule. Its layer carries `unemu.readme = "hand-written"`, and every
  later edit keeps it, from the GUI or `image firmware edit`, until the user
  asks to regenerate it (Regenerate from fields, `--regenerate-readme`); a
  generated README is generated again by every edit.

The config gains optional fields, so earlier configs still decode:

```toml
name = "Kiselev 8088_bios 1.0.2 (Xi8088, AT reset)"   # the title
notes = "Reads the AT display switch with inverted polarity."  # several lines allowed
description = "Open-source PC BIOS by Sergey Kiselev in its Xi8088 configuration."
labels = ["open-source", "xi8088", "at-reset"]

[fits]
boards = ["ibm-5170"]
machine_classes = ["at"]

[provenance]                  # kind: dumped | downloaded | built; omitted when unstated
kind = "built"
from = "skiselev/8088_bios a4af458a72bc33c4be066b33bd76e8af2b24111c (MACHINE_XI8088), at-reset.patch"
revision = "a4af458a72bc33c4be066b33bd76e8af2b24111c"
date = "2026-11-11"           # supplied, never the host clock

[[roms]]
id = "system-bios"
role = "system-bios"
catalogue_id = "kiselev-xi8088-1.0.2-atreset"
sha256 = "61589573c9809273198b137d9419c87242b0a5f63c2ad834a9565950c3d6becc"
size = 32768
blob = "sha256:61589573c9809273198b137d9419c87242b0a5f63c2ad834a9565950c3d6becc"
requires = ["kbc-8042", "rtc-cmos", "pic-cascade"]

[[sources]]                   # the vendor fetch script's reproducible archive
blob = "sha256:ced63707661747fc045d1e6e565a5cc9be73f95a9c944accedd77b19467fb5f9"
filename = "kiselev-bios-a4af458-atreset-src.tar"
size = 481280
format = "tar"
roms = ["system-bios"]

[readme]                      # generated; digest and length illustrative
blob = "sha256:<README digest>"
size = 2215
```

Import prefills fits, required devices (`assembly::FirmwarePart` as the device
kinds `kbc-8042`, `rtc-cmos`, `rtc-cmos-128`, `pic-cascade`, `video-rom`,
`ata-primary` and `vga-registers`) and, for open firmware, the catalogue's
provenance text without a kind, because the catalogue does not say how the
bytes were made. Flags set the title, description, notes, vendor, version, date,
labels, fits (catalogue fits may only be narrowed, as the provider already
checks), required devices of unlisted ROMs, and provenance. In a statement an
empty text clears an optional field and an empty provenance clears the
provenance; an empty title, an invalid date and a provenance without details
are refused with their own reasons. Labels, boards,
classes and required devices are stored sorted, so equal sets have one
spelling, and licences in their canonical SPDX form. Layers are ordered ROMs,
source archives, extras, README. The manifest carries the standard
`org.opencontainers.image.title`, `.description`, `.vendor`, `.version`,
`.source` (the provenance address), `.revision` and, for an all-open package,
`.licenses` (the conjunction of the layers' distinct licences, built from their
parsed expressions and therefore not bound by the 256-byte limit of one
licence statement); source, README and
extra layers carry `org.opencontainers.image.title` with their file name. There
is no `.created`: no creation time is supplied and the host clock never enters
a package. Two imports of the same inputs give byte-identical manifests, and the
identity is known before anything is written.

*Revisions.* A package is immutable. `image firmware edit NAME:TAG` applies
the same metadata and licence statements, can add or replace the source archive,
and publishes a new revision under a new tag (`:N` to `:N+1` by default) that
reuses the stored ROM and document blobs, with a new config and README. A
package under the tag that holds the same ROM and document bytes counts as
already imported, whatever its metadata, licences, source archive or README
(written by an earlier version, by another statement or after a GUI review),
and an edit changes it; an import whose statement would change such a package
is refused and names the edit, and any other bytes under the tag are a
conflict.

*Push guard.* Besides the user-supplied rules of §5.1, rule 4 refuses an open
ROM layer under a licence that needs its source when the linked archive is not
in the same closure, unless the target is a registry marked private outside
`unemu/`. Rule 4 checks ROM layers only; documents and archives fall under the
user-supplied rules. Its status for one layer is
`image_store::license::source_missing`, for frontends that show it. An all-open
package with its source may enter `unemu/`. The most restrictive
local mark per digest still wins, so a user-supplied record of the same ROM in
another local image keeps it private.

*Machine images.* A ROM resolved from a package is embedded user-supplied, as
before, unless the catalogue lists it as open, the package marks it open and
the package links an archive it marks open. Then the machine config records the
archive's digest as `source_archive` on the firmware binding, the image embeds
the package's archive layer (the same stored blob, so it costs nothing extra),
and the ROM is open under the catalogue licence and the archive under its own,
linked by `unemu.source`. A ROM read from a plain file stays user-supplied. The rest of
an imported machine image stays user-supplied, so this only keeps the ROM's
local mark from restricting the package. New machine… (`pc_image::create`)
binds package ROMs by digest under the same rule: a ROM open under a licence
that obliges its source brings its package's archive layer and link, any other
ROM keeps its package's marks without the link, and bytes the catalogue lists
as proprietary stay user-supplied. Its generated configuration, metadata and
zero bases are open under `CC0-1.0`, so the preview's closure, judged by the
guard's own rules, tells whether the image may enter `unemu/`.

**Identity and determinism.** Full SHA-256 identifies each exact stored blob;
per-ROM/part hashes also verify decoded bytes (§2). The config digest identifies
ROM bindings, part order/placement, fit claims, descriptive fields and extras.
The **package digest** is the OCI manifest digest; tags, transfers and source
provenance pin it. A firmware package has no machine digest. Editing fit text or
documents changes package identity; a machine's identity changes only when its
resolved guest-visible config/firmware changes. Use stable versioned TOML and
manifest serialization, LF, config/part traversal order and no generated time
fields; `date` is supplied firmware metadata, never the host clock. Folder import
and hashing are host preparation and cannot advance virtual time. Replays pin
resolved bytes/config and events at retired-instruction boundaries, not tags,
folder order or discovery timing. Full verification, closure checks, tag-last
publication, repair and private transfers reuse §§3.1/5.1.

**Import a ROM folder once.** An explicit Import ROM folder… action enumerates
files in stable relative-name order, with cancellable listing/hashing progress.
Match complete-file SHA-256 **and length** against the provider catalogue and its
known chip layouts, independently of filenames. Preview logical packages assembled
from complete matching layouts; show socket/lane/offset and the assembled hash.
Duplicates share one blob; alternative layouts remain separate choices, never
an arbitrary mixed set. If several catalogue identities fit, ask for a selection
in the preview rather than choosing by enumeration order.

Report unmatched, unreadable, size-mismatching and incomplete inputs separately,
with reasons. An incomplete chip set is a disabled candidate (“1 of 2 chips
present”), not a tagged package. Unmatched files are unchecked and **not imported**
unless explicitly added with a role, layout and fit claims; they remain unlisted.
Documents likewise require explicit addition as extras. Import selected copies
and fully verifies only selected complete packages through §3.1 before publishing
tags. Cancellation publishes no incomplete package. Source files stay unchanged;
paths remain local hints, never package fields. Afterwards the imported packages
work with the folder offline or removed; readiness does not rescan it. Re-import
or refreshing the remaining folder fallback is a separate explicit action.

**Store-first machine-file resolution.** Implemented with explicit frontend store
roots; plain library loaders use declared paths and ROM roots without a store. Resolve a
machine file's inherited declarations and pins first, then use this order:

1. Query a rebuildable store firmware index by exact assembled/per-chip pins and
   compatible role/layout/catalogue identity. It maps identities to package
   manifest digests and blob bindings, not to mutable tags or name matches. Resolve
   and pin a complete package once, verify its bytes and all supplied machine
   pins, then use the existing assembly/validation path. Identical-byte matches
   are interchangeable; use a stable package-digest order for provenance. No
   “latest BIOS” selection or package metadata may change the machine's mapping.
2. On a store miss, check the explicitly declared image/part paths, preserving
   defining origins and existing pin diagnostics. An unpinned explicit path stays
   an explicit path: never infer a store package by basename or title. Missing
   identity requires user selection and verified pins before store lookup.
3. For still-unresolved pinned inputs, use the existing persistent ROM index
   (`firmware-index.v2`) and configured ROM-folder discovery, with the frontend's
   current root policy. Metadata-validated cached hashes are candidate hints;
   final load still verifies bytes. Refresh stale listings and retain the current
   single retry after a located file fails verification. Discovery alone does
   not import files; offer the explicit import action to avoid later rescans.

The store firmware index and persistent host-folder index have different jobs.
Rebuild store summaries from package configs without listing external folders;
never load or refresh folder roots on a complete store hit. A damaged selected
store package blocks use and offers repair, rather than silently substituting
folder bytes. A store miss is different from corruption. With no configured
store, preserve today's machine-file workflow and its folder fallback; repository
fixtures/goldens need no store. Neither index replaces full hash/pin validation.

**Embedding in machines.** Use in machine… previews selected bindings and the
resolved machine diff for a stopped draft; it never changes a running machine
or writes source TOML implicitly. Machine image construction embeds the exact
ROM/chip blobs into its **own closure**, preserving machine-file parts, lanes,
offsets, container mapping and pins (§2). Record contributing package manifest
digests in manifest annotation `unemu.firmware.sources`, a string containing a
JSON array of unique digests in first-use order, parallel to
`unemu.media-set.sources`. This is provenance only; execution never fetches the
package or depends on its tag, extras, source folder or index. Package and machine
share identical blobs by digest. Removing the package tag cannot break the
machine; its own closure is a GC root. A provenance-only change creates a new
image digest with the same machine digest.

**Library.** Add **Firmware** beside All machines and Installation media. Rows
show name, role summary, fits, version/date and redistribution. The details pane
shows catalogue ids, logical ROMs, socket/lane/offset layout, short SHA-256
(with full values available), boards/classes and checked compatible library
machines, documents with Open, and Use in machine… / Export. Incomplete import
records stay visible but disabled with the missing-chip reason and Resume/Remove;
no incomplete candidate is presented as a usable package. Private Export carries
the complete verified package closure. Use is disabled for missing/damaged blobs,
unverified pending checks or unsupported roles/providers, with the provider's
reason. Reuse §7's versioned summary cache, lazy bounded rebuilds and incremental
text search over name/vendor/version/notes, roles, boards/classes and catalogue
ids. Warm listing never reads ROM bytes or scans folders; fit checks for selected
machines remain separate readiness work. Edits stay local overlays until explicit
new-version publication; personal notes remain local as in §2.1.

*Implemented (2026-11-15):* the Firmware node with its package count, Role, Board,
Machine class and Redistribution facets, search, and sortable rows with name,
roles, chip count, vendor, version/date, fits, catalogue ids, redistribution and
size. Package summaries carry the config's descriptive fields, logical ROMs and
extras, so the pane shows the chip layout (socket, part number, lane, offset,
size, short SHA-256 with the full value on hover), documents and **Used by**: the
local machine images that hold every chip of a ROM, or its assembled image.
Both sides compare raw identities (ROM and chip pins; a `+zstd` layer's decoded
digest), so compression does not hide a match. Used by is not a compatibility
check. Import ROM folder… (File, Tools and the collection) scans on the library
worker with the persistent ROM index and progress; Cancel stops the scan. The
review lists complete sets (picked by default), already imported sets,
ambiguous identities (unpicked; two picks of one identity are refused) and
refused sets with the scanner's reasons. Unmatched files appear only as a count
by cause (size or SHA-256 mismatch); unreadable files are listed by name.
Import selected verifies and publishes the picks. The GUI refuses per set: a tag
conflict, a changed source or another failure refuses only that set, and the
other picks still import. `image firmware import` stops the whole run at the
first tag conflict instead. Both use the same ambiguity rule
(`Candidate::is_ambiguous`). Firmware rows never offer Start. Use in machine…
and Export are shown disabled; documents are listed without Open because no
host-open path exists yet.

*Implemented (2026-11-15), mock-up revision 3:* the Import ROM folder… review
lists a set whose tag already holds its ROM bytes as already imported, and
gives every new set a Redistribution choice (catalogue-open sets preset open with
the catalogue licence, proprietary ones fixed user-supplied, others open only
with an SPDX licence from the parser's list or a free expression the parser
checks as it is typed), Source archive… (the host file picker, opened in the
scanned folder, next to the user's statement that the archive is the
corresponding source; the library checks the zip or tar signature and records
the SHA-256) with the archive's own licence statement, and a per-set details
editor: title, description, notes,
labels, fits (catalogue fits narrowed only, with Restore catalogue fits),
provenance (kind, details, supplied date), the layers and licences table and an
editable README preview. Edit metadata… (in the pane and the File menu; Add
source archive… is the same dialog with the picker) uses the same editor on a stored package and publishes
`:N+1` through `firmware_import::edit`. Neither dialog decides a rule: the
library worker previews each changed statement (`preview_manifest_with`,
`preview_edit`, one at a time per statement), and the dialog shows the verdict,
the layers' marks, the README text and the library's reason, with its
command-line flags named as the dialog's controls; Import selected and Publish
new revision stay disabled until every picked statement is accepted. The
package pane adds the description, labels, provenance, the licence of every
chip, document, source archive and README, the guard's Config and Push lines
(the command line's push plan, local and catalogue marks included, read on the
worker), the README section with Open README (the text shown in the program)
and, for a copyleft open ROM without its source layer
(`license::source_missing`), the warning badge in the row and the pane with the
push-guard notice and Add source archive….

**Open point:** maintainer approval of this section and the Firmware collection,
chip-details and Import ROM folder… views. The artifact shape, private default,
extras reuse and store-first order are accepted; no further policy choice is
needed for this design step.

### 5.4. Future artifact types

These are accepted directions, not detailed schemas or implemented capabilities.

**Saved states.** `upc-5pw.17` will define the portable artifact and compatibility
contract around §2's reserved `saved_state` field and
`application/vnd.unemu.machine.state.v1` payload. Pin exact machine/media and
emulator/CPU/device versions as §4 requires; reject the reserved field until
save/restore support exists. A disk checkpoint alone is not a saved machine.

**Deterministic input recordings / replays.** A recording pins a machine digest,
initial disk heads (and other mutable initial state) plus a virtual-time input
script. Events use retired-instruction boundaries, with the halted-clock contract
where applicable, never host timing. This supports speedruns, demos, reproducible
bug reports and goldens. Event encoding, compatibility and verification still
need their own design; this section enables no replay action.

**Collections.** A portable collection can use a native OCI image index to group
machine, installation-media and firmware manifest descriptors by digest, reusing
ordinary OCI traversal and deduplication. This differs from today's local UI
collections/labels. Ordering, collection metadata and missing-member behavior
remain for a later design; no custom collection payload is specified here.

### 5.5. Official template images

Shipped machine files (`machines/`) are development and test fixtures. What a
user starts from is an **official template image**: an ordinary machine image in
the official namespace of the default registry, pulled into the local store
like any other (§5.1), never a machine file.

**Naming.** Templates and the firmware packages they use live under `unemu/` on
`registry.unemu.dev`, with Docker-style short names (§5.1):

| Repository | Content |
| --- | --- |
| `unemu/<name>` | a template machine, such as `unemu/open-at` |
| `unemu/firmware/<catalogue-id>` | the open firmware package of one catalogue entry |

Both follow the OCI repository grammar (`[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*` per
component), so catalogue ids such as `kiselev-xi8088-1.0.2-atreset` need no
mapping. Each template generation has a semantic-version tag (`1.0.0`), which
never moves, and `latest`, which follows the newest version. The local tags of
a template build are the short forms a pull writes: `open-at:1.0.0` for
`unemu/open-at`, and `unemu/firmware/<id>:1.0.0` for a package (two components
remain after `unemu/`). Media sets and future software images get their own
names when their licence decision is made.

**First generation (1.0.0).** `open-at`: an IBM 5170-class board with the
Kiselev AT-reset BIOS, the unemu VGA BIOS and XTIDE Universal BIOS r638 (each
with the source archive the catalogue pins), 640 KiB + 1 MiB, VGA, a COM1 UART
without a mouse, one 1.2M drive with an empty catalogue, a blank zero-base 504
MiB (1024/16/63, the largest INT 13h CHS disk) ATA master at 1F0h, no sound
card. `open-at-games`: the same plus the cards of `machines/at-games-open.toml`
without its media: SB16 at 220h (IRQ 5, DMA 1 and 5), a Microsoft mouse on
COM1 and a 2 MiB Lo-tech EMS board at 260h with its frame at D000h. Neither
holds an operating system or other software: disk images with an operating
system need their own licence decision.

**Build.** `image template build` (commands.md, Official template images)
imports the three packages from the fetched fixtures with their pinned source
archives, then creates each machine through the New machine create API
(§5.3, `pc_image::create`): zero-base blank disks, a generated configuration
(CC0) and the packages' ROM and archive blobs bound by digest. Titles and
descriptions are manifest annotations. Nothing depends on the clock, the host or
the fixture paths, so the same fixtures give the same manifests and digests on
every run and host. Before any tag of an image is written, the build applies
the push guard of `unemu/` on the default registry (every blob open, each
copyleft ROM with its source archive in the closure) and stops on a refusal;
only the judged manifest is tagged. Version tags never move; a changed template
needs a new version, and a test pins the digests of the published one. `latest`
moves only away from an image that lower versions of the same repository name,
never from a higher version or from an image the build did not version. **The
build never pushes**: publishing the tags is the maintainer's step, with a dry run
of each push first.

**Get templates….** File › Get templates… follows the approved get-templates
mock-up (`mockups/get-templates.html`); this paragraph is the one description
of its behaviour, which commands.md and status.md point to. The library offers
it in its empty state while it holds no machine image; firmware packages and
media sets do not count, and the empty state contacts no registry. The dialog
lists the templates of the default registry (`UNEMU_REGISTRY`, then
preferences): repositories the registry catalog (`/v2/_catalog`, paged with
`n` and `Link`; zot lists the repositories the caller may read, so a stored
login goes with the first request) names under `unemu/`, without
`unemu/firmware/…` and `unemu/test-…`, which are not read at all, keeping
manifests of the PC machine artifact type only. Each repository offers the
version tag `latest` names when that is one of its 16 newest releases (its
pre-releases when it has no release); otherwise it offers the newest of them.
A row shows name, tag, title, description and the size of the manifest and
its distinct blobs. A template already in the library, or whose local tag
names another image, is dimmed and cannot be chosen; a click anywhere on any
other row chooses it, and Get is hidden when the registry offers none. A
repository that does not answer properly, or that this account may not read,
is skipped. An offline or unreachable registry, a refused or a missing login,
a registry without a catalog or an unreadable library ends the listing with
one message and Try again, after at most the login-check timeouts (10 s to
connect, 15 s per read); closing the dialog stops the listing before the next
repository. A registry without a catalog would need a small well-known index
image instead; the catalog suffices for zot, so none exists. The two login
cases offer Log in…, which opens Tools › Preferences › Registries on that
registry's Log in dialog; a registry with no entry there offers Add
registry…, which opens the Add dialog prefilled with the host and no other
row selected. Either way the dialog lists again once Preferences closes. Get
pulls the chosen templates one after another on the library worker through
the Pull… path, by the digest shown, so each closure is verified before its
local tag is written. One progress bar covers them all; blobs the library
already has, also those a template earlier in the same Get fetched, count as
done and are not fetched again. While pulls run, Close becomes Cancel: Cancel
and Escape stop the running pull at its next read and the queued ones before
they contact the registry, and quitting cancels them as in Pull…. Once every
pull answered, the dialog sums up what was added and offers Close or Back to
the list; the status bar names the templates added.

## 6. Capabilities API sketch: validator-owned choices

READ: `crates/assembly/src/validate.rs`, `validate_resolved`, resource checks,
`check_board_memory`, IRQ routes and DMA inputs. It returns the first error in
fixed order, not a form schema. Some current representation domains live in
machine-file lowering/device configs. A UI must not transcribe those tables.

Each emulator exposes capabilities through a shared interface; its own validator
owns the hardware rules. The following is the PC provider sketch, not a dependency
of shared crates on PC assembly. Propose PC assembly-owned descriptors for
boards, card kinds, fields, defaults, scalar/enum domains, units, derived resources
and constraints. Lowering consumes these same domains; the validator consumes the same predicates. Keep
source-location mapping in machine-file. Move only duplicated facts as needed;
this is a proposal, not a rewrite of validation or a second rules engine.

Conceptual transport-neutral calls (JSON possible, not a network requirement):

```text
describe(schema_version) -> boards, card_kinds, field descriptors, rules_revision
choices(draft, field_path, draft_revision) -> values/range, enablement, reasons
validate_draft(draft) -> errors, pending_checks, resource_map, draft_revision
preview(draft) -> canonical machine TOML, diff, effective defaults, provenance
```

Each field descriptor has a stable path/key, accepted type, required/default,
unit, enum/range/step, editability and dependent fields. Candidate evaluation
replaces the old field/card, lowers the proposed edit, and runs shared validation;
it must not collide with the card's own previous resource allocation. Multi-field
edits such as switching IDE primary/secondary are evaluated atomically. Sparse
integer domains are returned as intervals/steps with conflict exclusions rather
than enumerating every possible RAM size or UART base.

A structured diagnostic carries `code`, `field_paths`, conflicting card ids,
resource/range, severity and readable reason. For incomplete drafts, run every
check whose prerequisites exist and mark the rest pending. Do not classify an
option as forbidden just because an unrelated required firmware blob is damaged
or not yet imported. For
complete configs the existing first-error API retains its order; an optional
collecting adapter invokes the same checks in that order. Option states are
`allowed`, `blocked`, or `pending` (for byte-dependent/missing inputs); pending
never means safe to Run. Server-side apply/launch revalidates the current draft,
including byte-dependent firmware/geometry and host path alias checks. Responses
carry revisions so stale choices cannot authorize an incompatible edit.

| Field or constraint | Example exposed by shared descriptors and validation |
|---|---|
| Board / CPU | `ibm-5170`, `80286`, optional `80287`; unsupported boards/CPUs disabled. CPU `rate = "8/13"` is steps/PIT-clock throughput, not cycle-exact MHz. |
| RAM | Conventional KiB step 16, maximum 640; extended KiB step 64, maximum 15232 for this board. Validate actual mapped intervals too. |
| Video | mda, vga-text, vga; VGA variants exclude one another; full VGA/MMIO may exclude MDA or EMS by overlapping decode. |
| SB16 | Bases 220h/240h/260h/280h, IRQ2/5/7/10, dma8=0/1/3, dma16=5/6/7. IRQ2 routes to AT IRQ9. Claim OPL ports and both DMA controllers. |
| EMS | Port 260h/264h/268h/26Ch with all 400h aliases; size 512/1024/1536/2048 KiB; 64 KiB frame at C000h or D000h if free. E000h blocked by system ROM on ibm-5170. No IRQ/DMA fields. |
| Conflicts | Report half-open MMIO/RAM/ROM intervals, port width/decode aliases, actual routed IRQs and `(controller, channel)` DMA ownership, not only displayed base values. |
| IDE / firmware | Primary 1F0h/3F6h/IRQ14 or secondary 170h/376h/IRQ15; slave requires master; fixed-disk firmware service required. |
| Drives / ROM | Exact geometry per catalogue entry; ROM placement, container offsets, chip lanes, services, compatibility and missing hash bindings remain validator/loader checks. |

Hardware rules are the existing models, not additional fidelity claims. See
[machine-files.md](machine-files.md), §§RAM rule, Cards and SB16: IBM Personal
Computer AT Technical Reference, Section 1, System Memory Map; Creative Sound
Blaster Series Hardware Programming Guide, §2. The UI displays validator reasons,
including an EMS C000h conflict with a video ROM, without embedding these facts.
Test acceptance: every advertised allowed complete candidate passes the existing
validator; blocked candidates produce its typed error; pending byte checks block
launch. CLI and UI must obtain identical results and source-located diagnostics.

Settings on an image (§7) describes a stored image's flattened hardware tables
through the same lowering as a machine file and takes every choice, reason and
diagnostic from these calls; structural choices the field list does not carry,
such as installing diskette drive B:, are candidate tables run through the
same calls. Publication revalidates the edited tables with the real ROM bytes
through the PC provider before any tag moves.

## 7. Approved desktop behavior and technology

The shell follows the approved [window mock-up](mockups/main-window.html) and
[decision-24](<../../decisions/decision-24 - Desktop shell is a classic in-window menu UI built with egui.md>).
One classic Win32-style application window has a title bar, in-window menu bar,
toolbar, client area and status bar with power and drive activity lights. The
menu bar stays inside the window on **every platform, including macOS**; the
macOS global menu keeps only the minimum: the application menu with About,
the standard Hide, Hide Others and Show All, and Quit. About and Quit run the
shell's own About and Exit, so Quit, Dock › Quit and logout ask before stopping
a running machine. Application commands remain in the same place on macOS,
Windows and Linux. Shared shell widgets use the selected provider's
capabilities for settings, media, guest
commands and status fields; the PC menu/dialog examples below do not require
other platforms to expose Ctrl+Alt+Del, floppy drives, CMOS or EMS. About names
the active emulator.

| Menu | Commands and role |
|---|---|
| File | New machine…, Open machine file…, Import image…, Import installation media…, Import ROM folder…, Machine library and Exit. |
| Machine | Start, Pause/Resume, Send Ctrl+Alt+Del, Power cycle, Stop, Settings… and Disk layers…; Save state remains disabled until supported. Gaming mode is a Machine-menu action when disposable workspaces are available. |
| Media | Per-slot inserted names, Insert…, Next image in catalogue and Eject; absent drives and unsupported CD-ROM are disabled with reasons. |
| View | Toolbar/status-bar visibility, scale, full screen and host audio mute. |
| Tools | Image store folder…, Import ROM folder…, Media catalogue…, Locate firmware…, Create blank disk… and Preferences…. |
| Help | Keyboard shortcuts and About unemupc98. |

The toolbar repeats common Start, Pause, Stop, Send Ctrl+Alt+Del, floppy,
Settings and Layers commands outside guest pixels. Enablement follows machine
state and backend capabilities. The status bar reports readiness/running state,
power, drive activity, CPU/rate, EMS and audio state; pointer capture/release is
visible in status text. Indicators observe completed boundaries, never drive
guest execution. View can hide the toolbar and status bar.

When stopped, the client area shows a welcome pane, the Machines view and details
for the selected image. The library must remain usable with **1,000+ local
images**, including stores with no configured registry. The Machines view is
Explorer-like in the same classic Win32 style: a tree pane on the left with
All machines, Recent, Favorites, user Labels/collections, the Genre, Platform,
Year, Developer, Publisher and Series facets (§2.1), the Installation media
collection (§5.2), the Firmware collection (§5.3) and, only when configured, Registry; a details list view fills the right pane. Sortable columns are name,
tag, last run, size and labels. A search box filters all text fields incrementally.
Group images by repository name, with tags as child rows, for example
`dos-lab > 1.0, 2.0, gaming`. The list is virtualized: draw only visible rows.

Startup reads the OCI index and the local summary cache, never every
manifest. Cache machine platform, name, tags, title, labels, stored size and last
run, plus per-title facets and the text-search index (§2.1); key immutable
summaries by manifest and metadata digests and version the cache schema. Index changes
invalidate affected tag mappings; library-metadata revisions invalidate local
labels/favourites/last-run, personal-field and overlay entries. Rebuild missing
or stale summaries lazily from
manifests in bounded batches after startup, prioritizing visible/selected rows;
show pending values until ready. Continue until all stale entries are rebuilt.
Warm-cache sorting and incremental search use summaries,
without fetching payloads or rehashing disks. On a cold cache, search/sort results
fill in as summaries are rebuilt rather than blocking startup on the whole
library. Rebuildable size summaries never replace the current full hash check
at every Start (§3).
Labels, collections and favorites live in local metadata. Labels the user wants
to travel with an image go in the metadata blob's `labels` (§2.1); edits stay
in a local overlay until published as a new version/tag, giving a
new image digest for the same machine. Local and travelling labels are combined
for display and filtering. Personal ratings/notes always remain local (§2.1).

The approved [main-window mock-up](mockups/main-window.html) shows the simple
list. The Explorer-like library requires its own mock-up and maintainer approval
before the library bead (`upc-5pw.9`) is implemented; this note changes no mock-up
or accepted shell styling.

Selected-image details contain hardware, firmware readiness and the base/layer
chain, with Start, Settings… and Layers… actions. Capacity, stored/exclusive bytes
and dedup savings belong in these details and the library's store summary.
A damaged or missing blob blocks Start and offers repair; an unfinished pull is
an incomplete row with Resume and Remove (§3.1). Tools › Locate firmware…
supplies ROMs when building/importing an image.
Last-run wall time is local library metadata only. Start, Enter or double-click
on a tag row replaces this same client area with the emulated screen and updates
the title to the machine name.
Stop returns to the welcome/library view; it does not open a separate dashboard.
Gaming mode uses the same transition with an isolated disposable workspace and
the keep/discard behavior in §4, without a separate launcher view.

Actions open **classic modal dialogs** over the application window:

- **Settings…** is a property sheet with System, Memory, Display, Sound, Storage,
  Expansion and Firmware tabs, and OK, Cancel and Apply controls. Board/CPU/rate,
  memory, video, SB16 port/IRQ/DMA, drives, EMS and firmware belong to their
  respective tabs. Disabled options show capabilities-API reasons beside the
  field; the UI does not duplicate validator rules. Pending byte-dependent checks
  cannot authorize Start. Before applying, preview the flattened machine-file
  diff and separate image/media binding changes within the dialog. Saving creates
  a new image revision; hardware changes require stopping/relaunching.
  **Settings on an image** (approved [save-as-image mock-up](mockups/save-as-image.html),
  revision 2) edits a draft of the stopped image's flattened hardware tables;
  a running image opens read-only. Each edit goes through the capabilities
  API and is written into those tables at once, so the review shows exactly
  what was validated. Firmware stays as stored. Diskette drives can be added
  (with an empty catalogue) or changed only while their catalogue is empty;
  a drive with media keeps its type and cannot be removed, and no fixed disk is
  added or removed here. **Review changes…** opens *Publish new image
  revision*: the line diff of the flattened configuration, the CMOS bytes
  recomputed as SETUP would, every binding (shared by digest or new), the old
  and new machine digests and the name and tag. Publish writes a new
  configuration blob, a new 64-byte CMOS blob when one changes, and a manifest
  that binds every disk, firmware, catalogue medium, metadata and artwork blob
  of the source again; the source image and its tags never change, and its
  workspaces keep running it. The manifest keeps the source's annotations
  (title, `unemu.firmware.sources`, others) and records
  `org.opencontainers.image.base.digest`. CMOS: a stored image keeps every
  byte except the SETUP fields whose value derived from the machine changes —
  diskette types (10h), fixed-disk types (12h, 19h–1Ah), the equipment byte's
  diskette, coprocessor and display bits (14h), base memory (15h–16h),
  expansion memory (17h–18h) and the expansion size POST records (30h–31h) —
  and then the checksum over 10h–2Dh (2Eh–2Fh); the clock and other options
  are kept. A stored image whose SETUP values cannot be derived (an unlisted
  fixed-disk geometry its stored CMOS describes) is kept byte for byte, and
  the review says so. An image without a stored CMOS image derives CMOS at
  Start; the review lists the bytes that derivation changes. The library
  worker reads the image and checks the draft; the sheet itself never opens
  the store, and Escape with unsaved edits asks before discarding them.
- **Insert floppy** shows the selected slot's catalogue, Browse…, read-only or
  journal write mode, detected geometry and compatibility/retention warnings.
  Insert commits the selection; Cancel retains the current medium.
- **Disk layers** shows the parent chain and writable changes alongside settings,
  firmware and other drive heads. Save as layer…, Discard changes and New image
  from here… use the complete-machine semantics in §4; saving stops the machine.
  Save as layer…, New image from here… (with *Flatten disk chains into new
  bases*), Gaming mode's Keep as new image… and Machine › Save as new image… all
  open the one **Save as new image…** dialog, also used by *Publish new image
  revision*: repository name, tag and (for saves) title. The tag follows the
  registry grammar; it defaults to the next minor version after the source's
  own version tag (else after the repository's newest version, else 1.0.0),
  skipping taken versions. An existing version tag is refused with *Use
  <next version>*; any other existing tag moves only after *Move tag?* shows
  the digest it names now, and the store then swaps from exactly that digest.
  The store itself refuses to move a version tag, and records deleted version
  tags so such a name can later name only its old digest. A store refusal
  rereads the tags and asks again. The saved manifest keeps the source's
  annotations (dropping per-build ones such as `created`), may replace the
  title, and records the source as its base; a save without changes is the
  source manifest itself, so its tag aliases the same digest. An interrupted
  save is offered as *Finish saving <tag>* with its own tag and expected
  digest. The form starts from the image the workspace's session pins. New
  disk layers stay user-supplied until marked open.
- **Browse registry…** is a separate dialog opened from the configured Registry
  tree node. It pages repositories and tags, offers search and pulls the selected
  image into the local store. Use `/v2/_catalog?n=<count>&last=<repository>` where
  supported and `/v2/<repository>/tags/list?n=<count>&last=<tag>`, following
  pagination `Link` headers. Tag listing is in the OCI distribution-spec;
  `_catalog` is the [Docker Registry HTTP API V2](https://github.com/distribution/distribution/blob/main/docs/content/spec/api.md)
  repository-listing extension, not a required OCI distribution-spec endpoint.
  Some registries, including Forgejo packages, do not expose `_catalog`; use
  their package-listing API or accept a manually entered repository name.
  Search filters fetched pages incrementally; use server-side search only when
  the registry's own API supports it, and show when results are still partial.
  Fetch titles lazily from the manifest title annotation for visible entries,
  and the small metadata blob and cover for the selected one, never the disks. Ordinary local startup never browses a registry.
- **Keyboard shortcuts** under Help lists both existing hotkeys and the new shell
  commands; **About** identifies the application and its components.

Existing Ctrl+Alt+F/E (Shift for B:), Ctrl+Alt+M and Ctrl+Alt+G keep their meanings
and invoke the same actions as menus. The mock-up also maps Ctrl+Alt+I to Insert,
Ctrl+Alt+P to Pause/Resume, Ctrl+Alt+Q to Stop, and Ctrl+Alt+End to sending guest
Ctrl+Alt+Del. These additional bindings are shell work, not existing CLI features.
Menus/dialogs release mouse capture and cooperate at a run boundary; closing
them does not recapture automatically. Dialog editing pauses at that boundary;
cancelling does not alter mounted media. A failed media finish/insert stops
execution and displays the persistence error. Save state and CD/DVD stay disabled
until their backend exists. Illustrative hardware values in the HTML do not
override the validator or establish new emulator capabilities.

The **classic skin is the default**: grey bevels, compact controls and a gradient
title bar. Preferences offers a flat modern skin with exactly the same menus,
toolbar, client area and dialogs. Both skins support light and dark themes,
including the system preference. Skin and theme affect presentation only.

**egui/eframe is chosen by decision-24.** The egui shell replaces the current
winit/softbuffer presenter with one event loop and one render surface. The
emulated screen is a texture updated from the existing frame sink; preserve crisp
pixels and the headless core. Self-drawn widgets reproduce the accepted skin
across platforms without a web engine or IPC. The Rust build belongs in the Nix
devShell; actual dependency weight, offline builds, keyboard focus and VoiceOver
behavior still need implementation verification. AccessKit provides the
accessibility integration; egui uses Apache-2.0 OR MIT. See upstream
[architecture](https://github.com/emilk/egui/blob/main/ARCHITECTURE.md),
[accessibility](https://github.com/emilk/egui/blob/main/docs/accessibility.md) and
[licence](https://github.com/emilk/egui). Fonts, icons and dependencies remain in
the existing licence audit. The HTML is a visual reference, not a web runtime.

The short selection rationale is retained for context, not as an open choice:

| Alternative | Rationale for the chosen egui shell |
|---|---|
| iced | A Rust desktop alternative, but egui supplies the chosen self-drawn classic skin and a winit integration path. |
| Slint | Declarative UI adds a separate UI compiler/backend and licence choice without changing the accepted window structure. |
| Tauri / web UI | A webview and host bridge add machinery the framebuffer shell does not need. |
| Native per platform | Separate shells/SDKs make the same in-window controls and skin harder to maintain across platforms. |

The deterministic core stays untouched: the shell uses existing run, input, media
and observation boundaries. Audio never paces execution. Presentation clocks,
UI work and storage preparation never replace retired-instruction virtual time.

## 8. Migration and explicit exclusions

Import selects a platform provider and records its required platform in the
config, metadata and manifest annotation. For PC, import an existing TOML/YAML
machine through its current extends resolver and origin-aware loader; verify all
bytes/hashes, flatten once, assign logical blob names and preserve effective
device order, policies, slot order and catalogues.
Locate missing inputs during import, copy all firmware and media into OCI
blobs, and include every catalogue medium, disk parent/sector pack and CMOS.
Validate the full closure before publishing a self-contained OCI manifest and
local tag; no imported image retains runtime dependencies on user folders.
Existing files and CLI paths remain first-class; import is opt-in and never
rewrites source files, bases or journals. The repository's tests and goldens
keep using machine files with open fixtures and never need a store or a
registry; store and transfer tests use synthetic open blobs in a temporary root. Re-export materializes each head to a
new raw file and restores normal machine-file fields and fresh journal paths.

For a v1 journal, stop/release its writer, verify the exact original base and
scan read-only using existing recovery rules. Interior corruption blocks import;
interrupted tails require an explicit recovery choice on a private copy. Freeze
a chosen closed/recovered generation into a new raw base first; keep the old
journal/retention history as a local backup. A later verified conversion can
create a fixed-disk sparse layer against the original base by comparing final
sectors; floppies always freeze whole.
Per-medium generation ids cannot become a synchronized machine snapshot. First
import records the chosen generation separately for every disk/catalogue/CMOS.
Copy the resulting bases and any verified converted layers into the OCI store;
the image must run without the original journals, bases or firmware folders.

Implementation status: local store, PC import, disk layers, workspaces, offline
publication and the desktop image library/layers dialog are implemented; registry
transport and daemon remain unimplemented.
Signatures, chunk-level dedup, encryption, qcow2 compatibility,
OCI referrers, floppy sector layers,
sibling-layer merge, deterministic install recipes, live multi-disk transactions,
new CPU/board, CD/DVD emulation and a generic saved-state format remain excluded.
Compression is planned and required before phase C image pushes (§5.1), while
Windows typed-input launch profiles belong to phase C (§2.1).
A reserved future field/button must stay disabled
until its backend capability exists. Implementation proposals belong in the
supervisor's tracker, not a second task list here. The shell appearance and
egui/eframe choice are accepted in decision-24; the layer/journal codec rollout
remains a review point. New GUI work requires an approved mock-up first.

## 9. Open points for the maintainer

All policy questions from the 2026-11-13 review are resolved in the sections
above. Implementation gate outcomes:

1. Done 2026-11-14: the zot/`oras` registry verification spike passed (§5.1).
2. Done 2026-11-13: the maintainer approved the Explorer-like library mock-up
   ([mockups/library.html](mockups/library.html)) as the starting point for the
   library bead (§7).
