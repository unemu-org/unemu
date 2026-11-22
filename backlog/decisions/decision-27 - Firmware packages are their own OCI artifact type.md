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
shape. Builds on [decision-25](decision-25%20-%20Machine%20images%20are%20self-contained%20OCI%20artifacts%20in%20a%20local%20store.md) (machine images) and [decision-26](decision-26%20-%20Installation%20media%20sets%20are%20their%20own%20OCI%20artifact%20type.md)
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

## Amendment 2026-11-15: licences, source archives and descriptive metadata

The maintainer wants open firmware publishable under `unemu/` with its
licence obligations met, and every package to describe itself well enough to
be understood decades from now without this program.

1. **Licence on every layer.** Each layer (ROM image, document, extra, source
   archive) carries an SPDX licence expression, for example
   `GPL-3.0-or-later` or `LGPL-2.1-or-later`, next to its redistribution mark
   (`open` or `user-supplied`). A layer with no known licence is
   `user-supplied`.
2. **Marking at import.**
   - A ROM that the firmware catalogue knows as open gets its licence and
     `open` automatically.
   - Any other ROM defaults to `user-supplied`. The user may mark it `open`
     only by an explicit statement that names its licence, such as
     `--open --license <SPDX>` in the CLI, or the same choice in the GUI
     import review. The program never guesses `open`.
3. **Source archives.** A package may carry the corresponding source as its
   own layer (a zip or tar archive with the role `source`). For a copyleft
   licence (the GPL, LGPL, AGPL and MPL families) a ROM can be marked `open`
   only together with its source archive. The archives for the project's open
   fixtures are built reproducibly from the pinned upstream revision plus the
   project's patches.
4. **The config follows its layers.** A package's config (the program's own
   metadata) is `open` when every layer is `open`, and `user-supplied`
   otherwise.
5. **Publication.** The push guard refuses to publish a copyleft `open` ROM
   to a non-private registry without its source archive in the same closure.
   A machine image that embeds such a ROM also carries the source layer; the
   blob is shared with the firmware package, so it costs nothing extra.
6. **Descriptive metadata at import.** Every package records, prefilled from
   the catalogue where an entry exists and editable during import:
   - what it fits: boards, machine classes, and the devices a ROM requires
     (for example the card kind a video BIOS or an option ROM drives);
   - free labels (for example `ami`, `turbo`, `xt-ide`);
   - a title and a description;
   - provenance: where the bytes came from (dumped from a named board and
     its chip markings, downloaded from a named source, or built from a
     named revision) and when;
   - a plain-text README layer (UTF-8 Markdown or text) that holds the
     description, provenance, chip markings, known quirks and licence
     notes in human-readable form.
7. **Long-term readability.** Packages use only open, plain formats: OCI
   layouts, JSON, UTF-8 text, zip or tar, SPDX identifiers and the standard
   `org.opencontainers.image.*` annotations where they fit. Nothing needed to
   understand or extract a package depends on this program.
8. **Changing metadata.** A package is immutable. Editing its labels,
   description, fit or licence publishes a new package revision with the same
   ROM blobs.
