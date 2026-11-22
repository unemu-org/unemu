# Deferred

unemu maintains store, registry and image-envelope work; unemupc maintains hardware, journals and disk-layer codecs.

Things **deliberately not implemented**. If something seems missing and is listed here, don't add it unprompted — there's a reason. Each entry: what, why deferred, when to revisit.

## Format

```markdown
### <Feature / behavior>

- **Why deferred:** <one paragraph>
- **Revisit when:** <trigger condition>
- **Tracked in:** <task-N, if any>
```

A20 gating is implemented: the board's 8042/port 92h line drives CPU physical
address bit 20 and survives CPU-only reset. See
[the board contract](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/design/machine-config.md#a20-line-and-system-control-port-a).

## Entries

### Layered disk images (base image plus content-addressed layers)

Block-level copy-on-write layers would store only changed sectors and identify
their parent by content hash. A base image, such as a user-supplied Windows 98 SE
installation, plus layers of installed programs would form a linear chain.
Layers cannot be freely combined: two layers built separately on one base both
change the FAT and the Windows registry, so combining them means building one
on top of the other. A discardable per-session top layer would make every run
start from the same disk state; committing it would create a new layer.

Optional recipe layers would replay a deterministic input script (keys, mouse
and media changes at virtual times) on the parent to reproduce the layer byte
for byte. Results could be cached by `(parent hash, recipe hash, CPU core revision)`.
Proprietary bases and layers are never committed to the repository. They live
in the user's private image store and private registry as parts of whole-machine
images; see [`design/machine-images.md`](design/machine-images.md), §§4 and 5.
A saved machine state would be tied to the emulator version; disk layers remain
plain data.

Still deferred beyond that design: recipe layers, block-level deduplication
across similar bases, FAT-aware optimisation, payload compression, sector
layers for floppies and merging sibling layers.

- **Why deferred:** No workload needs layered images before Windows 98 SE runs
  in phase C. The floppy journal and the coming hard-disk write-back cover
  persistence needs in the meantime.
- **Revisit when:** The hard-disk write-back is designed, and again when phase C
  needs a reproducible Windows 98 SE disk state for golden tests.
- **Requirement for now:** The write-back format must not preclude layers: keep
  the base read-only and identified by SHA-256 (including large hard-disk images), and keep written sectors separate
  from the base. Commit creates a new base file and never rewrites the old one.
  Sector access stays behind one overlay interface so a hash-linked chain can
  replace a single base and journal later. The floppy design already preserves this separation; see
  [`design/persistent-media.md`](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/design/persistent-media.md), §Context and
  §Header: exactly 128 bytes, and
  [`design/machine-files.md`](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/design/machine-files.md), §Drives.
- **Tracked in:** upc-18x.5 (unemupc).

### Machine-image daemon

The image store is a library that the GUI and CLI call directly, for local
storage and for registry push/pull. Immutable blobs, lock files and tag
compare-and-swap handle concurrent processes; see
[`design/machine-images.md`](design/machine-images.md), §§3.1 and 5.1.

- **Why deferred:** No current workflow needs a long-running service.
- **Revisit when:** Machines must run headless on a server with remote viewing,
  remote control is wanted, or background GC is needed.
- **Requirement for now:** Keep the store API transport-neutral, so a daemon
  would be a thin wrapper with no storage or validation rules of its own.

### Reusing Docker and oras registry logins

The registry client reads secrets only from its own sources: environment
variables bound to one host, or the macOS Keychain item it creates with
`image login`. It does not read `~/.docker/config.json`, oras credentials or
Docker credential helpers, so a registry already logged in with those tools
needs its own `image login`; see
[`design/machine-images.md`](design/machine-images.md) §5.1.

- **Why deferred:** Credential helpers are external programs with their own
  storage and prompts, and their entries are not scoped the way this client
  scopes credentials (one login per registry, never offered to another host).
  Supporting them safely needs its own design and tests, which no current
  workflow requires.
- **Revisit when:** Users pull from registries they already use with Docker or
  oras, or Bearer-token authentication lands and helpers become the natural
  source of tokens.
