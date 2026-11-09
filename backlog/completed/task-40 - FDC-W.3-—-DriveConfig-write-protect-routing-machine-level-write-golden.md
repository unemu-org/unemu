---
id: TASK-40
title: FDC-W.3 — DriveConfig write-protect routing + machine-level write golden
status: Done
assignee: []
created_date: '2026-07-18 21:59'
updated_date: '2026-07-18 23:46'
labels:
  - devices
  - config
  - test
dependencies:
  - TASK-39
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Config + integration layer on top of the device work. Route a write-protect flag from assembly::DriveConfig (crates/assembly/src/drive.rs) through assemble() to the Drive flag added in FDC-W.2 (default: writable, real-HW default — Fable Q4). Machine-level golden: a small real-mode ROM programs the 8237 for a read transfer (memory->device) + issues FDC WRITE DATA to write a patterned sector, then READ DATA it back and emits the result on serial. Golden fidelity (Fable D9): the written pattern MUST differ from the image's pre-existing bytes at that CHS, else a no-op write spoofs the test. Re-run the FreeDOS boot golden and confirm it is unchanged whether or not the boot path issues any write (Fable Q4). OUT OF SCOPE (Fable Q3): a DOS-level 'create a file' golden and host-file write-back for ImageSource::File — writes are volatile/in-memory only; both recorded as follow-up tasks. Note VERIFY ON's read-back verify pass already works (8237 TransferType::Verify). Fable-reviewed 2026-07-18.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 assembly::DriveConfig carries write-protect (default writable); assemble routes it to the Drive flag; a read-only mount rejects WRITE with NW end-to-end
- [ ] #2 Machine-level golden: guest writes a sector whose pattern DIFFERS from the original image bytes, reads it back byte-identical, emits over serial, IRQ6 observed; deterministic (no wall-clock)
- [ ] #3 FreeDOS boot golden re-run and unchanged; volatile-writes limitation documented in the uPD765 module header + deferred.md; DOS-level write golden and host write-back recorded as follow-up backlog tasks
<!-- AC:END -->
