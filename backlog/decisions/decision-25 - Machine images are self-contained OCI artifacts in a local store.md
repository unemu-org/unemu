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
[[decision-24]] (desktop shell). Details: `backlog/docs/design/machine-images.md`.

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
