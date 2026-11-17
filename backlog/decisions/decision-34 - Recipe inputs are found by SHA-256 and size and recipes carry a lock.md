---
id: decision-34
title: Recipe inputs are found by SHA-256 and size and recipes carry a lock
date: 2026-11-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-34 - Recipe inputs are found by SHA-256 and size and recipes carry a lock

## Status

Accepted — 2026-11-17 by the maintainer: he chose the frozen encoder profile named in the lock (recipes.md Q3), took the FreeDOS source archive as a prepared input (Q6), and asked for work files on top of the lock ([[decision-39]]). He confirmed the lock itself explicitly, and decided that the `+zstd` suffix of the version-1 media types is encoder profile 1: no manifest byte changes, and a later profile gets a new marker. Design step of `upc-5pw.113`; recipes.md §3 and §9.
Builds on [[decision-25]], [[decision-26]], [[decision-27]] and
[[decision-30]].

## Context

The core feature the maintainer asked for: a recipe finds its inputs by
content, not by name or path. A recipe for private media holds only hashes,
names and our own text; the user supplies the files from a library of their
own. If every required input matches, the build runs; otherwise nothing is
built and a report says exactly what is missing.

A second need is permanence. A recipe published once must build the same
digest with every later `upcli`. Today a set's digest depends on how one
release spells TOML and JSON and which annotations it writes; unemu-media
pins one `upcli` binary for that reason. A user's later `upcli` cannot be
pinned.

## Decision

1. **An input is its SHA-256 and its exact size.** A canonical name, other
   names, weaker hashes (`crc32`, `md5`, `sha1`), database ids and URLs are
   hints. A file is used only after its whole content has been hashed.
2. **A file-set input** lists many loose files as `{ path, size, sha256 }`
   and gives a tree. Each file is found on its own.
3. **Sources, in order:** the store (decoded digest), the download cache,
   the files beside the recipe, the user's libraries, and URLs. A URL is
   fetched only with the user's consent for that recipe or publisher, up to
   the pinned size, over `https`.
4. **A library** is a folder the user names. Its index is a rebuildable
   cache that records path, size, change time and computed hashes, and reads
   the directories of zip archives. A scan hashes only files whose size
   equals a wanted size. A catalogue (a CSV of path, size and SHA-256, a
   checksum file, a Logiqx DAT with its folder) may stand in for a scan. An
   index or catalogue proposes candidates and never proves anything.
5. **Required follows from the steps.** An input is required when a part of
   the main artifact depends on it. There is no `optional` flag.
   **A value with a stated hash and size is found instead of made:** a
   member of an archive, a blob of a locked output. An archive is opened
   only for members still missing. So a build gives the same bytes whichever
   source supplied a value, and nothing but bytes is read from an archive:
   order and times are written in the recipe.
6. **All or nothing.** With one required input missing, no tag and no
   artifact is written. `recipe plan` reports every missing input with
   name, size, SHA-256, database ids, what it is for, and any file that has
   the name but another hash.
7. **A pinned recipe carries a lock:** the output's manifest, config and
   metadata, byte for byte, as the author's `upcli` wrote them. The recipe's
   `digest` names the manifest. A build makes the payload blobs, checks each
   against its descriptor in the locked manifest, compares the lock's
   decoded fields with the recipe's description, and publishes the locked
   manifest unchanged.
8. **A recipe without `digest` is a draft.** It builds and is tagged
   locally; nothing is checked against a pin.
9. **What binds forever** is the list of recipes.md §9: the hash identity,
   the schema and the bytes each step writes per recipe version, the
   payload encoding a lock names, provider vocabularies (append-only). How
   a release writes configs, metadata, manifests and annotations does not
   bind.
10. **Versions.** `recipe = "MAJOR.MINOR"` selects the schema and every
    step's bytes. A fix that changes bytes is a new minor; the old behaviour
    stays. Every release builds the conformance corpus of every version it
    supports and compares digests.

## Consequences

- A recipe for private media publishes no payload byte, and its lock holds
  only digests, sizes, labels and our own text.
- Later releases never have to write an old encoding again, only read it,
  which [[decision-30]] already requires.
- The payload builders (the FAT builder, the zip reader, the text step)
  become versioned code with golden tests. A refactoring that changes one
  byte is a failed release test, not a surprise for users.
- A `+zstd` payload's stored digest depends on the encoder. One encoder
  profile is frozen, named in the lock and pinned by test vectors (the
  maintainer's answer to Q3, 2026-11-17). A later encoder is a new profile
  beside it.
- An output v1 cannot build, such as the FreeDOS source archive, is a
  prepared input named by hash; an `oci` source may serve it later (Q6).
- Import ROM folder… and Locate firmware become one case of the library
  index.
- The description exists twice, in the recipe and in the lock. The build
  compares them field by field.

## Alternatives considered

- **Find by name or path.** What every script does today; useless for
  private media, where each user's names differ.
- **Pin only the manifest digest and let each release rewrite the
  manifest.** Simple, and broken by the first release that adds an
  annotation or changes a serializer.
- **Pin the `upcli` version in the recipe.** Reproducible, but a user would
  need every old release for good.
- **An `optional` flag on inputs.** It can contradict the steps.
- **Trust the index.** Faster, and wrong the first time a file changes.
- **Freeze the writers of configs, metadata and manifests instead of
  carrying a lock** (the earlier draft's choice). No extra files, but every
  writer and the annotation set bind for good.
