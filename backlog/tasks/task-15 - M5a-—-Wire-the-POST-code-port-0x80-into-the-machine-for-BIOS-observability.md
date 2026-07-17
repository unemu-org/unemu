---
id: TASK-15
title: M5a — Wire the POST-code port (0x80) into the machine for BIOS observability
status: To Do
assignee: []
created_date: '2026-07-17 20:28'
labels:
  - devices
  - machine
  - dos
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The PostPort device already exists (added for test386, decodes 0x190). BIOS POST writes stage codes to port 0x80. Wire a PostPort (or a second instance) at 0x80 into the AT machine so BIOS progress is a deterministic capture, mirroring run_test386. This is the observability mechanism for the BIOS-boot golden.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Port 0x80 decoded to a POST-capture device in the AT/BIOS machine
- [ ] #2 A run_* helper drains the 0x80 code stream via the tracing seam
- [ ] #3 Unit/integration test on the capture path
<!-- AC:END -->
