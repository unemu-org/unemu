# AGENTS.md — unemu

Repository-specific instructions for coding agents. `CLAUDE.md` includes this
file and adds Claude-specific navigation guidance.

## Scope

`unemu` is the desktop and command-line application for unemu cores. This
workspace owns the front end, library and registry integration, media and
image management, recipes and host interface. The PC core lives in
[unemupc](https://github.com/unemu-org/unemupc); the NES core lives in
[unemunes](https://github.com/unemu-org/unemunes). Both are git dependencies
pinned to exact revisions in `Cargo.toml` and `Cargo.lock`.

The neutral session `machine-view` and synthetic `unemu-test-core` come from
[unemu-org/unemu-host](https://github.com/unemu-org/unemu-host), version 0.0.4,
pinned to `1692a1fcfd6c9900d3074786a54f51c9de3b30f0` in `Cargo.toml` and
`Cargo.lock`. Both use contract 0.5, pinned with `unemu-core-files` to
`316dd2a1f429cc3ef37a2bed203e89b2160079ff` (tag `v0.5.0`). They are not
workspace crates. Land their changes in unemu-host first, then re-pin this
workspace and run its gate, as for a core change.

CPU, devices, machine configuration and the core's tests belong in `unemupc`.
Its own rules apply there, including its x86jit dependency workflow. Land
core changes there before moving the front end's pin. Never copy core code
into this repository or commit bare path dependencies.

Use `bash scripts/core-patch.sh ../unemupc` for a gitignored local core
override and `--remove` to remove it. Use `scripts/core-checkout.sh` to
provision the pinned core's fixtures in its own devShell, and `--print-root`
to verify the fixture root.
The front end owns its FreeDOS 1.4 and recipe fixtures. Built files under
`vendor/` and `.core/` are never committed. Release notices name the front
end and both locked core revisions; release pins must name a matching core
release tag.

## Language and publication

- Talk to the maintainer in Polish. Everything committed is English: code,
  comments, documentation, identifiers, bead text, branches and commits.
- Write as if the repository were public. No proprietary firmware, guest
  software, manuals or screenshots; keep hashes and our own observations.
  Open fixtures and their licence texts may be committed. Real firmware and
  commercial titles may be named.
- Never name a private host, private path, personal setup or the CPU core's
  other consumers. `scripts/publication-guard.sh` checks the mechanical part.
- Push only to the configured `origin`. Creating or publishing a repository
  is the maintainer's step.
- Never commit without an explicit user request. Accepting a plan does not
  authorize a commit. Workers leave changes for the supervisor to review.
  `bd init` and the first `bd dolt push` can create commits themselves.

## Work and documentation

Open work is tracked in beads (`bd`, prefix **`ufe`**); the core keeps `upc`.
At session start read `bd memories`, then `bd list --status in_progress` and
`bd show <id>`. A sandboxed worker uses the supervisor's brief when the
tracker is unavailable. The supervisor claims a bead before code is written
and closes it only after it lands on `main`, naming the commit. Workers do
not claim or close beads. Append existing notes with `--append-notes`, never
`--notes`. Leave a mid-task handoff on the bead: done, next and blockers.
Cross-repository ids name their repository.

All project documentation lives under `backlog/`: knowledge in `backlog/docs/`,
decisions in `backlog/decisions/`. Open tasks stay in beads. Load only the
document relevant to the task:

- [Working on tasks](backlog/docs/working-on-tasks.md): lifecycle and git flow.
- [Conventions](backlog/docs/conventions.md): read before editing code.
- [Architecture](backlog/docs/architecture.md): structural changes.
- [Commands](backlog/docs/commands.md): builds and checks.
- [Dev setup](backlog/docs/dev-setup.md): tooling.
- [Deferred work](backlog/docs/deferred.md): before adding features.
- [Split design](backlog/docs/design/repo-split.md): repository boundaries.

Use the vendored `.agents/skills/backlog/SKILL.md` for docs and decisions,
never for open tasks. Prefer semantic navigation (Serena or repoctx) for
symbols, callers and dependencies.

## Engineering and checks

Keep changes within the task: no extra features, abstractions or cleanup and
no partial implementation of later milestones. Test changed application
behaviour at its boundary. Preserve the swappable core contract and
reproducible execution; host pacing must not introduce wall-clock inputs
into emulation. Keep disk bases immutable and SHA-256 identified, with
writes behind the existing overlay interface.

Run `scripts/affected-tests.sh --plan` before the affected-test runner.
The full checks are:

```sh
cargo build --locked --all-targets
cargo test --locked --workspace
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo clippy --locked -p window --bin unemu -- -D warnings
cargo fmt --check
```

Fixture and recipe lanes are in `scripts/ci-gate.sh`; use its verified core
checkout for fixture reads. Never launch GUI windows or take screenshots
during automated work. Follow a supervisor's toolchain and sandbox brief.

Maintainer and sole deciding authority: Mikołaj Mikołajczyk
(<mikolaj@mikolajczyk.org>).
