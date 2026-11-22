# Working on tasks

Open work is tracked in **beads** (`bd`, a Dolt-backed issue tracker; issue prefix `ufe`). Knowledge
— design, decisions, guides — stays as markdown under `backlog/`; a bead links to a doc section and
never copies it. Roles, the path from a bead to `main`, review and the worker rules are beads
memories (`bd memories`, keys `roles`, `bead-to-main`, `bead-conventions`, `worker-prompts`,
`sandbox-workers`, `publication-rule`; the publication rule is also in
[`conventions.md` § Publication](conventions.md#publication)); this page is the day-to-day reference. `bd <cmd> --help`
gives the flags. `bd prime` prints the tool's generic workflow; where it differs from these docs and
the beads memories (who claims and closes, what goes into `bd remember` — see
`bd recall bead-conventions` — pushing), these docs and the memories win.

The core (`unemupc`, prefix `upc`) and CPU (`x86jit`, prefix `xj`) keep
separate databases. Cross-repository ids carry the repository name, for example
`upc-sk3.16 (unemupc)`. A front-end bead waiting for a core change records that
wait in prose; dependencies cannot cross databases.

## Statuses

| Status | Meaning |
|--------|---------|
| `open` | Filed, scoped, not started. Default for every new bead. |
| `in_progress` | Claimed and actively worked. Claim it **before** you start writing code. |
| `closed` | Landed on `main`. The close reason names the commit and the doc section changed. |

`bd` derives "blocked" from dependencies: a bead with an open blocker does not show in `bd ready`.

```sh
bd ready --exclude-type epic               # unblocked work, highest priority first (plain `bd ready` lists the epics too)
bd list --status in_progress               # what is claimed right now
bd show <id>                               # one bead with its dependencies and parent
bd list --parent <epic-id>                 # the children of one epic
bd list --external-ref backlog:TASK-41     # the bead an old Backlog.md id became
```

Priorities are `P0`…`P4` (`P0` highest). The import mapped Backlog.md high/medium/low to
`P1`/`P2`/`P3`; unset became `P2`.

## Epics

Each theme is an epic (`bd list --type epic`); every task bead belongs to one, through `--parent`
(housekeeping chores may stand alone). The epic carries the theme's label; children keep their own
labels (create them with `--no-inherit-labels`). A dotted id (`ufe-12.1`) shows the parent a bead
was created under; after a reparent `bd show` is authoritative.

| Epic key | Theme |
|----------|-------|
| `ux` | application, library, media and settings UI; GUI work needs approved mockups |
| `tests-ci` | front-end test infrastructure and gates |
| release | front-end packaging, release pins and distribution |

Hardware, CPU, scheduler and machine-validation epics stay in unemupc. Open
front-end work is re-created with `external_ref unemupc:<old id>`; closed core
beads remain history there. Do not invent new ids for migrated work in docs.

A new theme gets a new epic; do not stretch an existing one to fit.

## Bead lifecycle

```sh
bd create --title "<title>" --description "<why and what>" --acceptance "<criteria>" \
          --type task --priority P2 --parent <epic-id> --no-inherit-labels --labels <theme labels>
bd update <id> --claim                              # before writing code
bd update <id> --append-notes "MEASURED: <command> → <result>"
bd close <id> --reason "<commit> — <what landed; doc section changed>"   # only once it is on main
```

- **Never `--notes` on an existing bead**: it replaces the whole notes field. Use `--append-notes`.
- Label evidence: **MEASURED** (command run, give it), **READ** (`file:line`), **ASSUMED** (not
  checked yet).
- Work found along the way is a new bead with `--deps discovered-from:<id>`, not folded into the
  current one.
- Do not close until `main` actually contains the work — an early close misleads `bd ready`.

## Definition of Done

A bead closes only when all of these hold on `main`:

1. `cargo test --workspace` is green.
2. `cargo clippy --all-targets --all-features -- -D warnings` is clean.
3. `cargo fmt --check` is clean.

The CI lane runs these checks via `scripts/ci-gate.sh`, plus the publication
guard, `cargo deny --all-features check licenses bans sources` on every run,
and the slow goldens after building their fixtures. Use
`bash scripts/ci-gate.sh <step>` to run the same gate locally; see
[`commands.md` § CI](commands.md#ci). Before merging one change,
`bash scripts/bead-gate.sh` runs the same kinds of checks on only what the change
can affect, with the slow goldens when it touches emulation; see
[`commands.md` § Affected tests and the bead gate](commands.md#affected-tests-and-the-bead-gate).

Plus, when the change warrants it:

- `cargo deny --all-features check licenses bans sources` whenever the dependency
  set changes (including features, sources, and licence policy), following x86jit's
  conditional Definition of Done;
- the slow front-end goldens when a core pin or fixture changes, with the
  verified fixture root and `UNEMUPC_REQUIRE_FIXTURES=1`;
- an A/B equality test when a refactor claims to change nothing observable;
- a mutation check on a new golden: break the feature, watch the golden fail.

## Git flow

One bead, one branch (`<type>/<bead-or-topic>`, [Conventional Branch](https://conventionalbranch.org/)),
one worktree outside the main checkout. The supervisor squash-merges each bead into `main` as one
commit: **Conventional Commits** (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`, `test:`, `ci:`,
`build:`), **GPG-signed**. Commits and pushes happen only at the maintainer's request. Never bypass
hooks. Details: `bd recall bead-to-main`.

The configured `origin` is the only push destination. The issue database lives in `.beads/`
(gitignored); `bd dolt push` stores it in `refs/dolt/data` on `origin` and it goes nowhere else.
Push it after a batch of bead changes, with the maintainer's agreement like any push. Until a remote
exists the database lives on one machine only — back it up with `bd backup` if it matters.

## Decision capture

- Bead-bound decision → on the bead: `bd update <id> --append-notes "Decided: X over Y — <reason>."`
- Cross-cutting / architecture-grade → `backlog decision create "<summary>"` (decisions stay
  markdown under `backlog/decisions/`). See [`decisions.md`](decisions.md).

## Session handoff

```sh
bd update <id> --append-notes "Session pause $(date -I). Done: <X>. Next: <Y>. Blocker: <Z|none>."
```

The next session runs `bd list --status in_progress`, then `bd show <id>`, and picks up without
rediscovering state from the diff.

## History

unemu maintains its front-end beads (`ufe`); unemupc maintains the pre-split
tracker and closed beads (`upc`). Backlog.md was the tracker until 2026-11-09.
Its [Done history](https://github.com/unemu-org/unemupc/tree/main/backlog/completed)
and [migration archive](https://github.com/unemu-org/unemupc/tree/main/backlog/archive)
live only in the core. Search old ids in the core's database with
`bd list --external-ref backlog:TASK-NN`. Open front-end beads carry
`external_ref unemupc:<old id>` after migration. Shared git history preserves
all pre-split commit references.
