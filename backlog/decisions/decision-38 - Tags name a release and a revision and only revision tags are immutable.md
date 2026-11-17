---
id: decision-38
title: Tags name a release and a revision and only revision tags are immutable
date: 2026-11-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-38 - Tags name a release and a revision and only revision tags are immutable

## Status

Accepted — 2026-11-17 by the maintainer (recipes.md Q4): `<release>-rN` immutable, `<release>` a moving alias, the digit-first rule retired, firmware tags `rN`. Design step of `upc-5pw.113`; recipes.md §8. Changes
the set-tag rule of `backlog/docs/design/machine-images.md` §5.2 (Tags) and
the firmware tag of §5.5; keeps the template rule. Uses [[decision-32]],
point 5.

## Context

Tags should carry the software's version (`freedos:1.4`), and a public tag
must not move. But our packaging of the same software changes: a corrected
description, a new label, an added licence text. Some things have no
version: an NES cartridge has a region and sometimes a revision.

Today a set tag is `VERSION[-EDITION][-FORMAT][-rN]`: every tag that starts
with a digit is immutable, the first packaging has no `-rN`, and later ones
are `-r2`, `-r3`. Firmware tags are whole numbers. Templates use semantic
versions and `latest`. The name a person types, `freedos:1.4`, is therefore
the first packaging for ever.

## Decision

1. **A tag is `[<release>-]r<N>`.** The release names the software; `rN`,
   from 1, counts our packagings of that release. It is the Debian
   revision, the RPM release and the `-rN` of Alpine and Gentoo.
2. **A release** is parts joined by `-`, each `[a-z0-9]+([._][a-z0-9]+)*`
   and none of the form `r` plus digits. By convention the parts are, in
   order and only when they apply: version, edition, region, language, and
   the format when it is not the release's own. Software with none takes
   its year. A thing nobody has identified has no release.
3. **A tag that ends in `rN` never moves**, locally or on a registry. Every
   other tag is an alias.
4. **The release alone is an alias** for its newest revision:
   `freedos:1.4` follows `1.4-r1`, then `1.4-r2`. Locks, workspaces and
   Machinefiles pin `rN` tags and digests.
5. **No `latest`** for media sets and firmware packages.
6. **No program reads meaning out of a tag.** Version, edition, regions and
   formats are fields of the title record and the config.
7. **Per kind:** media sets `<release>-rN`; firmware packages `rN`, with a
   release only when one catalogue entry has packagings that differ in
   content; machine templates keep semantic versions and `latest`; a recipe
   has its output's tag.
8. **A metadata change is a new revision.** Its config and media are the
   blobs the old one has, so a client fetches two small blobs, and equal
   config digests tell it that only the description changed.
9. **OCI referrers are for additions, not corrections:** optional parts,
   covers, manuals, scans. The record itself travels inside the artifact.

## Consequences

- `freedos:1.4` always means the best packaging of FreeDOS 1.4.
- The digit-first rule for set tags is retired. Releases 0.0.1 and 0.0.2
  keep it and refuse to follow an alias once an `r2` exists; accepted for
  preview releases, as [[decision-32]] accepts their other limits.
- Firmware tags change from `1` to `r1`, in the push that renames the
  platform.
- unemu-media moves the alias in the push that publishes a revision.
- Whether two dumps are one release or two is the author's judgement, and
  the naming conventions live in unemu-media's guide, not in code.

## Alternatives considered

- **Today's scheme.** The plain tag is the oldest packaging.
- **Semantic versions for everything.** Media have upstream versions that
  are not semantic, or none.
- **Metadata as a referrer of an unchanging content artifact.** The record
  would live beside the artifact: lost in an archive, and unavailable on a
  registry without the Referrers API.
- **Derive the tag from fields by a fixed rule.** Every rule about editions
  and default formats would bind for good; a written release needs only a
  grammar.
