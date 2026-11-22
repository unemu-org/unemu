---
id: decision-25
title: Machine images are self-contained OCI artifacts in a local store
date: 2026-11-13
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-25 - Machine images are self-contained OCI artifacts in a local store

## Status

Accepted — 2026-11-13. Records the maintainer's storage direction and their
answers to the open questions of the machine-image design review. Builds on
[decision-24](decision-24%20-%20Desktop%20shell%20is%20a%20classic%20in-window%20menu%20UI%20built%20with%20egui.md) (desktop shell). Details: `backlog/docs/design/machine-images.md`.

## Context

The maintainer wants Docker-like machine images: tagged bases such as
`win98:1.0`, hash-linked disk layers and a writable gaming layer. They also
want images pushed to their own server. A disk image alone is useless. A
machine needs its configuration, its firmware, every drive with its order and
layers, its media catalogues and its CMOS.

## Decision

1. **The whole machine.** A machine image contains everything needed to run:
   - the machine configuration;
   - firmware, BIOS and option ROMs included;
   - every drive with its slot, order and layer chain;
   - media catalogues and CMOS;
   - room for a saved state.
2. **OCI artifacts.** Images are OCI artifacts: standard OCI image manifests
   with our own artifact and media types. The same format works in a local
   store and in any OCI distribution registry.
3. **A local store, no daemon.** The store is an OCI Image Layout directory in
   the platform data directory, always on a local filesystem. The GUI and CLI
   use it directly as a library. Concurrency comes from immutable
   content-addressed blobs, lock files and compare-and-swap tag updates. A
   daemon is deferred.
4. **Private registry.** Images are pushed to and pulled from the
   maintainer's private registry, which may run anywhere, for example on a
   Proxmox host with NFS behind the registry. Registries are an allowlist
   with a `private` flag. Images that contain proprietary blobs go only to
   private registries. TLS and authentication are on by default; plain HTTP
   or anonymous access needs an explicit per-registry opt-in. zot is the
   recommended registry. An `oras` verification spike must pass before the
   transport is built.
5. **Proprietary content.** Proprietary bytes may live in the user's private
   store and registry. The repository never contains them, and any public
   export strips non-open blobs.
6. **Three identities.** A blob digest, a machine digest (everything the guest
   can observe) and an image digest (the OCI manifest). Workspaces and saved
   states pin the machine digest.
7. **Metadata.** Metadata and artwork are ordinary blobs in the manifest.
   Edits stay a local library overlay until the user publishes them as a new
   version or tag. Personal ratings and notes are local only and never create
   a revision. OCI referrers may be added later.
8. **Tags.** Pushed version tags are immutable. Only designated `latest`-style
   aliases may move, and only with an explicit force.
9. **Verification.** Every Start fully verifies every base by SHA-256 for now.
   After the phase C measurements this changes to verification at ingest plus
   an explicit "Check store" command.
10. **Media and compression.**
    - A changed floppy is frozen into a whole new blob; floppies have no
      sector layers.
    - `+zstd` compression is required before any phase C multi-gigabyte
      image is pushed.
11. **Title launch.** Launching a title supports DOS first. Phase C Windows
    titles use typed input such as Start › Run. There is no guest-side helper
    device.

## Consequences

- Machine files stay first-class. Repository tests never need a store or a
  registry.
- The library has to scale to thousands of local images: an Explorer-like
  view with a summary cache. It gets its own mock-up for maintainer approval.
- Machine images carry rich, eXoDOS-style game metadata, several titles per
  image and launch commands. Library facets come from that metadata.

## Amendment 2026-11-14: free tag names and which tags may move

The maintainer chose free tag names: a version (`1.0.0`), `latest`, or a
descriptive tag that says what the image holds, such as
`open-at:8mhz-1mb-2mbems`. Point 8 becomes:

- A tag that has the shape of a semantic version (`x.y.z`, optionally with a
  pre-release or build suffix) is immutable, locally and when pushed: once it
  names a digest, it always names that digest.
- Any other tag, `latest` or a descriptive one, may be moved to a newer image,
  but only after an explicit confirmation that shows the old and the new
  digest. The update stays a compare-and-swap. The old image remains reachable
  by its digest and by its other tags.
- Creating an image (New machine…) or saving one (Save as new image…) asks for
  the repository name and the tag; the version tag defaults to `1.0.0`, and a
  later save suggests the next version.

## Amendment 2026-11-14: registry.unemu.dev is the default registry

The maintainer chose `registry.unemu.dev` as the default registry of the unemu
family, in the role `docker.io` plays for Docker. Reference resolution follows
Docker's rules:

- `name:tag` (no slash) means `registry.unemu.dev/unemu/name:tag`. `unemu` is
  the official namespace, like Docker's `library`.
- `owner/name:tag` means `registry.unemu.dev/owner/name:tag`.
- A first component that contains a dot or a colon, or is `localhost`, is a
  registry host: `nas.example:5000/private/games:1` goes to that host.
- A reference without a tag means `:latest`; a digest reference (`@sha256:…`)
  pins exact content and ignores tags.

The default can be overridden: per user in the preferences, per command with
`--registry`, and by an environment variable for CI. Local store tags keep
their short form; the full reference is used only when pulling, pushing or
recording provenance. Until registry.unemu.dev is reachable from the
internet it serves the maintainer's LAN; public templates and fixtures live
under its `unemu/` namespace.

## Amendment 2026-11-14: private content never reaches a public registry

- Every registry the user configures has its own login. Secrets live in the
  operating system's credential store, never in preferences or project files.
- `private/` repositories always require login. They hold the maintainer's
  proprietary images (vendor ROMs, commercial software) and are never
  downloadable anonymously. A public (cloud) registry never hosts them.
- The client enforces this before any upload: an image whose closure contains
  a `user-supplied` blob can be pushed only to a registry marked private, and
  never into the public `unemu/` namespace; pushes to `unemu/` require every
  blob to be `open`. The GUI offers no override.

## Amendment 2026-11-14: tag grammar, no build metadata

The OCI distribution specification limits a tag to
`[A-Za-z0-9_][A-Za-z0-9._-]{0,127}`, so a `+` cannot be pushed. The
maintainer accepted the consequence:

- Every tag, local or pushed, follows that grammar. The client refuses any
  other tag when it creates, saves or moves one, so a local tag can always be
  pushed unchanged.
- A version tag is `x.y.z` or `x.y.z-<pre-release>` (`1.0.0`, `1.1.0-rc1`).
  Semantic-version build metadata (`1.0.0+build.5`) is not allowed; the
  digest already identifies the build. The first amendment's "optionally with
  a pre-release or build suffix" now reads "optionally with a pre-release
  suffix".
- Secrets live in one credential-store backend per operating system, detected
  at run time: the macOS Keychain, the Secret Service on Linux (GNOME Keyring,
  KWallet, KeePassXC) and the Windows Credential Manager. Without one the
  password lasts for the session only; scripts and CI use environment
  variables. A plain-text file is never used.

## Amendment 2026-11-15: privacy per namespace

The maintainer's registry serves two namespaces on one host: `unemu/`, the
public namespace with open content only, which will later move to a host on
the internet, and `private/`, which stays on the maintainer's own server and
always requires a login. Privacy is therefore set per namespace, as the
approved registry-accounts mock-up shows, not only per registry:

1. A registry entry lists its namespaces, each public or private; an entry
   without namespaces covers the whole registry, as before.
2. The client guard lets user-supplied blobs go only to a namespace marked
   private (or a registry marked private as a whole), and still refuses one
   that answers anonymous reads. `unemu/` is never private: it takes open
   content only, whatever the entry says.
3. When the public namespace moves, `private/` keeps its host; the entry for
   the new public host lists `unemu/` only.

## Amendment 2026-11-16: pulled images keep their registry host

With several registries attached, each with its own `private/` repositories,
a short local tag such as `private/media/x:1.1` cannot say where it came
from, and two registries collide. On 2026-11-16 the maintainer approved
revision 1 of the Remotes mock-up and decided that local tags of pulled
images carry the registry host, Docker-style; revision 2 of the mock-up
applies that decision:

1. A pull from `registry.unemu.dev` keeps the short local tags
   (`open-at:1.2.0`, `unemu/media/freedos:1.4`). Only the family default
   counts, not a default the user configures.
2. A pull from any other registry writes the full reference as the local tag,
   port included: `nas.example:5000/private/media/x:1.1`.
3. Images made here are given no host. A person may still type a
   host-qualified name, as with Docker; the pull record, not the name, says
   where a tag came from.
4. The store records the registry each pull wrote a tag from, so a short tag
   pulled from the default registry can be told apart from one made here.
   Existing tags are never renamed; without that record their source is
   unknown.
5. Push of a host-qualified tag goes to its own host unless a target is
   given; namespace policy reads the path after the host.

This replaces "Local store tags keep their short form" in the amendment on
the default registry above.
