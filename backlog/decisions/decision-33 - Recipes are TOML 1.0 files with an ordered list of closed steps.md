---
id: decision-33
title: Recipes are TOML 1.0 files with an ordered list of closed steps
date: 2026-11-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-33 - Recipes are TOML 1.0 files with an ordered list of closed steps

## Status

Accepted — 2026-11-17 by the maintainer, after reading the corpus in both
syntaxes: TOML, experimental at `0.x`; HCL stays only as the documented
fallback of point 7. Design step of `upc-5pw.113`. The full argument, the
specification and the corpus are in `backlog/docs/design/recipes.md` §4 to
§6 and `backlog/docs/design/recipe-corpus/`. Experimental (`recipe = "0.x"`)
until the freeze of recipes.md §14.

## Context

Installation media sets and firmware packages are built today by one shell
script per recipe in unemu-media, plus a `media.toml` or `firmware.toml`.
The scripts run curl, unzip, mtools, git, tar and Python. They cannot be
given to a user for private media, cannot be run by the window, and every
digest they produce depends on one pinned `upcli` binary.

The maintainer asked for three file kinds in one syntax family, a Mediafile,
a Firmwarefile and later a Machinefile, each with a description part and an
ordered execution part from a closed set of actions. He distrusts exotic
languages (KDL), thought TOML could not carry ordered execution, and
expected the choice to fall between HCL and a format of our own.

The corpus has nine real recipes in TOML and in an HCL literal subset,
written from one data model. Measured: 1490 lines in TOML, 1565 in HCL; six
of the nine recipes need no step, and the longest step list has three.

## Decision

1. **A recipe is one TOML 1.0.0 file.** Syntax that only TOML 1.1 accepts is
   refused, so every TOML 1.0 parser reads every recipe. Unknown keys and
   values are refused at every level.
2. **Three kinds, one schema family.** The identity table says the kind:
   `[media]`, `[firmware]` or `[machine]`. A file has exactly one and
   builds one artifact under one tag.
3. **The description part** is plain tables: `[title]` or `[package]`,
   `[license]`, `[needs]`, `[ids]`.
4. **The execution part is `[[step]]`**, an array of tables, ordered by the
   TOML specification. A step has an `id` and exactly one verb key from a
   closed set; the verb set shares no name with any argument. Version 0.1
   has three verbs: `unpack`, `fat`, `text`.
5. **Steps have no working directory.** A step reads named values (a file
   or a tree) and makes one named value. A step may refer only to inputs
   and to earlier steps, so the written order is a valid execution order
   and every reference is checked before anything is read.
6. **The format cannot compute.** No variables, interpolation, conditions,
   loops, includes or commands. No step starts a process, and no later
   version will add one.
7. **The schema is syntax-neutral.** It is a tree of tables, arrays and
   scalars. HCL is the tested fallback: if the Machinefile corpus shows TOML
   to be unreadable for long step lists, the same schema moves to the HCL
   subset of the corpus.

## Consequences

- The maintainer's objection to TOML is answered on evidence, not by
  authority: ordered steps are arrays of tables, and the model removes most
  steps. The record should be rejected if the Machinefile corpus says
  otherwise.
- One JSON Schema gives editor completion and validation for all three
  kinds. A script with a standard library reads any recipe.
- Steps are wordier than in HCL or a line format: `[[step]]`, `id`, verb.
  References are plain strings; only the checker tells a name from a text.
- The checker must refuse TOML 1.1 forms that this repository's `toml`
  crate accepts.
- `media.toml` and `firmware.toml` of unemu-media map to the description
  part by renaming keys; `build.sh` maps to inputs and at most three steps.

## Alternatives considered

- **An HCL literal subset.** Reads better where blocks have a kind and a
  name, and Packer is prior art for machine builds. But the subset would be
  a language we define and police (expressions, functions and templates
  must be forbidden), its Rust parser has one maintainer and no 1.0, it has
  no schema mechanism for a custom dialect, and it is a second language in
  a project that writes everything else in TOML. Kept as the fallback.
- **A line format of our own, like a Dockerfile.** Best for long step lists
  and big tables, weakest for the description, and every rule of it would be
  ours to specify and keep. Looks familiar; is not.
- **YAML.** Ordered steps and many users, but implicit typing and parsers
  that disagree: the wrong base for a file that must mean the same for good.
- **KDL, JSON, XML, Starlark, Nix, Lua.** Unknown, no comments, unpleasant
  to write, or a program.
