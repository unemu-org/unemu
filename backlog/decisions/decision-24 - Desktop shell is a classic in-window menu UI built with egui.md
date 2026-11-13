---
id: decision-24
title: Desktop shell is a classic in-window menu UI built with egui
date: 2026-11-13
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-24 - Desktop shell is a classic in-window menu UI built with egui

## Status

Accepted — 2026-11-13. The outcome of the UX spike: machine images,
capabilities, and GUI mock-ups. The maintainer approved the mock-up and chose
the options below.

## Context

Today the CLI opens a bare window (winit + softbuffer), and every action is a
hotkey inside it. That makes three things hard:

- attaching floppies, and later CD/DVD media;
- changing machine settings without knowing which combinations are valid;
- browsing machine images.

The spike produced a design note (`backlog/docs/design/machine-images.md`) and
two rounds of static mock-ups. The first round was a web-style dashboard. The
maintainer asked instead for a classic Win32-style application window, and
approved the second round
(`backlog/docs/design/mockups/main-window.html`).

## Decision

1. **One classic application window.** It has a title bar, a menu bar (File,
   Machine, Media, View, Tools, Help), a toolbar, a client area and a status
   bar with power and drive activity lights.
   - The client area shows a welcome screen with the machine library and the
     details of the selected machine.
   - Starting a machine turns the same client area into the emulated screen.
   - Stopping it returns to the welcome screen.
   - Settings, media and layer actions are classic modal dialogs. Settings is
     a property sheet with tabs; invalid options are disabled and show the
     reason.
2. **The menu bar lives inside the window on every platform, macOS included.**
   The application looks the same everywhere. The macOS global menu bar keeps
   only the minimum (About, Quit).
3. **The classic skin is the default.** It has grey bevels and a gradient
   title bar. A flat "modern" skin with the same structure is available in the
   preferences. Light and dark themes apply to both skins.
4. **The shell is built with egui/eframe.** It is pure Rust and draws every
   widget itself, so it reproduces the classic skin on every platform. The
   emulated screen is one texture updated from the existing frame sink. It
   needs no web engine or IPC, and its build fits the Nix devShell. The
   deterministic core does not change: the shell talks to the machine only
   through the existing run, input, media and observation boundaries.
5. **Option sources.** The settings dialog gets its options and its "why
   disabled" reasons from a capabilities API owned by the assembly validator.
   The UI does not keep its own copy of the hardware rules.

## Consequences

- The winit + softbuffer window is replaced by the egui shell. Hotkeys stay,
  and are listed under Help › Keyboard shortcuts.
- Implementation starts with the shell skeleton (menus, toolbar, status bar,
  welcome to screen, machine files from the machines/ folders). Then come the
  media dialog on the existing floppy catalogue, the capabilities API and the
  settings sheet, and later the image store and the layer dialog from the
  design note.
- New GUI work starts from a mock-up that the maintainer approves first.
