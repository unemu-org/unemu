# Machine images, capabilities and desktop UX

Status: **desktop UX and toolkit accepted**, 2026-11-13; the machine-image format,
store and capabilities API remain design proposals. This spike adds no Rust
implementation, new command or runnable bundle.

## 1. Scope and existing contracts

The approved [window mock-up](mockups/main-window.html) and
[decision-24](<../../decisions/decision-24 - Desktop shell is a classic in-window menu UI built with egui.md>)
define one classic application window, in-window menus on every platform and an
egui/eframe shell. The welcome/library client area becomes the emulated screen
on Start; settings, media and layer actions use modal dialogs. The HTML contains
sample data and illustrative interactions, not emulator wiring. Section 7
describes this accepted direction; implementation starts with the shell and
existing machine files, before the image store and layer dialog.

A machine image describes the **whole machine**, not just a hard disk: resolved
hardware configuration, firmware identities, every controller/slot and ordered
media catalogue, disk chains, initial inserted media, CMOS and optionally a
compatible saved state. A tag such as `dos-lab:1.0` names this complete graph.
An image can be incomplete locally when user-supplied blobs are missing.

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

## 2. Immutable manifest and identity

Use **TOML** for manifest version 1: it matches machine files, supports readable
hexadecimal hardware values and keeps reviewable diffs. JSON is appropriate for
capabilities responses but offers no advantage for hand-edited image metadata.
Hash exact UTF-8 object bytes with SHA-256; do not hash an unspecified TOML-to-JSON
conversion. A versioned writer produces stable key order, LF and no timestamps;
equivalent hand-edited TOML may have a different object identity. This is safe.
Unknown fields/versions fail closed. Bound metadata to 1 MiB, chain depth to 64
and total graph traversal to 10,000 objects before fetching blob bytes.

This is a schema sketch, not a copyable image. `<...>` stands for a full
`sha256:` plus 64 lowercase hexadecimal characters. No host paths or URLs are
stored. Every digest target also has a declared type and exact byte length in
an object inventory, checked before decoding; duplicate/unused inventory entries
and conflicting declarations are rejected. Firmware/media arrays retain order.

```toml
schema = 1
kind = "machine-image"
name = "dos-lab"
config = "<resolved-machine-toml>"
# Provenance only: config is complete without consulting the parent manifest.
parent_image = "<previous-machine-image>"

[[objects]]
id = "<resolved-machine-toml>"
kind = "machine-config"
bytes = 2048
redistribution = "open"
license = "AGPL-3.0-or-later"
# Other references below each require their own inventory record.

[[firmware]]
key = "system-bios"
source = "assets/system.bin"
blob = "<user-supplied-bios>"
# Split chips use separate source/blob bindings; config retains lane and offset.

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

The inventory includes the transitive closure of config, firmware, raw bases,
layer descriptors and sector packs, CMOS and saved state. Each entry declares
`redistribution = "open" | "user-supplied"`; only open entries carry a reviewed
SPDX licence expression and provenance. Names never imply permission.
`parent_image` is optional provenance, not hardware inheritance or a requirement
to download obsolete machine images. Traverse it for history only, not for run.

### Resolved hardware and media bindings

`config` is a flattened schema-1 machine TOML, with no `extends` or host endpoints.
Use the current resolver first, preserving board/card order, defaults, RTC start,
CPU/FPU, memory, ROM addresses/services, split-chip assembly and drive geometry.
Replace image paths with unique relative logical names (`assets/...`); these
names are looked up only in manifest bindings, never opened on the host.
Reject absolute paths, `..`, duplicate names, unresolved sources and unbound blobs.
This requires a future bundle loader; the current file CLI does not resolve it.

Config owns hardware and firmware mapping; manifest owns media content and initial
selection. Config drive entries keep controller, unit/type/geometry but omit
`image`, `sha256`, `media`, `empty`, `write`, `journal` and `keep_generations`.
Config NVRAM retains derived/blank without a manifest NVRAM table, or an image
with its logical name; image/journal policy belongs to the manifest table.
This **bundle-config profile** is
not a standalone machine file: the loader fills these holes from the manifest
before the existing lowering/validation path. No new hardware schema is invented.
ROM `image` names bind to `firmware.source`; hashes pin whole containers/chips,
with assembled BIOS pins retained when present. Catalogue compatibility and
firmware services still come from the existing catalogue and assembly checks.

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

## 3. Content-addressed store, tags and local workspaces

```text
<store>/
  objects/sha256/ab/cdef...       immutable bytes, full digest split 2 + 62
  tags/dos-lab/1.0.toml          digest of a machine-image manifest
  workspaces/play-001/           mutable host state, never image content
    session.toml                pinned manifest, per-medium heads and policies
    media/system.upfj           current single-writer journal
    media/cmos.upfj              independent NVRAM journal
    transaction.toml            recoverable publication intent
  local/blob-index.toml          digest -> local file hints, permissions, licence
  local/library.toml             last run, favourites, labels, UI preferences
  staging/                      unpublished files; not GC roots
```

Hash and length-check imported bytes, write a sibling temporary file, synchronize,
verify the written file, then publish without clobbering and synchronize the
directory. An existing object must verify before reuse. Base verification covers
**every byte, including large hard disks**; preserve mounted descriptor identity
and mutation checks from persistent media. A filename/digest cache cannot replace
verification. Never link a mutable user file into the immutable object store.
Local copy or verified external descriptor access is explicit; external sources
must remain unchanged while mounted and are never written by the emulator.

Tags are mutable local references; digests are immutable identities. Resolve a
tag once at launch and pin that digest for the session. Tag replacement uses
an expected-old-digest check and atomic durable publication. Branching creates a
new tag/workspace without editing the parent. Start with local tags and explicit
bundle archives; remote registry discovery/authentication is separate work.
Names, tags and workspace/media ids use `[a-z0-9][a-z0-9._-]{0,63}`; reject path
separators and duplicates. Digests, not user strings, determine object paths.

Deduplication is exact object reuse: identical raw bases and identical sector
packs occupy one object each, also across tags/bases. Sector packs can be reused
across different parents, while layer descriptors remain parent-bound. No claim
of deduplication for similar but nonidentical whole bases; chunking is deferred.
Library size is the distinct reachable byte total, excluding working journals;
savings compare it with the sum of each image's reachable size counted separately.
Show exclusive bytes and estimated reclaim separately from virtual disk capacity.
Deleting a tag removes a reference, not shared objects. GC marks tags, workspaces,
archives being created, saved states and active-run pins under a store lock;
recheck roots before sweeping. Never collect another image's shared base.

## 4. Disk chains and writable sessions

A raw base is immutable bytes with full SHA-256 and geometry. A sealed layer is
an immutable TOML descriptor: `schema`, `kind = "disk-layer"`, `parent` (raw base
or sealed layer digest), `base` (root raw digest), `geometry`, `sectors` (sector
pack digest), `sector_count` (number of replacements), and `result_sha256`
(digest of the complete reconstructed disk). All sizes must agree with the root.
A sector pack is ascending unique records of `u64le LBA + 512 data bytes`, no
header/padding; its exact length is `sector_count * 520`, bounded by capacity.
LBA bounds, digest and length are checked before use. Empty packs are allowed.
A future codec change needs a new schema, not a reinterpretation of these bytes.
Sealing emits only final sectors differing from the exact parent; command history
and recovery generation markers stay in the local journal, outside the layer.

Read order is writable sparse overlay -> newest sealed layer -> earlier layers
-> raw base, through **one `SectorMedia` adapter**. Controllers never see a chain
or host path. Resolve and verify parents before executing; reject cycles, missing
objects and wrong parents/geometry. `result_sha256` is verified by streaming the
complete reconstructed disk before mount or publication, independently of object
hash checks. Hashing is host preparation; it cannot grant guest instructions.

A layer binds to exactly one parent. Sibling layers cannot be merged by union:
filesystem allocation tables and other shared metadata may conflict. Installation
layers must be built serially on the chosen parent. Hardware/config changes make
a new complete machine manifest, even if all disk heads remain unchanged.

A workspace pins every media head and holds mutable journals separately. Current
version-1 journals bind a raw base's length/hash, not a chain: **do not relabel a
v1 header to mean a layer hash**. Initially materialize a verified new raw base
from the chain and use the current v1 journal unchanged. This costs space and
full-image I/O. A later versioned journal binding may name a chain head plus its
full logical digest/capacity; its scanner retains command/recovery guarantees.
Both paths use the same overlay interface and always leave the old base intact.

| Action | Proposed behavior |
|---|---|
| Start | Resume a selected local workspace, or create one from the pinned image. Readonly media reject writes; journal media get explicit private paths. |
| Gaming mode | Create an isolated disposable workspace for every writable disk/catalogue medium and CMOS; never reuse the regular journals. On exit ask keep as a new image or discard. No automatic discard after a crash. |
| Save as layer… (commit layer) | Stop the machine, finish all active media, seal each changed overlay into a parent-bound layer and publish a complete new manifest/tag. Parents remain unchanged. |
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
is a separate idempotent transaction step; both roots remain pinned until recovery
finishes. Failure leaves an old or complete new image plus recoverable journals; startup
offers retry/inspect, never a silently mixed machine. Partial published objects
are harmless unreferenced data. This is a proposed transaction requiring crash
injection tests, not a measured power-loss guarantee or a guest-consistent save.

CMOS uses the existing 64-byte base/journal rules, including excluded transient
and clock fields. The 512-byte disk pack is **not** used for CMOS: freeze to a new
64-byte raw object. Derived/blank NVRAM remains config state. Saved states, when
supported, pin exact config/firmware/media identities, inserted selections,
emulator/device snapshot versions and x86jit revision. Hash this compatibility
tuple without the enclosing manifest/state pointer to avoid self-reference;
incompatibility offers a cold boot without
the state, never an approximate restore. Saving media alone is not a snapshot.

## 5. User-supplied blobs, pull and export

Proprietary firmware, OS media, derived disks and layers are user-supplied and
identified by SHA-256 only in shared manifests. Never publish their bytes, names
of private installations, source paths, screen captures or credentials. A layer
containing copied proprietary sectors is restricted even if its parent is omitted.
New content is user-supplied by default; export permission requires explicit,
reviewed evidence. Restricted inputs propagate that classification to derived
content unless a separate review establishes redistributable content.

Proposed `pull`: import the manifest/open objects, enumerate unresolved hashes,
then let the user select folders to search. Hash complete candidate files and
match digest plus length; filenames are hints only. Search is local and opt-in,
with progress/cancel for large images. Never upload candidate bytes, folder lists
or filenames. Split firmware is matched by individual chip/container identity;
do not guess a match by assembled size. Store folder hints only in the local
index. A missing required blob blocks Run and shows its role, digest and locate
action; a missing saved-state blob permits an explicit cold boot.

Export traverses the complete object graph, validates inventories and includes
**only reviewed open blobs** with licence/provenance metadata. All restricted
entries stay as hash/length/type references; derived layers and optional state
are checked too. No mutable journals, local indexes or host paths leave the
workspace. Export previews omitted objects and resulting run readiness. Metadata
itself must be publish-safe. Hashes identify content, not ownership, licences or
trusted publishers. Signed registries and remote transport are outside this spike.

## 6. Capabilities API sketch: validator-owned choices

READ: `crates/assembly/src/validate.rs`, `validate_resolved`, resource checks,
`check_board_memory`, IRQ routes and DMA inputs. It returns the first error in
fixed order, not a form schema. Some current representation domains live in
machine-file lowering/device configs. A UI must not transcribe those tables.

Propose shared assembly-owned descriptors for boards, card kinds, fields,
defaults, scalar/enum domains, units, derived resources and constraints. Lowering
consumes these same domains; the validator consumes the same predicates. Keep
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
option as forbidden just because unrelated required firmware is missing. For
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

## 7. Approved desktop behavior and technology

The shell follows the approved [window mock-up](mockups/main-window.html) and
[decision-24](<../../decisions/decision-24 - Desktop shell is a classic in-window menu UI built with egui.md>).
One classic Win32-style application window has a title bar, in-window menu bar,
toolbar, client area and status bar with power and drive activity lights. The
menu bar stays inside the window on **every platform, including macOS**; the
macOS global menu keeps only the minimum About and Quit items. Application
commands remain in the same place on macOS, Windows and Linux.

| Menu | Commands and role |
|---|---|
| File | New machine…, Open machine file…, Import image…, Machine library and Exit. |
| Machine | Start, Pause/Resume, Send Ctrl+Alt+Del, Power cycle, Stop, Settings… and Disk layers…; Save state remains disabled until supported. Gaming mode is a Machine-menu action when disposable workspaces are available. |
| Media | Per-slot inserted names, Insert…, Next image in catalogue and Eject; absent drives and unsupported CD-ROM are disabled with reasons. |
| View | Toolbar/status-bar visibility, scale, full screen and host audio mute. |
| Tools | Media catalogue…, Locate firmware…, Create blank disk… and Preferences…. |
| Help | Keyboard shortcuts and About unemupc98. |

The toolbar repeats common Start, Pause, Stop, Send Ctrl+Alt+Del, floppy,
Settings and Layers commands outside guest pixels. Enablement follows machine
state and backend capabilities. The status bar reports readiness/running state,
power, drive activity, CPU/rate, EMS and audio state; pointer capture/release is
visible in status text. Indicators observe completed boundaries, never drive
guest execution. View can hide the toolbar and status bar.

When stopped, the client area shows a welcome pane, a machine list and details
for the selected row. The list shows names, image tags and last run; details
contain hardware, firmware readiness and the base/layer chain, with Start,
Settings… and Layers… actions. Capacity, stored/exclusive bytes and dedup savings
belong in these details and the library's store summary. Missing user-supplied
firmware blocks Start and points to Tools › Locate firmware…. Last-run wall time
is local library metadata only. Start, Enter or double-click replaces this same
client area with the emulated screen and updates the title to the machine name.
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
- **Insert floppy** shows the selected slot's catalogue, Browse…, read-only or
  journal write mode, detected geometry and compatibility/retention warnings.
  Insert commits the selection; Cancel retains the current medium.
- **Disk layers** shows the parent chain and writable changes alongside settings,
  firmware and other drive heads. Save as layer…, Discard changes and New image
  from here… use the complete-machine semantics in §4; saving stops the machine.
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

Import an existing TOML/YAML machine through its current extends resolver and
origin-aware loader; verify all bytes/hashes, flatten once, assign logical blob
names and preserve effective device order, policies, slot order and catalogues.
Review newly hash-pinned firmware/media and missing inputs before publishing a
manifest. Existing files and CLI paths remain usable; import is opt-in and never
rewrites source files, bases or journals. Re-export materializes each head to a
new raw file and restores normal machine-file fields and fresh journal paths.

For a v1 journal, stop/release its writer, verify the exact original base and
scan read-only using existing recovery rules. Interior corruption blocks import;
interrupted tails require an explicit recovery choice on a private copy. Freeze
a chosen closed/recovered generation into a new raw base first; keep the old
journal/retention history as a local backup. A later verified conversion can
create a sparse layer against the original base by comparing final sectors.
Per-medium generation ids cannot become a synchronized machine snapshot. First
import records the chosen generation separately for every disk/catalogue/CMOS.

No implementation, remote registry, signatures, chunk-level dedup, compression,
encryption, qcow2 compatibility, sibling-layer merge, deterministic install
recipes, live multi-disk transactions, new CPU/board, CD/DVD emulation or generic
saved-state format is included. A reserved future field/button must stay disabled
until its backend capability exists. Implementation proposals belong in the
supervisor's tracker, not a second task list here. The shell appearance and
egui/eframe choice are accepted in decision-24; the layer/journal codec rollout
remains a review point. New GUI work requires an approved mock-up first.
