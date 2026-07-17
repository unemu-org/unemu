---
id: TASK-23
title: M5a — Complete the 8042 AT keyboard-controller command set for the BIOS
status: To Do
assignee: []
created_date: '2026-07-17 21:49'
labels:
  - devices
  - dos
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The Kiselev Xi8088 BIOS runs the full AT 8042 init handshake (task-19 finding): controller self-test 0xAA, keyboard-interface test 0xAB, aux test 0xA9, read/write command byte 0x20/0x60, enable/disable keyboard+aux 0xAD/0xAE/0xA7/0xA8, read input port 0xC0, aux route 0xD4; and the keyboard must return BAT 0xAA after reset. Audit our i8042 against this set and fill the gaps so BIOS kbc_init completes without hanging. Also: PS/2 mouse is probed (PS2_MOUSE on) — the aux channel must fail gracefully (timeout, no hang) since we model no mouse, or plan a custom BIOS build dropping PS2_MOUSE.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 i8042 handles the full AT command set the BIOS issues (self-test, interface tests, command byte, enable/disable, read input port, BAT)
- [ ] #2 Aux (mouse) probe fails gracefully — no hang — with no mouse modelled
- [ ] #3 BIOS kbc_init runs to completion in an integration test / the BIOS boot
<!-- AC:END -->
