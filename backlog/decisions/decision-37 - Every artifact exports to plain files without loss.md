---
id: decision-37
title: Every artifact exports to plain files without loss
date: 2026-11-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-37 - Every artifact exports to plain files without loss

## Status

Accepted — 2026-11-17 by the maintainer, as written. The file-name annotation on layers changes manifest bytes, so it lands before the six published artifacts are pushed again (recipes.md §19, A3). Saving and loading whole artifacts as OCI Image Layouts is separate work (`upc-5pw.116`): this record is about unpacking one artifact into plain files. Design step of `upc-5pw.113`; recipes.md §3.10.

## Context

The store keeps managed copies, and duplication is accepted. A user who
trusts the store deletes the original files and keeps only the store. That
is safe only if every artifact unpacks back to its files: unmodified media
byte for byte, built disks as images and, on request, as files with names
and times. The way back must be documented well enough for a script that
has no `upcli`.

## Decision

1. **`upcli image export REF --to DIR`** writes each medium and ROM chip as
   its decoded bytes under its recorded file name, each extra under its
   file name, the config, metadata and manifest as stored, and a
   `SHA256SUMS` over all of them. The decoded bytes of an unmodified medium
   are the input's bytes.
2. **`--files`** also writes the content of every built file system as a
   folder tree, with the names and times the recipe pinned and the volumes
   of a split set merged. It needs a FAT reader in the program.
3. **Every layer descriptor carries its export file name** in
   `org.opencontainers.image.title`, the ORAS convention, so a generic OCI
   client names the files it pulls. A `+zstd` layer is one Zstandard frame;
   `unemu.decoded.digest` is the SHA-256 of what `zstd -d` gives.

   *Amendment, 2026-11-17 (the maintainer, implementing A3):* a `+zstd`
   layer is titled with `.zst` after its file name (`x86BOOT.img.zst`,
   `LICENSES.TXT.zst`), so an ORAS pull followed by plain `zstd -d` gives
   back exactly the original files under their names; an uncompressed
   layer keeps the plain name. The name rules and the uniqueness of names
   within an artifact apply to the decoded name, and the title with `.zst`
   must also be a valid file name. No layer of an input takes a name the
   export writes itself: `config.toml`, `metadata.json` (a set's metadata
   layer only), `manifest.json`, `SHA256SUMS` and `EXPORT-VERSION`, and in a
   firmware package `README.md`, its README layer's name. The rules are in
   machine-images §2, Export file names.
4. **The layout is documented** in `backlog/docs/` with a script of a few
   lines that uses only an OCI client, `zstd` and `sha256sum`, and it has a
   version file, `EXPORT-VERSION`. It is one of the things that bind forever
   (recipes.md §9).

   *Note, 2026-11-19 (B11, `upc-5pw.113.11`):* the script,
   `scripts/export-layout.sh`, is more than a few lines and needs `jq` to
   read the manifest, and mtools for `--files`; it reads an OCI Image
   Layout instead of an OCI client's pull.
5. **Three limits are stated, not hidden.** A container a recipe unpacked is
   not rebuilt: the user gets the members taken, not the archive. Members
   the recipe did not take are not in the artifact. Host file times are
   never stored. `recipe build` prints, for every input, `kept as stored`,
   `kept as files` or `not kept: N members unused`.

   *Note, 2026-11-19 (B11, `upc-5pw.113.11`):* the report names the
   exported files and reads `kept as stored in F`, `kept as files in F, G`,
   `kept as text with DOS line ends in F`; a way through an `unpack` step
   reads `members kept as …`, followed, unless the archive is also kept as
   stored, by `not kept: the archive itself, N members unused`
   (commands.md, Building recipes).
6. **A recipe that wants an archive itself kept** lists it as an extra.

## Consequences

- The config gains nothing; the file names travel as descriptor
  annotations. MEASURED on 2026-11-17: releases 0.0.1 and 0.0.2 read
  installation sets and firmware packages with a title on every layer.
  Scratch builds of each release's sources (`git archive` of the tag) took
  the three FreeDOS sets and three firmware packages the release builds
  itself, plus a synthetic set, added the title to every layer, and read
  them through the release's own code: manifest decode, store resolution
  and publication check, library summaries, a machine created from the
  titled packages, a firmware edit, a set re-import and a registry pull.
- The export writes `EXPORT-VERSION` beside `config.toml`, `metadata.json`,
  `manifest.json` and `SHA256SUMS`; no layer of an input may take one of
  these names.
- The test-only FAT reader in `crates/media` becomes product code.
- A user decides about deleting originals with the build's report in front
  of them. The program never deletes a source file.
- Files inside a built disk are not blobs. A later recipe that wants them
  finds them only when the library index can read FAT images in the store
  (recipes.md §13).

## Alternatives considered

- **Keep every input as a blob beside the artifact.** Fully lossless, and
  it doubles the store for every built set. Open as a later option for
  users who want it.
- **Rebuild archives on export.** Not reproducible for zip in general.
- **Record original file names in the config.** A new config key, which
  older releases refuse ([decision-30](decision-30%20-%20Published%20images%20declare%20the%20oldest%20unemupc%20that%20runs%20them.md)).
