---
id: TASK-41
title: 'Floppy WRITE host-file write-back (ImageSource::File persistence)'
status: To Do
assignee: []
created_date: '2026-07-18 23:47'
labels:
  - devices
  - fdc
  - deferred
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
µPD765 WRITE DATA (task-39) commits sectors to the mounted image Vec IN MEMORY ONLY. A file-backed image (ImageSource::File) is read once at assemble time and never written back, so guest writes are volatile — a fresh run starts from the on-disk image again. Add a write-back policy so a guest's floppy writes persist to the host image file. Touches the image-load seam (assembly crate) + a flush/write-back path from the Drive. No current consumer, hence deferred. Recorded in backlog/docs/deferred.md by task-40.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A DriveConfig backed by ImageSource::File can opt into write-back; guest WRITE DATA persists to the host file
- [ ] #2 Default stays volatile (no accidental host-file mutation); write-back is explicit and tested
<!-- AC:END -->
