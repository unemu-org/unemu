---
id: decision-44
title: Recipe format 1.1 adds machine templates and file inputs in fat steps
date: 2026-11-23
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-44 - Recipe format 1.1 adds machine templates and file inputs in fat steps

## Status

Accepted on 2026-11-24 after software review. The maintainer agreed to
`1.1` and both additions (`ufe-xle.2`, `ufe-uym.9.11`), approved the core
precondition of §3, and decided that published 1.0 recipes stay at 1.0,
that 1.1 ships whole in one front-end release, and that a 1.1 recipe may
name a 1.0 work file.

## Context

Decision-43 §6 and [recipes §9](../docs/design/recipes.md#9-what-binds-forever)
require a new minor without changing 1.0's bytes. Decisions 33–36 and 39 apply.

READ: recipe `spec.rs` reserves `[machine]`; FAT takes one tree.
`pc-image/src/create.rs:615` generates the templates' machine tables, and
:834 serializes stored image configs. It emits no standalone recipe input.

READ: unemupc is pinned to `472ababef9dbee6fdd552f7d873eaac79fd9864b`.
Its `crates/pc-config/src/schema/v1.rs` defines ROM image strings, byte hashes
and addresses (:119), drives (:176), and removable images (:197).
`machines/at-games-open.toml:28` names ROM files with explicit VGA/XTIDE
addresses; :77 names disk/media files. There are no typed recipe, package,
set-member or zero-disk references. `pc-config/src/origin.rs:73` resolves
paths; `lower.rs:37` lets a reader serve logical keys without host files.

## Decision

1. **Version 1.1 preserves 1.0.** A file opts in with `recipe = "1.1"` and
   remains TOML 1.0.0. The reader must build every 1.0 file with identical
   interpretation, step bytes and pinned manifests. Retain its schema,
   defaults, encoder profile, locks and tests. Never migrate or restamp on
   upgrade. A 1.0-only build refuses 1.1 with today's version finding,
   `"1.1" is not one of 1.0` (`parse.rs`); it never falls back to 1.0.
   Media work files keep decision-39's rules. A 1.0 recipe names a 1.0 work
   file only; a 1.1 recipe names a 1.0 or 1.1 work file, since 1.1 adds no
   work-file keys, so one work can hold releases of both versions.

2. **A machine recipe builds one template without guest execution.**
   `[machine]` requires `name`, `platform` and `version` (the SemVer output
   tag). It allows `digest` and `min_app_version`, but no release/revision.
   `[template]` requires `config`, a file-input id. It allows `disks`, an
   array of `{ id, size }` in exact bytes, and the template's own `title`,
   `description` and `labels`. Templates have no work files. File inputs
   retain size, SHA-256 and own-text bundling. `[license]` describes generated
   content and does not license imports.

   `[use.<id>]` requires exactly `image` and a manifest `digest`. Images must
   use immutable tags under existing rules: firmware uses `rN` or its
   release/revision form, and media uses `<release>-rN`. Refuse aliases such
   as `freedos:1.4` and `latest` even with a matching digest. Machine references
   would require SemVer, but 1.1 permits only firmware and media uses.

   This proposed recipe uses shipped firmware pins. Config size/hash and
   output digest are placeholders; acceptance uses emitted files and locks.

   ```toml
   recipe = "1.1"

   [machine]
   name = "unemu/open-at"
   platform = "ibm-pc"
   version = "1.2.0"
   digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000" # Placeholder.

   [template]
   config = "config"
   disks = [{ id = "system", size = 528482304 }]
   title = "Open AT 286"
   description = "An IBM 5170-class 80286 with open firmware only: the unemu AT BIOS (after Sergey Kiselev's 8088_bios), the unemu VGA BIOS and XTIDE Universal BIOS r638, each with its source archive. 640 KiB and 1 MiB of memory, VGA, a 1.2M A: and a 1.44M B: diskette drive and a blank 504 MiB fixed disk. No operating system."
   labels = []

   [license]
   expression = "CC0-1.0"

   [input.config]
   name = "open-at.toml"
   size = 1 # Placeholder; use the exact emitted byte length.
   sha256 = "0000000000000000000000000000000000000000000000000000000000000000" # Placeholder.
   bundle = true

   [use.bios]
   image = "unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1"
   digest = "sha256:c4c53a6a5c0727d8cccb26b22a92dcbf68e906aed7d9424be80763ea30533ff2"

   [use.vga]
   image = "unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1"
   digest = "sha256:c1972247ba5fc03dee5bc011b137e49c547ef1f4fc32dc3f74fa09b205c235e7"

   [use.xub]
   image = "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1"
   digest = "sha256:4dc25f1c00342e44b2476e84befc9dbbb1817d478cdda9b1fd0a5d8e026026a1"
   ```

3. **The core owns explicit bindings.** Drop the recipe firmware and media
   lists and derive closure from config references. Declaration order must
   not select roles, addresses or drives. Every ROM identifies a use, the
   package's logical ROM id, its role and physical address. An inserted
   medium identifies its drive, a use and a member such as `disk-1`.
   A fixed drive references a disk declaration by id and states geometry.
   Refuse unbound uses, wrong types or roles, ambiguous ids and size conflicts.
   A role is valid only when the bound package itself declares it for that
   ROM, in its catalogue entry or `[package.fits]`. The provider refuses a
   role the package does not declare, so a video BIOS can never be bound as
   the system BIOS.
   Media are optional to declare but required once referenced.

   This config excerpt omits other hardware tables. Its new fields await
   unemupc's schema decision. That design allows additive fields at schema 1,
   but the pinned parser refuses them. These are proposals, not current keys.

   ```toml
   schema = 1

   [firmware.bios]
   use = "bios"
   rom = "system-bios"
   role = "system-bios"
   base = 0xF8000

   [[firmware.option_roms]]
   use = "vga"
   rom = "video-bios"
   role = "video-bios"
   base = 0xC0000

   [[firmware.option_roms]]
   use = "xub"
   rom = "option-rom"
   role = "option-rom"
   base = 0xC8000

   [[card]]
   kind = "ide"
   controller_model = "ata"
   port = 0x1F0

   [[card.drives]]
   channel = "master"
   disk = "system"
   geometry = { cylinders = 1024, heads = 16, sectors = 63 }

   [[card]]
   kind = "fdc-765"

   [[card.drives]]
   unit = 0
   type = "1.2M"
   empty = true

   [[card.drives]]
   unit = 1
   type = "1.44M"
   empty = true
   ```

   For a media-bearing variant, replace A:'s `empty = true` with
   `inserted = { use = "dos", medium = "disk-1" }` and declare a pinned
   `[use.dos]`. This form is proposed too. The `system` geometry explicitly
   matches its recipe declaration: `1024 × 16 × 63 × 512 = 528,482,304` bytes.
   The games template's SB16 at `0x220`, IRQ 5, DMA 1/5, serial mouse and
   Lo-tech EMS at `0x260`/`0xD000` live entirely in config bytes.

   As a precondition, the binding lands in the core and the contract first,
   then unemu re-pins. The PC core's maintainers propose this shape:

   - `pc-config` gains one bound source form, not a family: a use id plus an
     item inside it (a ROM id, or a medium member such as `disk-1`), and
     `disk = "<id>"` for a declared zero disk. Lowering resolves bound items
     through a supplied table of content facts (id, digest, size, a byte
     reader), using the existing logical-key image reader. There is no second
     resolution path.
   - `role` and `base` are checked assertions against what the schema already
     determines (the system BIOS slot or an option ROM, the BIOS base derived
     from its size, the option ROM address). A mismatch is a diagnostic. The
     facts also carry the roles each package declares, so a role the package
     does not declare is refused.
   - Diagnostics cover an unbound use, an unused use, an ambiguous id, a size
     or digest conflict with the facts, a geometry that disagrees with the
     declared disk size, and an item that has both a path and a binding.
     A document may mix bound items and path items, because a user's machine
     may bind firmware and still use a local disk. A template recipe's config
     must use bound items only, because templates read no host files.
   - Old documents keep today's behaviour.
   - The seam is a contract operation (a later contract minor, such as 0.6),
     optional and `Unsupported` by default, like the 0.5 export. The core
     reports the binding slots a config document needs (use, item, kind and
     expected size where known). It composes the config bytes and the
     supplied facts into a validated, host-free prepared machine. The front
     end only hashes bytes and supplies facts. Other cores, such as a
     cartridge bound by digest, can use the same seam. The contract change
     needs its own decision record in unemu-core-api.

4. **Construction is deterministic and locks preserve its result.** Use
   the store or pull by manifest digest under the consent policy. Copy ROMs,
   corresponding source archives and referenced media into the verified
   closure, retaining rights and package provenance. Zero bases remain
   immutable behind the overlay and must match their bound geometry.

   The same recipe, inputs and app version must give the same digest, without
   clock, host paths, traversal order or host defaults. Sort unique unlocked
   layers by stored digest and refuse conflicting descriptors. READ:
   `create::finish` uses provider-reference order today. Locked order wins.
   Keep decision-34's three lock files, payload checks and lost/broken-lock
   rules. `machine.digest` pins the manifest.

5. **Names and publication retain existing rules.** Decision-35 maps
   `unemu/open-at:1.2.0` to `unemu/recipes/open-at:1.2.0`, without `machine/`.
   Reserve template names `media`, `firmware`, `machines` and every future
   kind segment under `unemu/recipes/`. A new kind must not shadow an existing
   template. Software's source directory may be `recipes/machine/`.
   Outputs keep SemVer and the guarded `latest` policy. Recipe tags match
   immutable output tags and have no `latest`. Recommend keeping published
   1.0 recipes at 1.0: no same-output tag replacement with 1.1. The maintainer
   must settle this restriction on decision-35's correction rule.

   Add `application/vnd.unemu.recipe.machine.v1`; retain existing types,
   annotation keys and recipe guards. Bundle only recipe, permitted texts
   and lock. Official outputs pass the guard before tagging and on push:
   every blob is open and copyleft ROMs carry source archives. Build never
   pushes. Software publishes templates; this repository builds test copies.

6. **The first publication carries locked compatibility markers.** Under
   implemented machine-images §5.6, `unemu.min-app-version` is the core floor;
   `unemu.min-frontend-version` rates packaging above the extraction baseline.
   Inherit firmware core floors and rate encodings in their own domain.
   A stated minimum can only raise the core floor. Rate preparation separately:
   the runnable output need not parse 1.1. Its recipe requires the first
   implementing front-end release. Refuse unsupported requirements before
   payload fetches and never restamp old locks.

7. **FAT gains direct file-input placements.** Keep the existing string/tree
   form unchanged and also accept a nonempty array:

   ```toml
   [[step]]
   id = "disk"
   fat = [{ from = "zip", dos_name = "APPEND.ZIP" }]
   format = "1.44M"
   order = "listed"
   ```

   Entries require `from` and `dos_name` and allow `time` under existing
   DOS local-time rules. Refuse other keys. `from` names one file input,
   not a step, tree or member. Its identity, download, consent, bundle and
   disclosure rules apply, and bytes are copied unchanged. `dos_name` is
   one uppercase root-level 8.3 name accepted by the volume profile.
   Refuse wrong references, paths, invalid names and stored-name collisions
   before payload reads, even for equal bytes. A later `dos_path` requires
   another minor; 1.1 readers refuse it.

   Listed order is array placement order; default name order sorts stored
   names. Entry time overrides the step fallback. Keep the existing rule:
   "A local date-time for entries that have none, for folders and for the
   label. Default `1980-01-01T00:00:00`." Without either time, use that FAT
   epoch. Preserve geometry, boot sector, serial, label, split and allocation
   by passing identical ordered path/bytes/time tuples to the volume profile.

   Alternatively, `{ path = "APPEND.ZIP", from = "zip" }` could replace
   size/hash in file sets, preserving FAT grammar but adding input dependencies.
   INFERENCE: direct placement is the smaller change. Recommend it and defer
   file-set references and general tree composition.

8. **Acceptance requires existing output bytes.** `ufe-xle.2` must emit
   exact machine-document bytes for both templates and their schema.
   Software keeps those bytes as file inputs. Emit the historical generated
   documents and stored configs as comparison evidence. Recipes using the
   emitted inputs must reproduce today's `1.2.0` digests in
   `pc-image/tests/template.rs::PINNED`. New bindings and sorting cannot
   excuse changed pins. Locks preserve historical names, bytes and layer
   order. Cross-check software's published template digests independently.

   Equal content, names, times, order and volume keys must give byte-identical
   volumes to the 1.0 two-input form. The earlier APPEND digest
   `sha256:ec6bd6f046193be4c62eba47922f38418c03f7793226d846b0c9138f73b56781`
   was a stand-alone draft. The published equivalent is
   `unemu/media/ibm-pc/freedos-append:5.0-0.6-r1`,
   `sha256:b793edeaf1710ba8152377d8166281c3f92434531db3fed37c9be510faac1f54`.
   It and the other 15 sets were verified anonymously after publication, and
   each recipe artifact can be pulled from
   `unemu/recipes/media/ibm-pc/<work>:<tag>`. Software supplies all 16 rewritten
   `recipes/media/freedos-*` recipes. Each must reproduce its published
   output digest without repinning. Acceptance files do not authorize
   replacing published 1.0 recipe artifacts.

9. **Schemas and reports change only as needed.** Generate
   `backlog/docs/design/recipe-schema-1.1.json` beside unchanged 1.0.
   Use separate version tables, test both for drift and distribute both
   with checksums. Retain the 1.0 corpus and add conformance for bindings,
   templates, guards and FAT equality.

   Changes to 1.0 plan JSON must be additive, without renaming, removing or
   reinterpreting fields. New rows describe artifacts and FAT `placements`
   (`from`, `dos_name`, `time`). Only array operands may use null; strings
   remain strings. Machine plans have no steps and one output with required
   dependencies. Do not add `schema_version`; this extension does not need
   it. Reports credit each input and destination without duplication.
   No build-report JSON command is added.

10. **Other features stay deferred.** Version 1.1 adds no guest Machinefile
    steps, including RUN, insert, start, wait, key, type or commit. It adds
    no processes, parents, installed systems or engine pins. Decision-36
    attachments remain unbuilt; templates have none. Other recipes §13
    features remain out, including general OCI sources and new formats.

## Consequences

Software owns template data and publication; the app keeps readers and tests.
Core bindings make hardware input data while preserving old source meanings
and output bytes. FAT gains a second operand form without input dependencies.
Published 1.0 recipes stay at 1.0 (maintainer, 2026-11-24): only new recipes
and new revisions use 1.1, so old front ends still read existing tags, even
at the same output digest.
Version 1.1 ships whole (maintainer, 2026-11-24): its first front-end release
carries both the FAT file placements and machine templates, never one
without the other. The 1.1 rows use the next-release value while in
development and take that release's actual version when it is cut,
independently of core floors.
A 1.1 recipe may name a 1.0 work file (maintainer, 2026-11-24), relaxing
rule O4 of recipes §4.6 for 1.1 only, so published 1.0 recipes and their
work files stay untouched when a work gains a 1.1 revision.
The core precondition of §3 is approved (maintainer, 2026-11-24). It
landed as contract `unemu-core-api` v0.6.0 with the optional
content-bindings seam, implemented by the PC core.

## Open questions

None.
