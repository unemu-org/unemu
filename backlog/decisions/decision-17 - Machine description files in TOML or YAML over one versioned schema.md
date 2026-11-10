---
id: decision-17
title: Machine description files in TOML or YAML over one versioned schema
date: 2026-11-10
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-17 - Machine description files in TOML or YAML over one versioned schema

## Status

Accepted — 2026-11-10. Part of phase A′ of [[decision-16]]. The design, with
the field-by-field schema, crate evaluation, examples and reserved names, is
[`backlog/docs/design/machine-files.md`](../docs/design/machine-files.md);
firmware, board models and machine classes are in
[`backlog/docs/design/firmware-and-machine-classes.md`](../docs/design/firmware-and-machine-classes.md).
The implementation work lives in beads under the `consolidation` epic.

## Context

A `MachineConfig` is already declarative ([[decision-7]]), but machines are Rust
presets and the CLI and the window hard-code them. The goal line runs through
several hardware generations, and users who care about period hardware expect
to compose a machine — board, firmware, cards, drives — without recompiling.
The format must not weaken determinism or create a second validation path.

## Decision

1. **One schema, two syntaxes.** A machine is a file with `schema = 1` at the
   top, written as TOML or YAML; both deserialize into one serde data model
   (crates `toml` and `serde-saphyr`) in a `machine-file` crate above
   `assembly`. `assembly` itself gains no serde dependency.
2. **The file describes hardware only.** `[board].model` (e.g. `ibm-5170`, a
   generic AT; later `ibm-5160`, `i440fx-piix3`) supplies the motherboard
   devices and their wiring; the machine class follows from the board. Every
   device in an ISA slot is a `[[card]]` whose `kind` gives default ports,
   IRQ, DMA and memory windows, each overridable. `[cpu]`, `[memory]` and
   `[firmware]` (images, option ROMs, and the services they `provide`, such as
   `fixed-disk`) complete the machine. Drives nest under their controller card
   with a `write` mode of `volatile` (the default), `readonly` or `journal`.
   RunPolicy, presentation and host-stream bindings stay outside the file.
3. **Paths and integrity.** Paths in a file are relative to that file. Images
   may pin a `sha256`; a mismatch is an error that prints both hashes. Images
   are read once, hashed and carried inline into the `MachineConfig`.
4. **Inheritance.** `extends` names a file or an embedded preset; tables merge
   field by field and cards merge by `id`. A card's `id` defaults to its
   `kind`; a second card of the same kind (e.g. a second serial port) must
   name an explicit `id`. Cycles and excessive depth are errors.
5. **Strictness.** Unknown fields are errors with the file, line and field
   path. Cycle-exact timing fields are rejected with an explanation
   ([[decision-16]] non-goal). Every reserved name in the design's list is fixed
   now and rejected until it is implemented, so new hardware is a new card
   kind, not a new schema version. Only a breaking change bumps `schema`, with
   a migration function per version.
6. **One validation path.** A file lowers to a `MachineConfig` that passes
   through the existing `validate()`. Presets become files under `machines/`,
   round-trip tested against the Rust presets, and the goldens load them.
   Rust test rigs may stay Rust presets on a `Bare` board.
7. **Device order is part of the machine.** The bus serves due devices in bus
   order, so a board model fixes its devices' order and cards follow in file
   order. The current BIOS preset is first reordered to that canonical order,
   proven by unchanged goldens.
8. **Frontends.** The window and the CLI take `--machine <file>`; the window
   shows the first display card unless `--display` names another.
9. **Toolchain.** The workspace follows the latest stable Rust that the Nix
   devShell provides; `rust-version` is raised with it (the YAML crate needs
   1.89 or later).

## Alternatives considered

- **Rust presets only.** No recompile-free composition; rejected as the goal
  line needs user-composed machines.
- **A `class` field instead of a board model.** Real firmware is tied to its
  board and chipset; a class cannot carry chipset registers or a firmware
  compatibility list. Rejected for named board models.
- **RON or JSON.** RON is Rust-specific; JSON has no comments. The maintainer
  asked for TOML and YAML.
- **serde_yml / serde_yaml_ng.** An open advisory and an archived repository,
  and an unmaintained unsafe parser underneath, respectively. Rejected for
  serde-saphyr, with serde_norway as the fallback.

## Consequences

- (+) Machines become data that users can share and vary; goldens and the
  window run the same files.
- (+) Adding hardware is additive under schema 1.
- (−) A schema is a compatibility promise: reserved names and merge rules are
  fixed early and are hard to change later.
- (−) A new crate and two parser dependencies; cargo deny and the MSRV move
  with them.

## Trigger to revisit

A breaking schema change is needed; the YAML crate stops being maintained; or
the board-model approach cannot express a phase C/D machine.
