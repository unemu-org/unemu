---
id: decision-35
title: Recipes are OCI artifacts in a recipes namespace beside their outputs
date: 2026-11-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-35 - Recipes are OCI artifacts in a recipes namespace beside their outputs

## Status

Accepted — 2026-11-17 by the maintainer: the platform in repository paths and the `unemu/recipes/…` namespace, now, with the `ibm-pc` push (recipes.md Q2). He confirmed explicitly: a recipe tag is replaceable only by a recipe that pins the same output digest; one artifact type per recipe kind; the `disclosable` guard for files under 256 bytes in published user-supplied recipes. Design step of `upc-5pw.113`; recipes.md §7 and §8.3.
Uses [[decision-32]] (point 5: the published artifacts are replaced once
before the first public release).

## Context

We publish recipes without payload. A user pulls one and builds the image
locally when their files match. The Remotes view gets a Recipes group with
states such as "can build: all 12 inputs found" and "missing 3 of 12". A
recipe therefore needs a place on a registry, a shape a client can read
without payload, and a rule that ties it to its output.

Repositories today are `unemu/media/<work>` and `unemu/firmware/<catalogue
id>`, with no platform in the path. The family is about to get a second
platform.

## Decision

1. **A recipe is an OCI artifact** with one `artifactType` per kind,
   `application/vnd.unemu.recipe.media.v1` and
   `application/vnd.unemu.recipe.firmware.v1`, because a registry filters a
   listing by artifact type alone. Its config is the recipe file
   (`application/vnd.unemu.recipe.v1+toml`). Its layers are the bundled
   inputs and the three lock files, each with its file name in
   `org.opencontainers.image.title`.
2. **Annotations** carry what a list needs: `unemu.platform`,
   `unemu.recipe.output` (name and tag),
   `unemu.recipe.output.digest`, title and description,
   `unemu.min-app-version` (the oldest release that can build it) and
   `CC0-1.0` as its licence.
3. **A bundled input is an ordinary input** with a hash and a size; the
   recipe's folder or artifact is the source that has it. Only our own
   text, at most 1 MiB each.
4. **A recipe's repository is its publisher's namespace, `recipes/`, and
   the output's path after its first component; its tag is the output's:**
   `unemu/recipes/media/ibm-pc/freedos:1.4-r1` builds
   `unemu/media/ibm-pc/freedos:1.4-r1`. A recipe without a licence
   statement names its output under `private/`:
   `unemu/recipes/media/ibm-pc/windows:3.0-1.2m-r1` builds
   `private/media/ibm-pc/windows:3.0-1.2m-r1`.
5. **Repositories gain the platform:** `unemu/media/<platform>/<work>` and
   `unemu/firmware/<platform>/<catalogue id>`. Machine templates keep
   `unemu/<name>`.
6. **A recipe tag may be replaced, but only by a recipe that pins the same
   output digest.** The output's digest is the identity; the recipe is one
   way to reach it, and a better way (a new mirror, another database id)
   replaces the old without a new revision of the artifact. The push guard
   enforces the rule. The local tag of a built artifact records the digest
   of the recipe that built it.
7. **The guard for recipes:** the artifact may hold the recipe, bundled
   texts and the lock, and nothing else. A recipe without a licence
   statement that names a file under 256 bytes is refused unless the entry
   says `disclosable = true`: the hash of a tiny file gives the file away.
8. **No recipe is published before the freeze** of recipes.md §14.

## Consequences

- A client decides "can I build this" from a manifest and a config of a few
  kilobytes and its own library index.
- Recipes for private media are public while their outputs never are; one
  namespace gives that its own access rule.
- Every published repository moves once. The six published artifacts are
  deleted and pushed again for `ibm-pc` anyway, so this costs one more
  change in the same step and nothing later.
- Recipe tags are the one kind of public tag that moves after the release.
  The rule that limits the move is checked by the client, not by the
  registry.
- Remotes needs a mock-up of the Recipes group before any GUI work.

## Alternatives considered

- **The recipe as a referrer of its output** (`subject`). Right for open
  media, impossible for private ones, whose output is on no registry.
- **The recipe in the output's repository under a suffix tag.** Mixes two
  artifact types in one tag list and gives no namespace for an access rule.
- **Immutable recipe tags with their own counter.** A third counter beside
  release and revision, for a thing whose identity is the output's digest.
- **Keep `unemu/media/<work>`.** Collides as soon as one work exists on two
  platforms.
