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
