---
id: decision-32
title: The PC platform id is ibm-pc and ibm-pc-at stays a read-only alias
date: 2026-11-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-32 - The PC platform id is ibm-pc and ibm-pc-at stays a read-only alias

## Status

Accepted — 2026-11-17 by the maintainer, before the first public release.
Amends [[decision-25]] (machine images), [[decision-26]] (installation media
sets) and [[decision-27]] (firmware packages), which name the platform
`ibm-pc-at`, and uses [[decision-30]] (the oldest release that runs an image).

## Context

Every machine image, installation media set and firmware package names the
machine family it belongs to, in the manifest's `unemu.platform` annotation and
in its config and metadata. The platform routes an artifact to the emulator
that provides it; machine-images.md defines it as a machine family, not a CPU
or a model. For the PC the id has been `ibm-pc-at`.

The family unemupc emulates is wider than the IBM 5170. A DOS title runs on
any IBM-compatible PC whose drives take its media: the PC and XT, the AT, the
PS/2 models, and the 386, 486 and Pentium machines on the project's goal line.
`ibm-pc-at` reads as the AT alone, so it would mislead every author of a
recipe and every reader of a manifest for as long as the format lives. What a
title needs from that family (a 386, VGA, a Sound Blaster) is a separate,
per-title statement, not the platform.

The id is in six published artifacts on registry.unemu.dev (the three
`unemu/media/freedos` sets and the three firmware packages) and in the local
stores of releases 0.0.1 and 0.0.2, which compare it exactly. Renaming gets
more expensive with every artifact published, so it is done now or never.

## Decision

1. **The PC platform id is `ibm-pc`.** It names the IBM PC compatible family
   that unemupc emulates, from the PC and XT through the AT and PS/2 models to
   the later 386, 486 and Pentium machines. Displays call it "IBM PC
   compatible".
2. **`ibm-pc-at` is a read-only alias of `ibm-pc`, kept for good.** Every
   reader treats the two as the same platform: the manifest annotation, the
   config and metadata `platform` fields, firmware fit statements, provider
   routing and every comparison between them, so an artifact that says
   `ibm-pc-at` reads, lists, pulls, runs and combines with one that says
   `ibm-pc`. No writer ever writes `ibm-pc-at`.
3. **Every writer writes `ibm-pc`.** New machine, Save as new image, import,
   media set and firmware import, template build, compress and catalogue
   edits all write the new id, also when they derive from an `ibm-pc-at`
   artifact.
4. **The new id is a feature with a row** in the decision-30 table: it is
   first read by release 0.0.3, so every artifact written with it carries
   `unemu.min-app-version` 0.0.3 or higher. Releases 0.0.1 and 0.0.2 ignore
   the marker and refuse such an artifact at pull, after the download, as
   decision-30 point 8 accepts for preview releases.
5. **Published artifacts are never rewritten.** The six published ones keep
   their tags and digests and stay readable through the alias. unemu-media
   publishes `ibm-pc` versions as new revisions (for example
   `unemu/media/freedos:1.4-r2`, firmware revision 2) once a release that
   reads them exists; this repository's pinned digests of the builders it
   still has move to the new values.
6. **Local stores are not migrated.** Images, sets and packages already in a
   store keep their bytes and digests; machines pinned to them keep working.

## Consequences

- One normalising function decides platform equality; exact string
  comparisons of platform ids are bugs. Tests cover an `ibm-pc-at` artifact
  read, listed, pulled, run and combined with `ibm-pc` firmware and media.
- Every digest this repository pins for templates, firmware packages and
  media sets changes once, in the change that renames the id.
- The per-title hardware requirements (a NEEDS statement in the build-file
  design, `upc-5pw.113`) describe what a title needs within the family; the
  platform id says nothing about CPU, video or sound.
- A future XT-class machine profile belongs to the same platform.

## Alternatives considered

- **Keep `ibm-pc-at` and document it as the family.** Free, but the name
  contradicts its meaning in every manifest and recipe forever.
- **Rename without an alias.** Breaks every published artifact and every
  local store. Rejected.
- **Rename later.** Each published artifact adds a revision to republish and
  a digest to move; after the first public release the alias would also have
  to cover releases users depend on.
