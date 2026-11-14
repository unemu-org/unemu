---
id: decision-27
title: Firmware packages are their own OCI artifact type
date: 2026-11-14
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-27 - Firmware packages are their own OCI artifact type

## Status

Accepted — 2026-11-14. The maintainer proposed the feature and accepted this
shape. Builds on [[decision-25]] (machine images) and [[decision-26]]
(installation media sets, including their extras). Details will go into
`backlog/docs/design/machine-images.md`.

## Context

Machines find user-supplied ROMs by scanning ROM folders, often on network
storage, and matching files against the firmware catalogue by SHA-256. A ROM
also comes with knowledge worth keeping next to it: which boards and machines
it fits, which chip position each part goes into, and documents such as a
technical reference or the BIOS source listings a vendor once published.

## Decision

1. **A separate artifact type.** A firmware package is its own OCI artifact
   (`application/vnd.unemu.firmware.v1`), next to machine images and
   installation media sets.
2. **Content.**
   - The config lists the ROM images with their role: system BIOS, option
     ROM, video BIOS, keyboard controller or other.
   - Each image records its chip position where it has one (for example
     "U27 even / U47 odd").
   - Every image has a SHA-256 that matches the firmware catalogue entry when
     one exists.
3. **Fit metadata.** The package records vendor, version and date, platform
   (for example `ibm-pc-at`), the boards and machine classes it fits, its
   catalogue ids and free notes.
4. **Extras.** Documents travel with the package through the same extras
   mechanism as installation media sets: manuals, technical references,
   published source listings and errata.
5. **Import once.** Importing a ROM folder matches files against the catalogue
   by hash and assembles packages, including multi-chip system BIOS sets. The
   folder is not scanned again afterwards.
6. **Machines.** A machine file resolves firmware from the store first and
   from ROM folders second. A machine image keeps embedding the ROM blobs,
   which deduplicate with the package by digest, and records the source
   package digest.
7. **Library.** The library gets a "Firmware" collection that shows what each
   package fits.
8. **Rights.** Proprietary ROMs (IBM and others) are `user-supplied`, live
   only in the user's private store and registry, and never enter the
   repository or public exports. Open firmware such as SeaBIOS, the Kiselev
   BIOS or XTIDE Universal BIOS may be marked `open`.

## Consequences

- The store's provider model covers a third artifact type. The neutral crate
  keeps the package manifest and extras generic; ROM roles, chip positions and
  fit rules are provider-declared.
- ROM folder discovery becomes an import source and a fallback, not the
  everyday path.
- The design note gains a firmware-package section. GUI work starts from a
  mock-up of the "Firmware" collection that the maintainer approves first.
