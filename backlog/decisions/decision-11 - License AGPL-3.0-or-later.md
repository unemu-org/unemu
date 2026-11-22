---
id: decision-11
title: License — AGPL-3.0-or-later
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

unemu maintains the application portions; unemupc maintains the core portions of this shared historical decision.

# decision-11 - License: AGPL-3.0-or-later

## Status

Accepted — 2026-07-17.

## Context

A research project that should keep derivatives open, including network use.

## Decision

License the project under **AGPL-3.0-or-later**. `LICENSE` holds the full text.
Each source file carries an SPDX header
`// SPDX-License-Identifier: AGPL-3.0-or-later` (a convention in `conventions.md`).

## Consequences

- (+) Strong network copyleft; derivatives and hosted use stay open.
- (−) May limit adoption by permissive/proprietary downstreams — acceptable given
  the research goal.

Note: the CPU core `x86jit` is a separate project under its own license; this
decision governs only `unemupc98`.

## Trigger to revisit

If a collaboration or distribution need requires a different license, record a
superseding decision.
