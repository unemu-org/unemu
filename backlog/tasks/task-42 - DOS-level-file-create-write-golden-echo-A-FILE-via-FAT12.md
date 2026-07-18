---
id: TASK-42
title: 'DOS-level file-create write golden (echo > A:\FILE via FAT12)'
status: To Do
assignee: []
created_date: '2026-07-18 23:47'
labels:
  - test
  - fdc
  - deferred
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The floppy WRITE path is proven at the controller level (task-40 real-machine write golden) + unit tests. This task drives FULL DOS to create a file (e.g. 'echo hi > A:\FOO.TXT' over the keyboard) and verifies it end to end through the FreeDOS FAT12 driver (TYPE/DIR shows it, or the raw sectors change). Much larger end-to-end path (boot + FAT12 driver + keyboard) with no incremental controller coverage until DOS write flows are the target. Deferred; recorded in backlog/docs/deferred.md by task-40. Likely #[ignore] (slow boot).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A FreeDOS golden creates a file from the keyboard and observes it back (DIR/TYPE or sector diff), deterministic
- [ ] #2 Exercises the FAT12 write path through the real DOS + uPD765 WRITE DATA end to end
<!-- AC:END -->
