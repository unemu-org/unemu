---
id: decision-43
title: Recipe format 1.0 frozen
date: 2026-11-21
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-43 - Recipe format 1.0 frozen

## Status

Accepted by the maintainer on 2026-11-21. Recipes remain unpublished
until a successful release conformance gate. This record
implements `design/recipes.md` §9, §14 and §19.2 B17, building on accepted
decisions 33–39. User documentation is deferred until the front-end split;
`backlog/docs/commands.md` carries the promises as the developer reference
and the later user documentation must take them over.

## Context

The software repository's migration supplies the real recipes and locks
(B14). The maintainer approved the independent private Windows 3.1 build
(B15) and the Machinefile syntax review (B16) on 2026-11-20. TOML stays:
the installation corpus demonstrated readable linear order and literal
strings despite repeated nested input headers. Those installations are
syntax evidence, not implemented Machinefiles.

The release must now preserve the interpretation and output of recipes
forever. Merely calling experimental `recipe = "0.1"` stable would evade
the explicit version boundary of recipes §9. Version 1.0 must mean
`recipe = "1.0"` in the file.

## Decision

1. **Version 1.0 is the implemented TOML 1.0 subset of recipes §6:**
   Mediafiles, Firmwarefiles and work files, checked against the generated
   `backlog/docs/design/recipe-schema-1.0.json`. That version's schema is
   closed: nothing is added to it later. This release refuses experimental
   `0.1` and unknown versions, including `1.1`; it does not interpret them
   as 1.0. Section 9 permits dropping `0.x`, which was never publishable.
   Every future release reads all versions from 1.0 through its own.
2. **The lock format is recipes §7.2:** three byte-preserved files,
   `<recipe>.lock/sha256/<hex>`, holding the output manifest, config and
   metadata. The recipe's output `digest` pins the manifest; its descriptors
   pin the other files and payloads. There is no new lock header or rewrite.
   A missing lock may be rebuilt only when the pinned digest is reproduced;
   a present broken lock is refused. Work-file drift warns by default;
   `check --deny-drift` and `build --deny-drift` refuse it for CI before
   inputs are read or an artifact is published. Conformance always builds
   the steps instead of accepting already built outputs as proof.
3. **The existing recipe artifact media types are the 1.0 types:**
   `application/vnd.unemu.recipe.media.v1`,
   `application/vnd.unemu.recipe.firmware.v1`,
   `application/vnd.unemu.recipe.v1+toml` (config),
   `application/vnd.unemu.recipe.work.v1+toml`,
   `application/vnd.unemu.recipe.input.v1` and
   `application/vnd.unemu.recipe.lock.v1`. The annotation keys of §7.1
   remain stable: `unemu.platform`, `unemu.recipe.output`,
   `unemu.recipe.output.digest`, `unemu.min-app-version` and the OCI
   title, description, licence and part-name annotations. The recipe's
   major version selects its media type; a new major gets new media types.
4. **The work-file format is §6.16 and decision-39:** `<id>.work.toml`,
   `recipe = "1.0"`, `[work]`, `[title]`, `[needs]`, `[ids]`, with its
   existing per-field merge rules. It shares descriptions only, includes
   nothing, matches the recipe's version and repository id, and travels
   with the recipe artifact. Locked metadata wins over later work edits.
5. **The remaining FAT defaults are retained, not redesigned:** the IBM PC
   FAT12 geometries and strict DOS names, data-disk boot sector that asks
   for replacement and retries INT 19h, fixed serial `0x28959855`, name
   order (files then folders, by stored 8.3 bytes), epoch fallback time
   `1980-01-01T00:00:00`, no long-name entries, and the current exact-volume
   split and cluster allocation. Explicit listed order, per-entry times,
   serial and label keep their existing interpretation. The current
   synthetic volume vectors and pinned corpus guard their bytes.
6. **Compatibility follows all nine promises of §9.** Input identity is
   SHA-256 plus exact size forever. The schema and step bytes are fixed per
   recipe version. New keys, steps, formats or corrected step bytes need a
   new minor; existing versions keep their behaviour and tests. Grammar or
   an existing meaning changes only with 2.0 (or a later major) and new
   media types. Stored `+zstd` bytes keep named encoder profile 1; another
   encoding needs another profile marker. Old configs, metadata and
   manifests remain readable, but their writers need not stay unchanged:
   locks preserve the author's bytes. Tag grammar, `rN`, and the `recipes/`
   mapping stay fixed. Provider names are append-only and never change
   meaning. Export remains layout version 1, with `EXPORT-VERSION`.
7. **What stays open:** all later features of §13, including Machinefiles,
   remain outside 1.0. The Machinefile corpus settles syntax only; §14
   does not freeze its assumed keys, waits, input actions or placeholder
   locks. Attachments remain specified by decision-36 but are not built by
   1.0. Their existing declared keys and meaning remain stable; later
   implementation does not permit additions to the 1.0 schema. The later about-text extensions (including the text-file media
   type) are not silently added to 1.0. New supported recipe keys require
   a minor version. Serialization, annotation writing order, default
   filenames and calculation of compatibility markers may change because
   the lock records their result. Plan/report JSON is separately versioned;
   library indexes, resolution order and consent policy remain open.
8. **Publication and release checks:** the generated schema is committed,
   compared byte for byte by `pc-image`'s `recipe_check` drift test and
   attached as `recipe-schema-1.0.json` to each binary release, with its
   checksum. CI and the release workflow must run `upcli recipe conformance`
   over the six shipped locked recipes and the remaining pinned Ken's
   Labyrinth 2.01 recipe, with `--library vendor/freedos --library vendor`.
   They never pass `--allow-drafts`: a draft, missing input or differing
   digest blocks release. Historical unpinned design examples are not the
   release corpus. Adding a pinned recipe requires extending this gate.

## Consequences

The version bump changes recipe text hashes, recipe OCI config/manifest
identities, bundled work-file layer digests and local `unemu.build.recipe`
provenance. These are distinct from the output pins. No lock, output
`digest`, template pin or payload is deliberately regenerated or changed.
The lock contains no recipe-version field and pins existing output bytes;
conformance must establish that those pins survive the bump. Any measured
output-pin change requires the maintainer's decision before proceeding.

Existing 0.1 recipes migrate by changing `recipe = "0.1"` to `recipe = "1.0"`
in each recipe and its work file, then running `upcli recipe check` before
pinning. For the software repository's existing recipes, only those version
lines change: their output digests and locks stay unchanged. Coordinate
that migration before upgrading the repository's pinned `upcli`; the 1.0
reader refuses experimental 0.1 files. This work does not edit or publish
that repository.
The existing 0.1 schema and HCL snapshots remain historical evidence;
Machinefile sketches remain experimental.

The release gate intentionally fails closed if fixture inputs are missing.
The release fixture set must supply Ken's `LABFULL.ZIP` (940242 bytes,
SHA-256 `643aa0ca8d1a7d2c88afecd952da00f3b912be8b83c4c1d2f4ab9b50491fadfd`)
or all its individually pinned members, and the recipe's original licence
index (3442 bytes, SHA-256
`b4b52913919b8f481289fe0beef2a76a812b4014e6a2b09357caa2aec91f4ff7`).
The licence index is bundled beside the corpus recipe, copied byte for byte
from the software revision `3c493f7`. The shared CI and release fixture lane
runs `vendor/kenslab/fetch-labfull.sh` to download the author's archive and
verify its size and SHA-256 before conformance, which runs without `--fetch`;
substituting another licence text or repinning is not an acceptable fix.

Acceptance must confirm this version boundary, the preserved FAT defaults,
the deferred user-documentation handover and fixture provisioning in CI.
Until then this is a proposed freeze, not permission to publish recipes.

## Preparation evidence (2026-11-20)

MEASURED: the new version-boundary regression failed against the original
parser (`1.0 is not one of 0.1`), then passed after the version change.
The focused `recipe` suite passes 73 tests; `pc-image`'s `recipe_check`
passes 38 and `recipe_lock` passes 17, including a rebuild of the six
shipped recipes from required fixtures at their unchanged output digests.
A separate SHA-256 audit against the worktree's original commit finds all
seven output pins identical and all 21 lock blobs byte-identical, with each
blob still hashing to its filename. After restoring Ken's bundled licence
index, the seven-recipe conformance run below also reproduces its output pin.

MEASURED: the affected-test planner initially failed its existing source
reader check: the standalone `unemupc` crate was absent from the shared
fixture-support and shipped-machine reader lists. Two reader-list additions
fix this pre-existing gate blocker; the strengthened scenarios failed
before that fix and the runner self-test now passes. The new 1.0 schema
scenario also verifies that editing the published schema selects its checker.
A command probe confirms the CI lane selects both corpus locations and the
two vendor libraries, omits `--allow-drafts`, and propagates a failing command
through its log pipeline. Hosted release execution remains unmeasured.

MEASURED: before restoring Ken's bundled licence index, the seven-recipe
CLI conformance run against only the vendor libraries exited 1: six
conformed, zero differed, one was not built, zero drafts were skipped.
After the byte-exact restoration it exits 0: seven conform, zero differ,
zero are not built, zero drafts are skipped. The sandbox run uses explicit
scratch store and preferences paths and no `--fetch`; all 19 game members
were already available locally. No output pin changed. The new archive
fetch could not resolve `advsys.net` in the sandbox, so downloading it in
the shared fixture lane remains unmeasured.
The actionable 0.1 migration diagnostic regression failed against the
generic refusal before the fix, then passed with all 73 recipe tests.
The schema-mapping scenario also fails under a mutation restoring the old
0.1 path, and passes after restoring the 1.0 rule.
