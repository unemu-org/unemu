---
id: decision-39
title: A work file holds what the releases of one work share
date: 2026-11-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-39 - A work file holds what the releases of one work share

## Status

Accepted — 2026-11-17 by the maintainer, who asked for work files and their rules, and chose that metadata schema 2 records the work id and the synopsis (recipes.md Q7). Design step of `upc-5pw.113`; recipes.md §6.16.
Extends [[decision-33]] (the syntax) and relies on the lock of
[[decision-34]].

## Context

One recipe builds one artifact under one tag, so several releases of one
work each repeat the title block: the regional releases of a cartridge
(Contra in the USA and Japan, Probotector in Europe, a Rev 1), the
shareware and full versions of Ken's Labyrinth, the three disk formats of
FreeDOS 1.4. The maintainer calls this a very common case, above all on
consoles. The first round of the design had no sharing by file at all.

## Decision

1. **A work file, `<id>.work.toml`, beside the recipes**, holds what all
   releases of one work share. A recipe names it by id: `work = "contra"`.
2. **It holds `[work]` with `id`, and `[title]`, `[ids]` and `[needs]`.**
   Nothing else: no inputs, steps, outputs, licence or digest. One level
   only: a work file includes nothing.
3. **The work is the repository.** The last component of a recipe's `name`
   equals its work's id.
4. **Merge rules, per field.** A single value: the recipe's wins (the
   regional title). `developers`, `publishers`, `genres`: the recipe's list
   replaces the work's. `labels`: the union. `[ids]` and `[needs]`: per key,
   the recipe's wins. Release fields (`version`, `edition`, `year`,
   `regions`, `languages`, `description` and the like) are refused in a work
   file; `synopsis` is refused in a recipe.
5. **Synopsis and description.** The record's description is the recipe's,
   or the work's synopsis when the recipe states none.
6. **`upcli recipe show`** prints the merged recipe and marks what came from
   the work file.
7. **The lock wins.** A work file is not pinned by its recipes. A pinned
   recipe builds its locked metadata, so an edit to a work file never
   changes a published image; `check` lists the differences, and a new
   revision takes them up.
8. **A recipe artifact bundles its work file** as it was at pin time.
9. **The work file is not a registry artifact.** Later the catalogue index
   (`upc-5pw.65`) can carry a work entry per repository, derived from the
   newest revision's metadata.
10. **No general `include`.** The work file shares the description; the
    execution part is shared by content only.

## Consequences

- On the corpus a regional NES recipe falls from 36 lines to 27, and a
  further release of the work costs 27 lines instead of 36.
- Metadata schema 2 records the work id and the synopsis as fields of
  their own (the maintainer's answer to Q7), so the catalogue can derive a
  work entry.
- Two published recipes of one work may carry different copies of the work
  file; each agrees with its own lock.
- A label that not every release carries cannot be in the work file, since
  a release cannot remove one.
- A recipe file alone is no longer the whole description; `show` gives it.

## Alternatives considered

- **A general `include` with merging** (the earlier draft's fragments).
  Shares steps and inputs too, at the price of merge rules for everything,
  nesting, and one edit changing what several pinned recipes build.
- **No sharing.** Simple, and the maintainer found the repetition too
  costly for the common case.
- **Pin the work file by hash in each recipe.** Every correction would edit
  every recipe.
- **Lists always merge, or always replace.** Merging credits the wrong
  publisher to a region; replacing makes every release repeat `game`.
- **The work file as its own registry artifact.** A second source of truth
  for data each artifact already carries.
