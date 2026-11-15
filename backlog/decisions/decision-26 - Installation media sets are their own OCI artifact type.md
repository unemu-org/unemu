---
id: decision-26
title: Installation media sets are their own OCI artifact type
date: 2026-11-14
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-26 - Installation media sets are their own OCI artifact type

## Status

Accepted — 2026-11-14. The maintainer proposed the feature and accepted this
shape. Builds on [[decision-25]] (machine images in a local OCI store).
Details will go into `backlog/docs/design/machine-images.md`; the work is
tracked as `upc-5pw.18`.

## Context

A machine image holds a whole machine. Games and programs, though, usually
arrive as installation media: a set of floppies, later CD images. A user wants
to keep such a set in the same store and registry, install it into any
machine, and get back the same floppies or discs when importing it elsewhere.

## Decision

1. **A separate artifact type.** An installation media set is its own OCI
   artifact (`application/vnd.unemu.media-set.v1`), next to the machine image.
   It is not a layer inside a machine image.
2. **Content.**
   - The config lists the media in order. Each medium has a label (for example
     "Disk 1 of 4 - Setup") and a format: floppy geometry derived from the size
     now, optical later.
   - The config also records the platform.
   - The layers are the medium blobs: raw floppy images now, ISO images later.
   - The set carries a metadata blob in the same schema as machine images
     (title, publisher, year, cover).
3. **Always read-only.** A set is a read-only base. When an installer writes to
   one of its disks, the write goes to a per-workspace journal, never into the
   set.
4. **Shared blobs.** Media blobs are content-addressed, so a set and a machine
   catalogue that hold the same disk store it once. A machine image may embed
   a set's media in a drive catalogue and record the source set's digest.
5. **Use.** The library gets an "Installation media" collection. Media > Insert
   gets "From library…", which loads a whole set into the drive's catalogue in
   order. The existing next/previous-disk actions then work on the set.
6. **Import.** A set is imported from a folder or a list of image files, with
   labels and order editable before publishing.
7. **Optical media.** Optical sets can be stored and listed. Inserting them
   stays disabled until CD-ROM emulation exists.
8. **Rights.** Installation media are usually proprietary. They are marked
   `user-supplied`, live only in the user's private store and registry, and
   never enter the repository or public exports.

## Consequences

- The store's provider model covers a second artifact type. The neutral crate
  keeps the media-set manifest generic; platform media formats are
  provider-declared, as for machine images.
- The design note gains a media-set section. GUI work starts from a mock-up of
  the library collection and the "From library…" dialog, which the maintainer
  approves first.
- No CD-ROM work follows from this decision; optical support only reserves
  the format.

## Amendment 2026-11-14: extras (loose files)

The maintainer asked for loose files in a set too, for example a DOS release
with its manuals, or a CD game with patches in zip or exe form. Accepted shape:

1. **Extras in the config.** A set lists extras next to its media. Each extra
   is a blob with its original file name, size, digest, a kind (document,
   archive, executable or other) and an optional DOS 8.3 name.
2. **Host side.** Documents open on the host from the library, without
   starting a machine.
3. **Guest side.** Chosen extras are packed on demand into a generated,
   deterministic FAT transfer medium: a 1.44M floppy when they fit, otherwise
   a FAT16 disk image. It is inserted like any other medium. An ISO variant
   follows once CD-ROM emulation exists. There is no guest-side helper and no
   host folder sharing.
4. **Order of work.** The media-set codec and CLI import carry extras from the
   start; the transfer-medium generator and the UI come after, the UI from an
   updated mock-up that the maintainer approves first.

## Amendment 2026-11-15: redistributable shareware sets

The maintainer approved publishing public shareware sets, starting with the
original Apogee shareware releases of Commander Keen 4, Wolfenstein 3D
episode 1 and Duke Nukem II episode 1, to the public `unemu/` namespace.
Point 8 stays the default; this amendment adds one exception to it:

1. **Explicit statement only.** A set is open only when the import says so,
   with a licence identifier for the vendor's terms (an SPDX `LicenseRef-`).
   Nothing is guessed from file names or contents; without the statement a
   set stays `user-supplied`.
2. **The terms travel with the bytes.** An open set carries the vendor's own
   terms text as an extra, and every layer records the digest of that extra.
   The client guard refuses to publish an open set whose terms text is not
   in the same closure and open.
3. **Unmodified releases.** The media hold the files exactly as the vendor
   released them, pinned by SHA-256 in the fetch scripts under
   `vendor/shareware/`.
4. **Still never in the repository.** Game files and the vendor's terms
   texts are never committed; the repository holds only the fetch scripts,
   the pins and a paraphrase of the terms.
5. **Rights follow the bytes**, as in [[decision-27]]: a machine image that
   embeds an open set is open only where all its other layers are.

## Amendment 2026-11-15: open media under SPDX licences, with sources

The maintainer asked for the official FreeDOS 1.4 Floppy Edition as open
installation media in `unemu/media/`, published with the official templates.
This extends the shareware amendment above:

1. **Two ways to open a set.** A set is open either under a `LicenseRef-` for
   a vendor's own terms with that terms text attached (shareware, above), or
   under an SPDX expression that states the real licences of its contents.
   Nothing is guessed; without either statement a set stays `user-supplied`.
2. **Copyleft needs its source.** A set whose expression contains a copyleft
   licence carries the corresponding source archive as an extra, and every
   copyleft layer links it (`unemu.source`), as firmware packages do
   ([[decision-27]]). The client guard refuses such a set for `unemu/` or any
   public namespace without that link; a private namespace still takes it.
3. **Every part is covered.** The open claim covers every program on the
   disks, each with its licence recorded in a licence index shipped with the
   set; a part without a verifiable licence or required source keeps the set
   `user-supplied`. Documents that state no licence are not attached to an
   open set.
4. **Operating systems are allowed.** Open operating-system installation media
   such as FreeDOS may be published in `unemu/media/` next to the template
   machines; the template machines themselves still ship without an operating
   system.
