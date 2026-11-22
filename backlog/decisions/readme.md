# Decisions

Architecture and technology decisions that guide development — not tasks.

```sh
backlog decision create "<title>"   # → decision-N - Title.md
ls backlog/decisions/                # list existing (no `decision list` subcommand)
```

See [`../docs/decisions.md`](../docs/decisions.md) for **when** a choice is weighty enough to
record here versus leaving it as a task note.

## Split ownership

unemu maintains decisions 24–27, 29, 33–39 and 43. Shared historical records
11, 20, 30, 32, 40 and 41 remain here; unemupc maintains the hardware and
execution portions, while unemu maintains host output and application portions.
The other records (1–10, 12–19, 21–23, 28 and 42) live in
[unemupc](https://github.com/unemu-org/unemupc/tree/main/backlog/decisions).
Pre-split `upc-*` ids in these records belong to unemupc; their historical wording is retained.
Decision 31 is absent from the tree and its path history. Keep that gap and
continue the local series without renumbering existing records.
