# Machine images, capabilities and desktop UX

Status: **`+zstd` binary blobs implemented** (§2), 2026-11-14: provider-declared payloads
are stored compressed (level 3, single-threaded) with decoded SHA-256/size annotations;
resolution verifies decoded bytes by streaming and writes nothing; only mounted file-backed
media are decoded into the rebuildable materialized cache, which `image sweep` trims;
imports and publication compress by default, `--no-compress` keeps raw fixtures, and
`image compress` preserves the machine digest;
**the encoder frozen as Zstandard profile 1 with test vectors** (§2), 2026-11-17;
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
Examples are `ibm-pc`, `zx-spectrum`, `c64` and `atari-800`; the list is extensible.

The PC platform id is `ibm-pc`: the IBM PC compatible family unemupc emulates,
from the PC and XT through the AT and PS/2 models to the 386, 486 and Pentium
machines. Displays call it "IBM PC compatible". What a title needs from the
family (a 386, VGA, a Sound Blaster) is a per-title statement, never the
platform. Its first id, `ibm-pc-at`, is a read-only alias kept for good
([decision-32](<../../decisions/decision-32 - The PC platform id is ibm-pc and ibm-pc-at stays a read-only alias.md>)):
local stores keep the artifacts written under it, and they keep their bytes.
Every reader treats the two ids as one platform, in the manifest annotation,
the config and metadata `platform` fields, firmware fit statements and
provider routing, through one normalising function of the shared store crate
(`image_store::platform`); an exact comparison of platform ids is a bug. No
writer writes new bytes naming the alias, also when it derives from an
artifact that names it; compress and catalogue edits reuse the source's
config, which may still name the alias.

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

**Encoder profile 1 (frozen 2026-11-17).** A pinned recipe's lock names the
stored digests of its `+zstd` payloads ([recipes](recipes.md) §3.8, §9 row 4),
so the encoder's exact output binds like a decoded byte does. The `+zstd`
suffix of the version-1 media types *is* encoder profile 1; no manifest byte
names it otherwise (the maintainer's decision, decision-34). Profile 1 is:

| Part | Profile 1 |
|---|---|
| Which payloads | a payload is encoded exactly when its provider declares the `+zstd` variant of its media type and it is at least 4 KiB (`MIN_COMPRESS_SIZE`) and at most that variant's declared decoded-size limit; otherwise it is stored raw. The limits: media sets (`codec::MEDIA_PAYLOAD_TYPES`) floppy 2880 KiB, optical 900 MiB, extra 2 GiB; firmware packages (`codec::FIRMWARE_PAYLOAD_TYPES`) ROM 16 MiB, extra 2 GiB, tar source archive 2 GiB; PC machine images (`pc_image::provider::PAYLOAD_TYPES`) firmware 16 MiB, disk base 16383×16×63×512 bytes, sector pack 16383×16×63×520 bytes, floppy 1440 KiB, tar source archive 2 GiB |
| Library | crates `zstd` 0.13.5, `zstd-safe` 7.4.1 and `zstd-sys` 2.1.3, pinned with `=` in `crates/image-store/Cargo.toml`, default features off; `zstd-sys` builds the libzstd 1.5.7 C sources it bundles, not a system libzstd (`ZSTD_SYS_USE_PKG_CONFIG` stays unset) or another backend |
| Level | 3, with libzstd's own level-3 parameters for the pledged size; no explicit window, strategy or block size, no long-distance matching |
| Threads | one: no worker count is set, so libzstd encodes in the calling thread whether or not its multi-threaded encoder is built (Cargo feature unification could build it without any change to `Cargo.lock`) |
| Frames | one frame per payload, written in one streaming pass with the exact decoded size pledged before the first byte and no flush before the end |
| Frame header (RFC 8878 §3.1.1.1.1) | Content_Checksum_flag set (the frame ends with the low 32 bits of the decoded bytes' XXH64); Frame_Content_Size always written; Dictionary_ID_flag 0, no dictionary; reserved bit 0 |
| Window (RFC 8878 §3.1.1.1.2) | 2 MiB (window log 21), shrunk by libzstd to the smallest power of two that holds a smaller payload: up to 2 MiB a single-segment frame without a Window_Descriptor, above it the descriptor byte for 2 MiB (`0x58`) |

Test vectors pin it (`crates/image-store/tests/zstd_profile.rs`): seven
synthetic payloads built by fixed rules, one per compressed kind (a floppy's
boot sector, FATs and root directory; a tar source archive; a 64 KiB system
ROM; the start of an ISO 9660 image; a sector pack; a text extra; a 3 MiB
FAT16 disk base), sized to cover every row of libzstd's level-3 parameter
table and both frame shapes, each with its frame's length and SHA-256, and
the whole frame of the 244-byte floppy one. Boundary vectors go through the
same import path: 4095 bytes stay raw and 4096 bytes become `+zstd` (frame
pinned), and a media-set floppy of exactly 2880 KiB is encoded while one byte
more stays raw; image-store's two payload-type tables and the threshold are
compared by value. The same file checks the frame header fields above; shows
that changing one setting (level 2 or 4, checksum or content size off, no
pledge, an intermediate flush, window log 20 or 22, long-distance matching, a
dictionary) moves at least one pinned digest; and compares the `Cargo.lock`
entry of every crate named after zstd with the three above (name, version,
crates.io source and checksum exactly; dependency names without version
suffixes), so another version, source or backend fails `cargo test`, and any
other crate named after zstd fails with a message asking for a review. The
`compression` test `linked_zstd_is_the_pinned_release` fails only if another
libzstd *release* is linked: a system libzstd 1.5.7 linked through
`ZSTD_SYS_USE_PKG_CONFIG` passes it, and the vectors are the guard that
catches any library whose frames differ.

Profile 1 never changes. An encoder that writes other bytes for the same
input (another libzstd release, level or option), or that encodes other
payloads (another threshold or limit), is a new profile with its own marker
beside `+zstd`, and every release keeps decoding all profiles.
Failing vectors mean the encoder in use is no longer profile 1, never that the
pins need updating. Decoding is not part of a profile: a conforming decoder
reads every profile's frames, so profile 1 binds only code that writes `+zstd`
payloads.

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
    "unemu.platform": "ibm-pc",
    "unemu.min-app-version": "0.0.3",
    "org.opencontainers.image.base.digest": "<previous-machine-image>"
  }
}
```

Annotations are string values and never bind hardware or media:

| Annotation | Meaning |
|---|---|
| `org.opencontainers.image.title` | On a descriptor: optional logical name for generic OCI tools. On the manifest: a preview title for registry browsing. Informational only. |
| `org.opencontainers.image.base.digest` | Manifest only: image digest this image was derived from. Provenance; never needed to run and never a reason to download an older image. |
| `unemu.platform` | Required on the manifest: machine platform, matching config and metadata (a retired id matches its replacement, §1 Platform neutrality); a routing hint for registry/library previews, never an independent authority. |
| `unemu.redistribution` | `open` or `user-supplied`; absent evidence defaults to user-supplied. |
| `unemu.license` | Reviewed SPDX licence expression for open content. |
| `unemu.source` | On an open ROM, installation medium or installation-set extra: decoded digest of its corresponding source archive in the same closure (§5.3, §5.2). |
| `unemu.terms` | On an open installation medium or extra: decoded digest of the vendor's terms-text extra in the same closure (§5.2). |
| `unemu.provenance` | Publish-safe evidence supporting the open classification. |
| `unemu.pull.registry` | Local only, on a tag's manifest descriptor in the store's `index.json`, never in a manifest: the registry host, port included, that a pull wrote the tag from (§3, §5.1). |
| `unemu.made-here` | Local only, beside `unemu.pull.registry` and never with it: `true` when a write in this library, not a pull, made the tag (§3). |
| `unemu.min-app-version` | Manifest only, optional: the oldest release of the program providing `unemu.platform` that reads and runs the artifact, a SemVer `MAJOR.MINOR.PATCH`. Absent means no requirement (release 0.0.1). A hint read before any strict decode, never an authority and never part of a config (§5.6). |

The config is the single authority for bindings, slots, catalogue order and
media heads; layer descriptors (§4) are the single authority for parents.
The required config `platform` is the authority for routing. Its value must name
the same platform as the metadata envelope and `unemu.platform` annotation before
local tag publication (`ibm-pc-at` and `ibm-pc` name the same one, §1); a mismatch
or missing platform is rejected. Metadata edits cannot change this
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
platform = "ibm-pc"

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

For `platform = "ibm-pc"` (or its alias `ibm-pc-at`), `config.machine` is a flattened schema-1 machine
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
catalogue entry's geometry must match its payload exactly and be one of the
media the drive type takes (`FloppyGeometry::accepted_media`: 720K in a 1.44M
drive, 360K in a 1.2M drive); any other payload that merely fits in the drive is refused.
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
`{"schema": 1, "platform": "ibm-pc", "labels": [...], "titles": [...]}`.
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
  local/machines.toml             machines: named local instances of images (§3.2)
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
`dos-lab:1.0`. A local reference is `[host[:port]/]repository:tag`. As in a
Docker reference, a first component followed by `/` names a registry host when
it contains a `.` or a `:`, or is `localhost`; it is lowercase, with an
optional port, or a bracketed IPv6 address. Images made here (template build,
Save as new image, imports, media and firmware imports) and images pulled from
the family default registry `registry.unemu.dev` are given no host; an image pulled
from any other registry keeps its host, so one repository path on two
registries never shares a local name (§5.1). A person may still type a
host-qualified name for an image made here, as with Docker; the app never
derives one, and the pull record below, not the name, says where a tag came
from. A host has one spelling: lowercase, DNS labels of at most 63
characters, and a port from 1 to 65535 without leading zeros. The name, host
included, is at most 255 characters. Namespace policy reads the repository path after the
host. Pull records its source registry on each tag it writes, as the
`unemu.pull.registry` annotation of the tag's index descriptor, which tells a
short tag pulled from the default registry apart from one made here. Every
other write (import, New machine…, Save as new image, a revision, a retag)
marks a tag it creates or moves to another image with `unemu.made-here` =
`true` instead, also when it copies a pulled tag's descriptor; a pull never
writes the mark and replaces it with its record. Rewriting a tag with the
image it already names keeps its record. Tags written before the store kept
these records have neither: their source is unknown, never guessed, and
they are never renamed. A machine pins its image without either. Names
suggested for images made from a pulled one (Save as new image, Publish
revision) take the family short form, without the host and, when one
component remains, without `unemu/`; an official template pulled from another
registry is still revised under its `-local.N` suffix. Which registry a host-less repository is pushed to
is host configuration (§5.1). Digests
are immutable identities. Resolve a tag once at launch and pin the result for
the session. Unpublished local tag replacement uses compare-and-swap under the
index lock (§3.1). Tags of installation media sets and firmware packages
follow their own rule (§5.2, Tags): a revision tag `[<release>-]rN` never
moves, and the store moves the release alias forward in the same
compare-and-swap that writes a newer revision of it; a pull by an alias
writes the revision it names, with its record, and the alias follows with
the same record. References tracking pushed version tags retain their published digest;
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

### 3.2. Machines: named local instances of images

Decision-29 splits what an image is from what a user runs. An **image** is the
immutable manifest above: shareable, pushed and pulled, named by tags. A
**machine** is a named, local, mutable instance of one image manifest, pinned
by digest and remembered with the `repository:tag` it was made from. Its
changes (disk and floppy journals, CMOS, machine settings, pending inserts)
live beside the image, and it never leaves the local store: machines are never
pushed, exported or pulled. One image can have many machines. The docker
analogy holds: image ≈ docker image, machine ≈ container, Save as new image ≈
`docker commit`, Gaming mode ≈ `docker run --rm`.

**Store layout.** A machine is one record in `local/machines.toml` plus the
workspace of the same id under `workspaces/<id>/`, which holds its session pin
(`session.toml`) and journals exactly as any workspace does (§3, §4). The
record file is versioned (`schema = 1`); a newer or invalid file fails closed
and is never rewritten. Edits reread it under the short index lock and replace
it atomically, like `local/library.toml`; lock order stays store lease,
workspace lock, index lock. Every operation that changes a machine's journals,
settings or pending inserts first takes its workspace lock (`machine::own`),
so a running machine (any process holding the lock) refuses them; a lock held
for an instant by another listing's running probe is retried briefly first.
Rename, favourite and labels change only the record and work while the
machine runs.

```toml
schema = 1

[machines.dos-games]               # the workspace id, fixed for the command line
name = "DOS games"                 # 1–64 characters, unique ignoring case
source = "open-at-games:1.1.0"     # the tag it was made from; empty without one
origin = "new"                     # new | duplicate | migration
created = "2026-11-10 09:12 UTC"   # host text, like a last run; optional
last_run = "2026-11-15 09:41 UTC"  # host text, set at each Start; optional
favorite = true
labels = ["games"]
# migrated_from = "regular-…"      # only while a migration move is pending

[machines.dos-games.image]         # the pinned manifest descriptor, by digest
mediaType = "application/vnd.oci.image.manifest.v1+json"
digest = "sha256:…"
size = 1796

[machines.dos-games.settings]      # optional: provider hardware tables that
# …                                # replace the image's from the next Start

[[machines.dos-games.pending]]     # media for the next Start
drive = "A:"
medium = "disk-1"                  # the medium id in the set's configuration
reference = "wolf3d-shareware:1.4" # for display; the set is pinned below
all = false                        # whether every disk of the set joins the catalogue
journal = true                     # journal in the machine, else read-only
[machines.dos-games.pending.set]   # the installation media set, by digest
# …
```

Times are host text the frontend supplies (the CLI and the desktop read the
host clock for library metadata only); nothing in the execution path reads a
clock (determinism rule). The id is a slug of the name (lowercase ASCII letters
and digits with single hyphens, at most 56 characters, `machine` when nothing
remains), numbered `-2`, `-3`, … when a machine or any workspace directory has
it. Rename changes only the name. A new machine's default name is its image's
title (title annotation, else first catalogue title, else the reference), then
`title (2)`, … . Names compare case-folded (uppercased, then lowercased, so
"Straße" and "STRASSE" are one name) and may not contain control characters
or invisible formatting characters (bidirectional controls, zero-width
characters, the byte order mark, tag characters); Unicode normalization (NFC)
is not applied yet. An id never starts with `regular-`, `gaming-`,
`disposable-` or `fresh-`, the prefixes the desktop and Gaming mode give their
own workspaces: such a slug gets `m-` in front ("Fresh install" is
`m-fresh-install`). The record pins a manifest descriptor without the local
tag annotation; a machine never follows a tag.

**Derived state** (the library's State column), read without taking the
lock: *Running* when another owner holds the workspace lock; *Changed:* the
drives (in drive order: A:, B:, C:, D:) whose journals hold written sectors,
then *CMOS* when the CMOS journal's content differs from the image's CMOS, then
then *inserted media* when the journal of a disk inserted from an installation
set holds writes (listed as changes with drive `inserted`), then
*settings* when an overlay exists; *Never started* without a last run;
otherwise *Same as image*. The listing also gives the journal bytes on the
host, the tags that name the image now (none: **detached** — the image stays
in the store for the machine; blobs are never collected while referenced), the
image the source tag names now when it **moved**, pending inserts, a pending
publication, and a *problem* when that machine's workspace or image cannot be
read (the other machines list normally).

**Operations** (`pc_image::machine`, mirrored by `image machine …`):

| Operation | Behaviour |
|---|---|
| New machine from image | Record first under the index lock (name and id reserved), then the workspace is pinned to the image at once; nothing is verified or materialized until Start. A failure removes both. |
| Rename | The name only. |
| Duplicate | Copies the session pin and every journal (never locks, the desktop's record, publication staging or journal temporaries) into a new workspace, with the record's settings, pending inserts and labels, on the same image. Not the favourite or last run. The copies are independent. Refused while a publication is pending. |
| Reset to image | Discards the workspace contents under its lock (§3.1 discard), drops the settings and pending inserts, and pins the workspace again. Name, id, favourite, labels and last run stay. |
| Delete machine | Removes the workspace, then the record. The image and its tags stay. A crash between the two leaves a record without a workspace, which starts again from its image. |
| Settings | Checks the hardware tables as a revision of the image's (`revision`: firmware and drive slots rebound, defaults frozen) and keeps them as the machine's overlay from its next Start; a firmware choice binds the ROM of a firmware package in the local store at every Start and in Save as new image…; the overlay records each such ROM's package digest and layer descriptor (`bound_firmware`, never shown as a hardware table), so the ROM still binds by digest after the package loses its tag; a system BIOS of another CMOS layout, or an unlisted one, resets the machine's stored CMOS to the bytes derived from its configuration (`SettingsChange::cmos_reset`; the same layout keeps it), announced by `media/cmos-reset.pending` before the record changes, which Start (and Save as new image…) finishes after an interruption; current tables that no longer bind fall back to the image's machine with an unknown layout, so clearing always works; tables that lower to the image's machine, or an explicit clear, drop the overlay. The record is written first and goes back if the CMOS journal write then fails; a SETUP field that cannot be derived keeps the stored bytes, with a note. A field AT SETUP records in CMOS (memory, drives, equipment) is recomputed as Settings on an image recomputes it (§2, revision): only that field and the checksum change, written into the machine's CMOS journal when the image journals CMOS; a stored CMOS image without a journal is recomputed at every Start; derived CMOS follows the configuration. The image never changes. |
| Save as new image | Offline whole-machine publication (§4) of the journals with the overlay as the new configuration, titled with the machine name by default. By default the machine moves onto the new image with fresh, empty journals and no overlay; with *keep on old image* the publication intent records it, the workspace keeps its image, journals and overlay, and the new image stands alone. Writes to disks inserted from installation sets are never part of the image; the command line warns about them, and moving onto the new image starts their journals afresh too. |
| Start | Two steps. `prepare` opens the workspace's session on the pinned image (the session pin wins over the record, so a rebind from elsewhere is honoured) with the overlay and puts each pending insert into its floppy drive (chosen disk first, then the set's others when all join; journals named `<set hex>-<medium id>.upfj` as a live insert from the library names them); it changes no record. `confirm`, called once the machine has assembled, records the last run and the pinned image and drops the inserts the launch put in, so a failed assembly keeps both. A pending insert lasts until that Start; afterwards the image's catalogue applies again. |

`image run NAME:TAG --workspace ID` on a machine's workspace starts it as the
machine (overlay, inserts, last run) after checking that the tag names the
pinned image. `image publish --from-workspace` and `image workspace discard`
refuse a machine's workspace and name `image machine commit` or `reset`,
which keep the record and the settings overlay consistent. `image sweep`
keeps every machine's image.

**Migration.** Opening a store for machine work (`machine::open_store`, every
`image machine` command) first adopts older workspaces, idempotently and
without losing data:

- A desktop workspace (`regular-…` from its image digest, or the older
  `gaming-…`) becomes a machine named after its image's title (then
  `title (2)`, …) with an id from that name, and takes the image's favourite
  star and last run. Its directory moves to the new id under its workspace
  lock and the namespace lock; the desktop's own record of its sessions
  (`desktop.toml`) is dropped, since the sessions are the machine's now. The
  record, written first, names the old id in `migrated_from` until the move is
  made, so an interrupted move completes on the next migration; an empty
  directory left at the id is removed first, one with data is never touched.
  Once the move is made the record forgets the old id.
- A workspace named on the command line keeps its name as both id and name.
- Unfinished Gaming-mode workspaces (`disposable-…`, `fresh-…`) stay as they
  are.
- A workspace in use waits for a later run. A machine whose move is still
  pending because its old workspace is in use refuses every change and Start,
  without ever making an empty workspace at its id.
- A damaged workspace is reported (`migration-skipped:NAME`) and skipped; the
  others migrate.

**Desktop library** (approved machines and media-picker mock-ups,
2026-11-15). The library worker opens the store and runs the migration once
per start; workspaces it adopts are named in a one-time notice above the
Machines list. The sidebar is one Library group: Machines (with Recently
played and Favourites, which list machines), Images, Installation media,
Firmware and Machine files; the selected collection's facets sit under it
(machines filter by their own labels and their image's catalogue metadata).
The Images, Installation media and Firmware lists show each tag's Source
after its name: the registry host its pull recorded (`unemu.pull.registry`),
`local` for a tag a write here made (`unemu.made-here`), and a dash for a tag
with neither record, whose source is unknown and never guessed. A Remotes group follows Library (approved remotes mock-up,
revision 2): one node per registry of Preferences › Registries, the default
registry first and `registry.unemu.dev` always; under an open registry its
namespaces, a private one locked until a login lists it; under an open
namespace one group per artifact type with repositories (Machine images,
Installation media, Firmware). Nothing contacts a registry until its node is
expanded or selected, and every request runs on the library worker (§5.1).
A node's list has the mock-up's columns (Name, Source, Title, Latest, Type,
Licence, Size, Local state); a repository of several tags expands into them,
newest first: a machine image's by version, then `latest`; a set's or
package's by release, the alias of each release first as the name of its
newest revision, then its revisions highest first (§5.2, Tags), so a set
repository stands for the alias of its first release. Newer available
compares versions of a machine image, and only the revisions of one release
of a set or package. A local release alias at another image than the
registry's alias is compared by the revision it names with the highest
revision of that release the registry lists: Newer available when the
registry's is higher, "Local is newer: …-r2" when the library's is, and
otherwise, or for an alias an earlier release wrote that names no revision,
the neutral Local tag names another image. A tag this release cannot run is
muted with "Needs unemupc X or newer" (§5.6). Strips over the rows say a listing runs, stopped or no
longer answers, and a message replaces the rows when nothing is listed, with
Stop, Refresh, Try again or Log in… as the state offers. Next 50 › reads the
catalog's next page; F5, View › Refresh remote and the toolbar's Refresh
list the shown registry again, and the toolbar's Registries… opens
Preferences › Registries; every start of a machine stops every listing
first. A repository row stands for the newest version this release runs:
`latest`'s version when it is one of the 16 newest and runs here, else the
newest one that runs here; the listing reads versions newest first until it
reaches that one, at most the 16 newest, the row names the newer versions
that cannot run here and why, and only a repository with no such version
that runs here is muted (§5.6). A tag runs here when it is an IBM PC
compatible artifact (`ibm-pc`, or the retired `ibm-pc-at`) of the PC
provider's three types and its release requirement is met; another
family's image is muted with the reason and cannot be pulled. A login on a registry Preferences does not list says Add
registry…. A click selects a tag; Ctrl (⌘ on macOS) adds or removes one and
Shift selects a range. The details pane of one tag shows the manifest's
annotations (title, vendor, description, licences, source), its digest
("Pull uses this digest") and type, the edition and version its metadata
record adds once selected, a media set's disks, the local tag a pull writes
and its source, the tag's local state, what a pull downloads with the blobs
the store holds left out, and the repository's tags, each a link. Its
buttons: Show in library (for a tag in the library), Pull…, Pull and create
machine… for a machine image (New machine from image… once it is in the
library), Pull and insert… for a set while a stopped machine has a drive
that takes every disk (Insert…, the media picker on that drive, once it is
in the library), and Contents…, which lists every descriptor of the manifest
with its size, redistribution mark and whether the store holds it, all read
from the manifest. A tag this release cannot run, one of a registry that no
longer answers, or one already in the library cannot be pulled, and the pane
says why under its buttons. For several tags the pane lists them with their
local states and offers Pull… for those it can take. **Pull…** (also Image ›
Pull…, the toolbar's Pull…, shown while the library shows Remotes in the room
of the speed control, and Enter on a tag's row) pulls by the digest the
list showed through the Pull path, so the local tag is written last (§3.1),
and moves no local tag but one: a set or package pulled by its release
alias writes the revision the registry's alias names and moves the local
alias of that release forward to it (§5.2, Tags), so an alias row behind
the registry's is pulled under its own name. Any other tag whose local name
holds another image goes alone, under another local tag the dialog proposes
(`name-new:tag`), and a typed tag that names another image is refused before
anything is fetched.
Several tags are pulled one after another under one progress bar that
counts blobs already in the store as done; tags in the library, tags whose
local name holds another image, tags this release cannot run and tags of a
registry that no longer answers are skipped, each with its reason. Cancel or
Escape stops the running pull at its next read and the waiting ones before
they contact the registry. Pull and create machine… then opens New machine
from image… with the manifest's title as the name; Pull and insert… then
leaves the set's first disk pending in the chosen drive for the machine's
next Start. After a pull the library refreshes and the row turns In library
from the store, without a new listing. Image › Pull by reference… pulls an
image named by its reference, as Image › Pull… did before. The
library opens on Machines. Every machine change runs on the worker through
`pc_image::machine`, never on the frame loop:

- *Start* on a machine runs it in its own workspace (`own`, `prepare`); the
  shell reports when the machine has assembled and only then the worker
  `confirm`s the run, so a failed assembly keeps the pending inserts and the
  last run. Start on an image without a machine creates one named after the
  image and starts it; with machines it opens Start… with the last-run
  machine preselected and New machine last.
- *Gaming mode* on a machine plays a disposable copy of it as it is
  (`prepare_copy`: its journals and session pin are copied, its settings
  overlay and pending inserts apply, nothing is confirmed); on an image it
  plays a fresh copy of the image, as before. The copy records what it runs
  with (`copy-of.toml`: the machine, its tag, labels, settings overlay and
  pending inserts), so later changes to the machine never reach it. When it
  ends: Discard, Keep as new machine… (`keep_copy`: the disposable workspace
  is renamed to the new machine's id, which takes the recorded settings and
  labels) or Keep as new image… (`publish_copy`, with the recorded
  settings); a power cycle starts it again the same way (`resume_copy`). The
  desktop record of a copy names its machine (`machine` in `desktop.toml`).
- *Settings…* on a machine opens the approved property sheet over the tables
  the machine runs with, marks what differs from the image and keeps the
  result with `change_settings` (OK, Apply); Image › Settings… still makes a
  new image revision. *Save as new image…* asks whether the machine moves
  onto the new image (default) or keeps its changes on the old one, with the
  repository defaulting to the machine name's slug at 1.0.0 and the title to
  the name; an unfinished save is finished from Disk layers' form. *Disk
  layers…* on a machine shows its chains with its working changes; its
  Discard changes resets the machine and its Save as new image… commits it.
- *Insert…* of a drive opens the media picker. Into a stopped machine it
  stores a pending insert (`set_pending_inserts`, replacing the drive's
  earlier one); the drive chips and the Media menu show it "at Start" and
  Take out again removes it. Into a running or halted machine the chosen
  set goes in at once through the existing library insert path. The
  Installation media collection's Insert into… lists the drives of every
  machine that take the set and opens the picker there. Pending disks change
  one drive at a time (`set_pending_insert`, on the record as it is). A disk
  the library refuses (a damaged set, a busy store) only says why; the
  machine keeps running.
- *Unfinished saves.* An interrupted Save as new image… is finished with the
  choice its intent recorded (`pending_commit`, `finish_commit`): moving onto
  the new image, or keeping the changes on the old one.
- *While a machine runs*, the worker still answers machine jobs: Settings…
  opens read-only, Rename… and New machine… work, and a change that needs
  the running machine's lock is refused with a reason.
- File › New machine… builds the image and, unless its Name and tag step's
  check box is cleared, a machine from it (named after the image title
  unless named there).

*Rolling back* to a build without machines: rename each machine's directory
`workspaces/<id>` back to a `regular-…` name (any `regular-` name works; that
desktop finds its workspace by the pinned image) and remove
`local/machines.toml`; settings overlays and pending inserts, which that
build does not know, are lost, journals are not.

Out of scope, as decision-29 says: rebasing a machine onto a newer image or tag,
machine export and import, machines on a registry, and the full insert flow
for a stopped machine (only its stored result, the pending insert, exists here).

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
| Save as layer… (commit layer; the Disk layers button reads Save as new image…) | Stop the machine, finish all active media, seal each changed fixed-disk overlay into a parent-bound layer, freeze each changed floppy or CMOS to a new raw blob and publish a complete new manifest. Media without changes keep their head. The user names a new version/tag or moves an eligible unpublished local tag by compare-and-swap; pushed version tags remain immutable. Parents remain unchanged; the workspace then pins the new image and starts fresh journals. |
| Discard changes | Stop and release writers, then remove only this workspace's changes after showing affected media. The pinned image still exists. |
| New image from here… | *Designed, not built yet; no menu or button offers it.* Select a complete historical manifest and fork a workspace at all of its heads; picking only one disk is an explicit advanced operation with a mismatch warning. |
| Export new base | Stream the selected chain into a new raw file, verify it, and publish without replacing any original. This is the existing media-tool meaning of commit/compact. |

The Disk layers dialog labels commit layer **Save as new image…** (it was
**Save as layer…** until 2026-11-16), distinct from **export new base**. An eject still closes a recovery generation; it does not
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
  and refuse a different digest for immutable tags (version tags, and the
  fixed tags of installation media sets). Moving aliases follow the
  explicit-force policy below. This check and its repeat just before the
  `PUT` send every manifest and index type in `Accept` (OCI image manifest and
  image index, Docker schema 2 manifest and manifest list): a registry may
  answer `404` for a tag whose type the request leaves out (distribution
  does so for an OCI image index) or substitute one platform's manifest for a
  list, and a push would then take an occupied tag for a new one. Pull, which
  decodes the manifest, still asks for the OCI image manifest only.
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
  the default host. A pull from `registry.unemu.dev` writes the short local tag
  (`open-at:1.2.0`, `unemu/media/ibm-pc/freedos:1.4-r1`); a pull from any other registry,
  a configured default one included, writes the full reference with its host
  and port (`registry.lan.example/private/games/dune2:1.0.0`,
  `nas.example:5000/private/media/x:1.1`) and records the registry on the tag
  (§3). Push of a host-qualified local tag goes to its own host unless a
  target reference is given; push of a short tag goes to the default host.
- The registries list in preferences holds entries of a host, an optional
  login user and its privacy, never a secret. Privacy is set for the whole
  registry with a `private` flag, or per namespace (the first path component
  of a repository) with a `namespaces` list, each namespace public or private;
  an entry with namespaces carries no `private = true` of its own. One host can
  so serve a public `unemu/` and a private `private/`:

  ```toml
  [[registries]]
  host = "registry.unemu.dev"
  user = "NAME"
  namespaces = [{ name = "unemu", private = false }, { name = "private", private = true }]
  ```

  `unemu/` is never private: an entry that marks it private is refused when
  preferences are read. The default registry is known without an entry and is
  public until marked private. When the public namespace moves to another
  host, `private/` keeps its entry and the new host's entry lists `unemu/`
  only. The command line sets privacy with `image registry set`, the desktop
  shell with Preferences › Registries; both write the same keys.
- Push refuses a registry that is neither listed nor the default, and, on an
  entry that lists namespaces, any repository outside them, whatever the image
  holds ("X/ is not a configured namespace on this registry"). A repository is
  in a namespace only below it: `private/games` is in `private/`, a repository
  named `private` is not. A blob's effective class is the most restrictive one
  recorded for its digest in any local manifest (§2). An image with any
  user-supplied blob (the default classification) goes only to a namespace
  marked private, or to an entry without namespaces marked private as a whole,
  and never into the official `unemu/` namespace on any host; pushes into
  `unemu/` require every blob to be open. Before such a restricted push, probe
  anonymous `tags/list` and catalog access for the target namespace, not just
  `/v2/`: `403`, or `401` with a Basic challenge, indicate private access;
  `200` or `404 NAME_UNKNOWN` indicate open access, and a `401` Bearer
  challenge is inconclusive; both refuse the push. There is no override.
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
- **Version tags are immutable, and so are the fixed tags of installation
  media sets.** If a tag of semantic-version shape already names another image
  digest, refuse the push even with force; retrying the same digest is
  idempotent. A tag of an installation media set that is not an alias (§5.2)
  is refused the same way, whether the set is the image pushed or the one the
  tag names now, and the refusal suggests the next revision tag. Any other tag
  moves only after an explicit confirmation showing the old and the new
  digest. OCI has no tag compare-and-swap:
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
- The desktop shell's Image › Push… and Image › Pull by reference… follow
  the approved registry-accounts mock-up; Pull… of the tags selected in the
  library's Remotes follows the remotes mock-up (§3.2, Desktop library). Push… shows the dry run of the local image (every
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
platform = "ibm-pc"

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

**Manifest annotations** (implemented 2026-11-15). Registry web UIs read the
OCI image-spec's pre-defined annotations, so a set's manifest carries them,
filled from its one title record like a firmware package's (§5.3) from its
config:

| Annotation | Value |
|---|---|
| `org.opencontainers.image.title` | The title. |
| `org.opencontainers.image.description` | The description. |
| `org.opencontainers.image.vendor` | The publishers, joined with `, ` in the order stated. |
| `org.opencontainers.image.authors` | The developers, joined the same way. |
| `org.opencontainers.image.licenses` | Open sets only: the conjunction of the layer licences (below). |

A field the record leaves empty adds no annotation, so a set imported without
catalogue fields keeps its manifest bytes (a test pins a default set's
digest). There is no `org.opencontainers.image.created`, because an import has
no clock, and no `org.opencontainers.image.url`, because §2.1's title record
has no URL field. Like every annotation (§2) they are informational, never an
authority. They are part of the manifest and so of the set digest: when the
tag exists, a stored set whose title, description, vendor or authors
annotations differ from those the import writes (one stored before they
existed, for example) is a tag conflict, and an identical re-import is
already imported.

All descriptors default to `unemu.redistribution = "user-supplied"`, including
metadata/artwork, unless reviewed licence/provenance proves them open (§5).
The most restrictive local classification for a digest still wins. Restricted
sets stay in the private store/registry and private archives, never repository
fixtures or public exports. §5's future public recipe strips restricted bytes;
it never silently exports the original private manifest.

**Redistributable sets** ([decision-26](<../../decisions/decision-26 - Installation media sets are their own OCI artifact type.md>)
amendment 2026-11-15, implemented 2026-11-15). A set is open only by an
explicit statement at import: `image media import … --open --license
LicenseRef-<name> --license-text <EXTRA>`. The licence is one SPDX
`LicenseRef-` that names the vendor's own terms; an identifier from the SPDX
License List is refused, because a set cannot carry the source an
open-source licence may oblige, unless the set names its source archive
(Free-software sets with their source, below). `--license-text` names, by file name, exactly
one of the set's `--extra` files: the vendor's terms text as released.
`--open` needs both flags and each flag needs `--open`; nothing is guessed
from file names or contents. The statement uses the `image_store::license`
machinery of §5.3:

- Every medium and every extra layer is marked `open` under that licence and
  carries `unemu.terms`, the decoded digest of the terms-text extra (the
  counterpart of a ROM's `unemu.source`; the terms extra links itself).
- The config and the metadata, which this program generates, follow the
  firmware-package rule: open under `CC0-1.0` because every other layer is
  open. They carry no terms link: the amendment's "every layer records the
  digest" is read as every layer of the vendor's bytes, and the generated
  metadata layer is this program's own CC0 output (the title and labels the
  user typed), not the vendor's work. The manifest adds
  `org.opencontainers.image.licenses`,
  the conjunction of the layer licences (for example
  `LicenseRef-<name> AND CC0-1.0`).
- Without the statement every descriptor stays user-supplied, byte for byte
  as before (a test pins a default set's manifest digest). Marks are part of
  the set: when the tag exists, the same bytes under other marks, another
  licence or another terms extra are a tag conflict, so a set is opened by a
  new import under a new tag.

*Push guard, rule 5.* Open installation media go to a registry not marked
private, or into `unemu/`, only with their terms text: in a media set every
open layer except the generated metadata must link, through `unemu.terms`,
one and the same digest (the first link in manifest order names it), and
that digest must be an open extra layer of the same closure. A missing or
unparsable link, a link to another digest, or a terms text that is absent,
not an extra or not open refuses the push and names the layers; a registry
marked private outside `unemu/` keeps the user's own copy. The refusal's hint
says to import the set again under a new tag with the statement. A
user-supplied set falls under rules 1 and 2 and never enters `unemu/`; for a
media set their refusals add the same hint. Outside a media set
the rule checks only the links it finds: any open layer that links a terms
text needs that text, open, in its closure. The status is
`image_store::license::terms_unlinked`, or `guard::terms_without_text` with
the guard's effective classes. `image media show` prints the terms extra each
link names, every layer's class, licence and link, and the guard's verdict
from its own classification.

**Free-software sets with their source** (2026-11-15). An open set may also
be stated under an SPDX licence expression instead of a vendor's
`LicenseRef-`: `image media import … --open --license <EXPRESSION>
--license-text <EXTRA> --source <EXTRA>`. `--source` names, by file name,
exactly one extra of the set: the archive of its corresponding source, an
extra of kind archive (`.zip`, `.tar`, `.tgz`, `.gz`, `.7z`, … by extension)
other than the licence text. An expression that is not one `LicenseRef-` is
refused without `--source`,
copyleft or not, with the same "the vendor's own terms" refusal as before;
the `LicenseRef-` path is unchanged and may name a source archive too. Every
medium and extra then also carries `unemu.source`, the decoded digest of
the source extra (which links itself); the config and metadata carry no
source link. Marks are part of the set, so another source extra under the
same tag is a tag conflict. `image media import` prints `source "<name>"`
after the `open` line; `image media show` prints a `source` line beside the
`terms` line.

*Push guard, rule 4 for media.* Rule 4 of §5.3 covers installation media
too: an open floppy or optical medium, and an open extra of an installation
set, whose licence obliges the source under the active policy (any GPL,
LGPL, AGPL or MPL identifier) goes to a registry not marked private, or into
`unemu/`, only when its `unemu.source` names a layer of the same closure
that is an extra, a tar or a zip. A missing, unparsable or dangling link
refuses the push (`CopyleftWithoutSource`, worded "open copyleft media", with
the hint to import the set again with `--source EXTRA`); a private registry
outside `unemu/` keeps the user's own copy. Outside an installation set a
copyleft medium still needs its source (a machine image binding such a disk
must carry the set's source archive), and so does an extra that links one;
an unlinked extra there is a document beside a firmware package's ROMs or a
machine's media and is not checked. The set rule needs the set's config
descriptor in the closure, which the guard always passes. A machine that
misses both a ROM's and a medium's source is worded "ROMs and media". The
refusal hints name `LicenseRef-NAME`, or an SPDX `EXPRESSION` with
`--source EXTRA`. ROMs keep their own rule: their
archive is a tar or zip layer. Rule 5 is unchanged: an open set's layers
also link its licence text. `image media show` reports the case as
"private registries outside unemu/ only: open copyleft media without the
set's source archive". Unit tests cover the guard with synthetic sets;
`image_store::license::source_missing` is the shared status.

*Rights follow the bytes.* The most restrictive local mark per digest still
wins: when another local tag records one of the set's media or its terms
text as user-supplied (the same disk imported without `--open`, or a machine
image built from the disk file), the open set stays private and `image push
--dry-run` names that tag. A new import cannot clear such a record: the
refusal says so, and only deleting that tag (the library's Delete tag) or
pushing from a store without it does. A machine image that embeds an open set's media is
open only when every other layer is open; machine import still marks every
floppy user-supplied, so today such an image stays private, and its floppy
records restrict the set's media in the same store. The media themselves
hold the vendor's unmodified files, pinned by SHA-256 in the fetch scripts
under `vendor/shareware/`; game files and terms texts never enter the
repository.

**Read-only bases, writable installations.** Inserting a set from the
library (the media picker's This library) resolves a set tag once, verifies
it and offers loading the whole set into the
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
policy (extended by the tag rule below), upload resumption, complete-closure
verification and tag-last publication apply to sets. Private OCI archives carry the same verified closure.

**Tags.** Set and package tags name a release and a revision
(recipes.md §8.2, decision-38): `[<release>-]rN`, where the release is parts
joined by `-`, each `[a-z0-9]+([._][a-z0-9]+)*` and none `r` plus digits
(`freedos:1.4-r1`, `1.4-720k-r2`, `contra:usa-rev1-r1`, a package's `r1`).
A tag that ends in `rN` names one set or package forever: neither the local
store nor a push moves it, not even with `--move` or a confirmed current
digest, no set or package takes one from another image, and a deleted one
names no other image, as a deleted version tag; the refusal suggests the
next revision (`1.4-r2`). The release alone (`freedos:1.4`) is an alias for
the newest revision of that release, also when the release is shaped like a
semantic version (`1.0.0`): the version rule of machine images does not bind
sets and packages. The store writes a new alias only at the newest revision
of its release here (a pull of a registry alias published before revisions
excepted) and moves it in the same compare-and-swap, under the index lock,
that writes a newer revision; an explicit move goes only forward to a higher
revision of the same release that a local revision tag names, never backward
or to another release; a push moves it only to the newest revision of its release that the
registry already has, after the usual confirmation (§5.1; the client lists
the repository's tags and compares the digest each revision of the release
names). Sets and packages have no `latest` and take no new tag outside the
scheme. The parts of a release are the
author's convention, not fields: nothing reads a version or a format out of a
tag. Tags an earlier release wrote still read as they are: `freedos:1.4` is
an alias without revisions, which stays where it is, `1.4-r2` a revision,
and a firmware `:1` an alias of release `1`. Machine images keep the
version-tag rule alone. Unlike a version tag, a deleted set tag is not
remembered in the local store; the registry still refuses to move it.

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
Since the approved media-picker mock-up (2026-11-15) one Insert… per drive
opens the media picker, which replaces Browse / catalogue… and From
library… (the picker's This library source runs the same validation and
load, without its dialog); the pane's Insert into… lists the drives of the
library's machines that take the set and opens the picker on one (§3.2,
Desktop library). Add to machine… stays disabled: the store keeps no
machine catalogues yet.
The dialog and `image media import` share one plan-and-publish function and its
refusals. Both write the set's one title record: title, publishers, genres,
release year, software platform and description, and on the command line also
developers (`--publisher`, `--developer`, `--genre` repeatable in order,
`--year`, `--platform`, `--description`, implemented 2026-11-15). The shared
plan refuses a release year outside 1 to 9999 and a software platform outside
§2.1's controlled values, the choices the dialog offers; reading keeps other
platforms for sets made elsewhere. When the tag exists, the window writes nothing: it resolves the stored
set, refuses at once on another artifact type or a different layout, metadata
or descriptive annotation (above), then compares the sources' raw SHA-256
with the stored decoded digests, so a set stored compressed or raw reports
already imported and any other content a tag conflict (the command line keeps refusing an existing tag
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
platform = "ibm-pc"
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
and publishes a new revision under a new tag (`:rN` to `:rN+1` by default; a
package an earlier release tagged `:N` goes to `:rN+1`) that
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
`unemu/`. Rule 4 checks ROM layers here; a package's documents and archives
fall under the user-supplied rules, and a copyleft extra of a firmware
package or a machine image is checked only when it links a source archive
(§5.2, rule 4 for media). Its status for one layer is
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
`:rN+1` through `firmware_import::edit`. Neither dialog decides a rule: the
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
| `unemu/firmware/<platform>/<catalogue-id>` | the open firmware package of one catalogue entry |

Both follow the OCI repository grammar (`[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*` per
component), so catalogue ids such as `kiselev-xi8088-1.0.2-atreset` need no
mapping. Each template generation has a semantic-version tag (`1.0.0`), which
never moves, and `latest`, which follows the newest version. The local tags of
a template build are the short forms a pull from `registry.unemu.dev`
writes: `open-at:1.0.0` for `unemu/open-at`. Sets and packages carry the
platform in their path (recipes.md §8.3, decision-35), so one work on two
platforms never shares a repository. A firmware package's tag is its
revision, `unemu/firmware/ibm-pc/<id>:r1` (several components remain after
`unemu/`, so the local tag keeps the full name), and without a release it
has no alias. Installation sets live under `unemu/media/<platform>/<work>`,
one repository per work, tagged with the release (the version and, unless
it is the edition's own format, the floppy format) and the revision:
`unemu/media/ibm-pc/freedos:1.4-r1`, `:1.4-1.2m-r1`, `:1.4-720k-r1`, each
with its release alias (`:1.4`, `:1.4-1.2m`, `:1.4-720k`). Neither a package
nor a set has `latest`, and no revision tag moves (§5.2, Tags). Only the
official builders write these `unemu/` names. Local imports (`image firmware
import`, `image media import`, the library's import dialogs and Create from
files) name their result under `private/` by default
(`private/firmware/ibm-pc/ibm-5170-bios-1985-06-10:r1`), open or not, so a
local import never takes the name a pull of the published artifact writes;
an explicit tag still wins, and the guard decides where a push may go.
Future software images get their own names when their licence decision is
made.

**Who publishes what (2026-11-16).** The firmware packages and the
installation sets are built and published by the unemu-media repository
(<https://github.com/unemu-org/unemu-media>) with a pinned `upcli` release;
`image template build` builds the template machines only.
`pc_image::template::build_firmware` and `build_media` build the same images
locally from this repository's fixtures, under the published tags and with
the values `image firmware import` and `image media import` take there; their
tests pin the published digests, so each pin cross-checks the two
repositories. A template binds the packages by digest, so its digest does not
depend on where the packages came from. The packages and sets published so
far name the retired id `ibm-pc-at` (§1) and the names before the platform
segment and revisions (`unemu/firmware/<id>:1`, `unemu/media/freedos:1.4`);
before the first public release they are deleted and pushed again under the
names above as their `ibm-pc` rebuilds, whose digests the local builders'
tests pin (decision-32, decision-35, decision-38). The names change no
manifest byte, so those pins did not move with them.

**Installation sets (2026-11-15).** The templates describe the official
**FreeDOS 1.4 Floppy Edition** (`vendor/freedos/fetch-floppy-1.4.sh`,
`pc_image::template::MEDIA`) as one set per floppy format it ships:
`unemu/media/ibm-pc/freedos:1.4-r1` (the edition's 1.44M, 7 disks),
`:1.4-1.2m-r1` (8) and `:1.4-720k-r1` (12), each with its release alias,
each disk byte for byte the edition's, in
installer order (`x86BOOT.img`, then `x86DSK01.img`…), labelled "Disk N of M
- boot disk and installer (x86BOOT.img)" or "… - packages (x86DSKnn.img)".
Every set carries the same two extras: `LICENSES.TXT`, kind document, a
licence index this repository writes and dedicates under CC0-1.0 (each
program's licence with the notice it rests on, and the quoted terms of
`LicenseRef-FreeDOS-other`); and `freedos-1.4-floppy-src.tar`, kind archive,
a deterministic tar
of the official FreeDOS 1.4 source packages of every program on the disks
(the release LiveCD's package zips with their `SOURCE/` trees, mTCP for
FDNET's DHCP, FDBANNER) and of the installer at its `FreeDOS-v1.4` tags
(FDI-x86, FDI, the fd-nls translations and fonts it uses, the release
builder). The release documents (`readme.txt`, `announce.html`,
`changes.log`, `report.html`, `verify.txt`) state no licence of their own and
are not carried; the edition's documentation travels inside its disks and
the source tar under each package's licence. The title record states the
work's title only, "FreeDOS" (the version, the edition and the format have
fields and a tag of their own), publisher and developer "The FreeDOS
Project", year 2025, platform DOS, genre "operating system", version 1.4,
edition "Floppy Edition", a description, and labels `freedos`,
`open-source`, `operating-system`; the manifest carries the OCI title,
description, vendor and authors annotations from it. Every fixture is checked
against its pin before an import, so other bytes at a fixture path are
refused.

The sets are imported (locally by `pc_image::template::build_media`, which
checks every set fixture before the first tag) open under the
conjunction of their programs' licences, `GPL-1.0-only AND GPL-1.0-or-later
AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND
LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND
Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND
LicenseRef-FreeDOS-other` (exactly the 256-byte statement limit; the non-SPDX
terms named together and quoted in `LICENSES.TXT`; a test keeps the code and
the index equal), with `LICENSES.TXT` as the licence text and the source tar
as the source archive (`--license-text` and `--source` of §5.2). Each set goes
through the guard of `unemu/` before its manifest is stored (rules 4 and 5
pass: every medium and extra links both), tagged with its own tag only. The
template machines' closure did not change with them, so
their generation stayed `1.1.0` with its pins (it is `1.2.0` since, below); a
separate test pins the three sets' published digests. Each program's licence is read
from its own notices (main source headers, licence files) and from the LSM
only where those name no version; the narrowest grant of a package is listed.
Two parts are read closely and are inside the claim: the cpidos fonts ship as
UPX-packed CPI files, the form their author edits (`SOURCES.TXT`), so each CPX
carries its source losslessly; the zlib 1.2.3 library SHSURDRV links is
modified and marked as such (`ZLIBCDRD.TXT`), and the zlib licence obliges no
source. The library's Remotes list the sets under `unemu/` › Installation
media, apart from the machine images Get templates… shows.

**First generation (1.0.0).** `open-at`: an IBM 5170-class board with the
Kiselev AT-reset BIOS, the unemu VGA BIOS and XTIDE Universal BIOS r638 (each
with the source archive the catalogue pins), 640 KiB + 1 MiB, VGA, a COM1 UART
without a mouse, one 1.2M drive with an empty catalogue, a blank zero-base 504
MiB (1024/16/63, the largest INT 13h CHS disk) ATA master at 1F0h, no sound
card. `open-at-games`: the same plus the cards of `machines/at-games-open.toml`
without its media: SB16 at 220h (IRQ 5, DMA 1 and 5), a Microsoft mouse on
COM1 and a 2 MiB Lo-tech EMS board at 260h with its frame at D000h. Neither
holds an operating system or other software: disk images with an operating
system need their own licence decision. Generation 1.0.1 changed only the
VGA BIOS package's source archive. **Generation 1.1.0** replaces the system
BIOS package with the open AT BIOS, `unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.2`
(the unemu fork of the Kiselev BIOS, with its source archive) and adds an
empty 3.5-inch 1.44M B: drive (unit 1, which also takes 720K disks) beside the
1.2M A:; the rest of the hardware is unchanged. Its firmware packages also
state a title, description, vendor, version and labels; earlier generations
had only the catalogue id as their title. **Generation 1.1.1** replaces the
system BIOS package with `unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3`, the
fix release whose POST clears extended memory (so FreeDOS keeps its XMS after
Ctrl+Alt+Del), and moves the VGA BIOS package to the source archive of
release `v0.8a-unemu.3`, with unchanged ROM bytes; the XTIDE package and the
hardware are unchanged. **Generation 1.2.0** replaces the VGA BIOS package
with `unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std`, the fork's release based on
upstream 0.9d whose teletype BEL sounds the PC speaker (the 0.8a entry stays
catalogued as superseded), and moves the system BIOS package to the
re-measured source archive of the same release, whose `NOTICE` now names the
fork's public repository; the system BIOS ROM, the XTIDE package and the
hardware are unchanged. Generation 1.1.1 was never published; it is replaced
rather than re-pinned because a local store may already hold it.

**Build.** `image template build` (commands.md, Official template images)
imports the three packages from the fetched fixtures with their pinned source
archives (`pc_image::template::build_firmware`, as local copies of the
published packages under their published tags, with no output line and no
guard check of their own), then creates each machine through the New machine create API
(§5.3, `pc_image::create`): zero-base blank disks, a generated configuration
(CC0) and the packages' ROM and archive blobs bound by digest. Titles and
descriptions are manifest annotations. Each package states its title,
description, vendor, version (the release tag or revision its catalogue
provenance names) and labels through the same statement as the metadata flags
of `image firmware import`, so its config and README carry them and its
manifest carries the standard `org.opencontainers.image.title`, `.description`,
`.vendor` and `.version` annotations that registry web UIs show. No source URL
is stated; each source archive's `NOTICE` names the fork's public repository
(<https://github.com/unemu-org/at-bios>, <https://github.com/unemu-org/vgabios>). Nothing
depends on the clock, the host or the fixture paths, so the same fixtures give
the same manifests and digests on
every run and host. Before any tag of a template is written, the build applies
the push guard of `unemu/` on the default registry (every blob open, each
copyleft ROM with its source archive in the closure) and stops on a refusal;
only the judged manifest is tagged. Version tags never move; a changed template
needs a new version, and a test pins the digests of the published one. `latest`
moves only away from an image that lower versions of the same repository name,
never from a higher version or from an image the build did not version. **The
build never pushes**: publishing the tags is the maintainer's step, with a dry run
of each push first.

**Get templates….** File › Get templates… and the Get templates… button of
the library's empty state, shown while the library holds no machine image
(firmware packages and media sets do not count; the empty state contacts no
registry), select Remotes › the default registry (`UNEMU_REGISTRY`, then
preferences) › `unemu/` › Machine images, listing it first if this session
has not (§3.2, Desktop library; the approved remotes mock-up, revision 2,
note 2). The Get templates dialog of the get-templates mock-up is retired;
its rules hold in Remotes: a template row offers the version tag `latest`
names when that is one of its 16 newest releases (its pre-releases when it
has no release), else the newest of them, among those this release runs
(§5.6); only PC artifacts of the PC provider's types run here, so a machine
image the node lists for another machine family, or another tool's artifact,
is muted with the reason ("A c64 machine image: this release runs IBM PC
compatibles") and cannot be pulled, as the dialog offered PC machine images
only; the Type column says Machine template only for a repository one name
under `unemu/` (`unemu/open-at`), while a deeper one (`unemu/staging/…`)
lists under the same node as a machine image; repositories the live registry
test leaves under `unemu/test-…` are never listed; Pull goes by the digest
the row showed and writes each local
tag after its closure is verified; a template already in the library says
so; several are chosen with Ctrl or Shift and pulled one after another under
one progress bar that counts blobs already in the store as done; Cancel
stops the running pull at its next read and the queued ones before they
contact the registry; a login a registry asks for opens Tools › Preferences ›
Registries on its Log in dialog, or on Add registry… for a registry with no
entry there, and the registry lists again once Preferences closes; listing
uses the login-check timeouts (10 s to connect, 15 s per read).

### 5.6. Compatibility of published images

Status: **accepted** in
[decision-30](<../../decisions/decision-30 - Published images declare the oldest unemupc that runs them.md>)
(design step of `upc-dfs.15`), accepted by the maintainer 2026-11-17;
**implemented in part: core and command line** (2026-11-17). The marker,
its parser and lenient reader, the shared and PC tables (every row is
0.0.1; the next release, 0.0.3, is `image_store::compat::NEXT_RELEASE`), the
writers, the early refusal of pull, Start, `image run` and `image machine
new`, the `unemu/` push guard, machines inheriting their firmware packages'
value and the command-line listings are in place
(`backlog/docs/commands.md` § Minimum release of an image). Remotes' choice
of the newest runnable version, its note of which newer version needs which
release, its dimming of a repository with nothing runnable and its refusal
to pull a tag this release cannot run are in place too (2026-11-17; Get
templates now shows Remotes, §5.5). The library's disabled rows of local
images and the summary cache's record of the requirement remain.
Published templates, installation sets and firmware packages stay on the
registry for good, and older releases read them too. This section records
what today's readers do with a newer image and defines a marker, so an
older release lists such an image disabled, with "Needs unemupc X or newer",
instead of failing to decode it or running it wrongly.

**Today's readers.** Every codec fails closed, and annotations are open.
The manifest envelope and its descriptors refuse unknown fields; every
config (machine, set, firmware package, sealed layer, zero base) refuses
unknown keys. Each vocabulary refuses values it does not know: board, CPU,
FPU, card kind, drive type, controller, serial mouse, media format, extra
kind, firmware role, fit board and class. So does an unlisted system BIOS
or option ROM. The manifest codec checks only the values of
`unemu.redistribution` and `org.opencontainers.image.base.digest` and
requires `unemu.platform`; it ignores every other annotation. Metadata
schema 1 is strict, and a newer schema is shown read-only (§2.1).

| Reader | New config media type | New layer media type | New config key | New vocabulary value |
| --- | --- | --- | --- | --- |
| Library summaries | Unreadable row (⚠) with the decoder's reason | Row looks normal | Machine image: row looks normal (its config is not read); set or package: unreadable row | Machine image or package: row looks normal; set with a new format: "Unknown floppy geometry" |
| Selecting a machine image, Start, `image run` | Refused | Refused | Refused with the decoder's message | Refused with the decoder's message |
| Remotes (File › Get templates… shows them) | Row listed, read leniently; Pull is refused before any blob | Row listed; Pull is refused before any blob | Row listed; Pull is refused after the download | Row listed; Pull is refused after the download |
| Pull…, `image pull` | Refused before any blob | Refused before any blob | Refused after every blob is fetched; nothing tagged | Same |
| New machine from image… | Refused | Refused | Machine made; it cannot start | Machine made; it cannot start |
| Insert… and the media picker | Refused | Refused | Refused | Refused |
| Firmware list of New machine… | Package skipped | Package skipped | Package skipped | Package offered; making the image is refused |
| `image ls` | Kind `unknown` | Listed | Listed | Listed |

Local records (`local/machines.toml`, a workspace's `session.toml`) are
schema 1 with no unknown keys allowed. A newer file fails closed and stays
untouched. A machine's settings overlay is checked again at Start. `image
remote ls` lists names only.

**Releases 0.0.1 and 0.0.2 (measured on 0.0.2).** A probe built from the
v0.0.2 tag read synthetic images derived from a valid open-firmware machine.
Release 0.0.1 reads images with the same code: between the two tags only
the tag-move rules changed. The probe found:

- Unknown annotations change nothing, whether on the manifest
  (`unemu.min-version`, `dev.unemu.min-app`) or on the config descriptor.
  Decode, summary, Start check, New machine from image… and `upcli image
  pull` all succeed, and the pull writes the tag. Any other key behaves the
  same way, because the codec checks only the two keys named above.
- `application/vnd.unemu.machine.config.v2+toml`: `upcli image pull` fails
  before any blob is fetched with "unsupported media type: …". In a shared
  store the image is an unreadable row and `image ls` says `unknown`.
- A new top-level config key, a new key in `machine.memory`, the reserved
  card kind `adlib` and an unknown card kind `future-device`: the pull
  fetches all 8 blobs and then fails. The messages are "unknown field
  `min_version`", "unknown field `future_option`", "card kind `adlib` is
  reserved, not implemented (schema 1)…" and "unknown variant
  `future-device`…". No tag is written; the fetched blobs stay untagged. In
  a shared store the row looks normal and New machine from image… succeeds,
  but Start is refused.
- A firmware package with an unknown role or an unknown catalogue id passes
  the store codec. Its row looks normal, but the pull's final check refuses
  it ("unknown PC firmware role", "unknown firmware catalogue id"). An
  unknown key in a firmware or set config is refused by the codec. A set
  format `tape` is refused by the PC provider.

So 0.0.1 and 0.0.2 ignore any marker, never run an image whose config holds
something new, and fail at pull or Start with a decoder message. Only a
change in what an existing key or value means could make them run an image
wrongly, and the proposal forbids that.

**Proposal.**

- **Marker.** The manifest annotation `unemu.min-app-version` holds a
  SemVer release `MAJOR.MINOR.PATCH`: the oldest release of the program
  providing `unemu.platform` that reads and runs the artifact. For
  `ibm-pc` (and its alias `ibm-pc-at`) that program is unemupc; `unemupc`
  and `upcli` share its version. Machine images, installation sets and
  firmware packages use the same rule. Without the annotation there is no
  requirement, so the six published sets and packages keep their digests. Like every annotation it
  is a hint, not an authority: codecs stay strict. Media-type majors move
  only for a decoder break. A release never changes what an existing key or
  value means; it adds new ones.
- **Value.** The shared store crate and the provider keep a per-feature
  table: the release that first reads each optional key, vocabulary value,
  payload media type, firmware catalogue entry and widened range. Writers
  stamp the maximum over what the artifact uses. When the maximum is the
  baseline 0.0.1, they leave the annotation out, so rebuilding something
  older releases already read gives byte-identical manifests. A recipe of
  `image template create`, or `--min-app-version` on the importing
  commands, may raise the value, for an emulation fix a title needs, but
  never lower it. Template creation and the push guard of `unemu/` compute
  the value again and refuse a missing or lower marker. New catalogue
  fields go into metadata schema 2, never into schema 1, so metadata never
  raises the value.
- **Platform id.** The platform ids of the alias table are shared rows:
  `ibm-pc-at` is the baseline and `ibm-pc` is first read by 0.0.3, because
  0.0.1 and 0.0.2 compare platform ids exactly. Every PC writer writes
  `ibm-pc`, so every PC artifact written since declares 0.0.3; releases 0.0.1
  and 0.0.2 ignore the marker and refuse such an artifact at pull, after the
  download (decision-32).
- **Reading.** The annotation is read leniently before the strict decode.
  A requirement is met when it is no higher than the release's level: its
  package version, or the newest version its own table names if that is
  higher, so development builds read what they write. A value that does not
  parse counts as unmet.
- **Display.** The library rows of machine images, sets and packages, the
  machines made from such an image, Remotes, Get templates, the media
  picker and the command line say "Needs unemupc X or newer". They disable
  Start, New machine from image…, Insert…, Use in machine…, `image run` and
  `image machine new`; manifest details and Delete tag stay. Get templates
  offers the newest version this release can run (`latest`'s version when
  it can), notes which newer version needs which release, and dims a
  repository with nothing runnable. **Pull refuses before fetching any
  blob.** A tag is written only for a validated closure (§3.1), and a
  release cannot validate a config it does not read.

**Test plan.** All inputs are synthetic and go through today's decoder
paths:

1. A machine image, a set and a package, each carrying
   `unemu.min-app-version = "99.0.0"` and an unknown annotation, decode in
   strict, display and provider modes. This guards the property 0.0.2
   relies on.
2. The requirement reader handles an absent value, met and unmet values, a
   development-build level, a malformed value, and a manifest with an
   unknown artifact type or top-level field. For the last, the lenient read
   still finds the marker.
3. Each table row has a test image that carries the row's version. Tests
   fail when a vocabulary value, optional key, payload type or catalogue
   entry has no row. The pinned digests of the published sets, packages and
   templates still match. A recipe may raise the value and may not lower
   it. The push guard refuses a missing or lower marker.
4. Library: marked images whose configs also hold an unknown key, card
   kind, format or role become rows that show the requirement. They read no
   config and record no failure, and their actions are disabled with the
   reason.
5. Pull from the mock registry: a marked image is refused with no blob
   request and no tag. An unmarked image with an unknown card kind keeps
   today's refusal, the fail-closed backstop.
6. Get templates: when `latest` is marked 99.0.0 and an older version is
   not, the older one is offered with the note. When every version is
   marked, the repository is dimmed.

Optionally, the unemu-media pipeline can also run the release a new
template's marker names (its pinned `upcli`) against the built template.
That tests the table against the real release.

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
| Drives / ROM | Each catalogue entry's exact geometry, one of the media the drive type takes; ROM placement, container offsets, chip lanes, services, compatibility and missing hash bindings remain validator/loader checks. |

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
| Help | Keyboard shortcuts and About unemupc. |

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
Group images by repository name, with one child row per manifest, for example
`dos-lab > 1.0, 2.0, gaming`. Tags of one repository that name the same
manifest, such as a version tag and its `latest` alias, share one row that
lists them, version tags first (`2.0, latest`); the Installation media and
Firmware collections list them the same way, and counts count rows. Actions
use the row's version tag; Delete tag… names the tag it removes and offers the
row's other tags. Machine files from disk are listed only under their own
Machine files entry, never among the stored images. The list is virtualized:
draw only visible rows.

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
  what was validated. Firmware is chosen on the Firmware tab (approved
  [firmware-settings mock-up](mockups/firmware-settings.html)): a System BIOS
  and, for a card with a BIOS of its own, a Video BIOS, among the catalogue
  entries that fit the board, grouped by family (one CMOS layout per family,
  `FirmwareEntry::cmos_layout`); an entry that cannot be chosen says why (the
  validator's reason, or "not in your store" with Import ROM folder…). A
  superseded entry is marked, follows the releases of its family, is never
  preselected and offers "Use <replacement>"; the catalogue stores its
  replacement and a short reason (`FirmwareEntry::superseded_by`). A choice
  writes the ROM table of a firmware package in the local store (asset names
  from the bytes' SHA-256); lowering binds the package's ROM blob by digest,
  marked as an image marks it, with the source archive that travels with an
  open copyleft ROM. Another CMOS layout replaces a stored CMOS image by the
  bytes derived from the revision's configuration (the clock, status
  registers and shutdown byte stay); the same layout keeps it. Publish
  revision asks for a new tag and never overwrites one, `latest` included:
  the next minor version for your own repository, the source version with a
  local suffix (`1.1.0-local.1`) for an official template, whose revision
  stays in the local store; taking user-supplied firmware makes the revision
  user-supplied, which the review says. Diskette drives can be added
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
  (title and others) and records `org.opencontainers.image.base.digest`;
  `unemu.firmware.sources` is written again from the packages the new
  closure's ROMs come from (a source package whose ROM left the closure goes,
  the package of a newly bound ROM joins; one no tag names any more stays as
  recorded). CMOS: a stored image keeps every
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
  firmware and other drive heads. Its Save as new image… and Discard changes use
  the complete-machine semantics in §4; saving stops the machine. Its Save as
  new image… (with *Flatten disk chains into new bases*, unchecked), Gaming
  mode's Keep as new image… and Machine › Save as new image… all
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
  annotations (dropping per-build ones such as `created`, and writing
  `unemu.firmware.sources` again when the machine's settings chose other
  firmware), may replace the title, and records the source as its base; a save without changes is the
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
