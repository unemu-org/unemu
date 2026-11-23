# Coding conventions

unemu maintains front-end conventions; unemupc maintains hardware and execution conventions in its own repository.

Generic conventions that apply regardless of stack. Stack-specific rules (language idioms, framework patterns, formatter config) go in the **Stack-specific** section at the bottom — filled at bootstrap.

## File naming

- Pick **one** casing per category and stick with it (e.g. PascalCase for components, kebab-case for scripts, snake_case for modules). Document the choice in Stack-specific.
- One unit per file (one component, one class, one primary export). Co-locate tightly related sibling files (CSS module next to component, test next to source).

## Imports

- Cross-folder imports go through a folder's barrel / public entry, not into its internals. The barrel is the contract; internals are not.
- Prefer path aliases (`@core`, `@services`, ...) over deep relative paths once the project grows past ~3 directory levels.

## Comments

- **Default: no comment.** Names do the work.
- Add only when the *why* is non-obvious: hidden constraint, subtle invariant, workaround for a specific bug, surprising behavior.
- Never explain *what* the code does — well-named identifiers already do that.
- Don't reference the current task / fix / PR ("added for X", "handles case from #123") — that belongs in the commit message, not the source file.
- **Comments must read for a human who has never seen our backlog.** No milestone tags, tracker ids or decision numbers in source comments. Explain the constraint in plain language. Public modules and traits document their ownership and contracts; hardware facts cite the datasheet by name and section or mode. Source comments contain no URLs.

## Commits

- Conventional Commits by default (`feat:`, `fix:`, `chore:`, `docs:`, `refactor:`, `test:`, `release:`). If your project uses a different convention, document in Stack-specific.
- GPG-signed. The `gpg-uid-guard` pre-commit hook refuses to sign if `user.email` has no matching UID on `user.signingkey`.
- **Never commit without explicit user request.** This rule supersedes any plan acceptance.

## Phase / scope discipline

- Don't pre-empt later milestones. If a task is scoped to a later milestone, don't half-implement it during earlier work.
- If a refactor would be cleaner alongside a bug fix but isn't required, defer it — create a backlog task instead.
- Don't add error handling, fallbacks, or validation for scenarios that can't happen at the call site. Trust internal code; validate only at system boundaries (user input, external APIs).

## UI / output text (if applicable)

- Terse. Lowercase. No emoji in UI text or logs unless the project explicitly opts in.

## Publication

The repository is public. Everything committed (code, comments, docs, bead
text, commit messages) follows the publication rule in `AGENTS.md`:

- **No proprietary bytes.** Proprietary firmware, guest software, manuals,
  scans and screenshots of them stay out of the tree; tests and docs keep
  hashes and our own observations. Open firmware, open fixtures and their
  licence texts (the FreeDOS licence index, say) may be committed.
- **Names are fine.** Real firmware (an IBM BIOS, say) and commercial titles
  may be named in code, docs and tests.
- **Nothing private.** No private hosts, private paths or anyone's personal
  setup, and the CPU core's other consumers stay unnamed. Examples use
  `example.org`, `example.com` or `.example` hosts.

`scripts/publication-guard.sh` checks the mechanical part on every commit and
in CI; see [`commands.md` § CI](commands.md#ci).

## When in doubt

- Read the relevant decision: `ls backlog/decisions/` (see [`decisions.md`](decisions.md)).
- Check active work: `bd ready` (prefix `ufe`).
- Ask the user. Solo project — they're the only deciding authority.

---

## Stack-specific (Rust)

- **Edition**: 2021. **MSRV**: latest stable Rust from the devShell (currently
  1.99), compatible with both pinned cores and the contract. The workspace is
  the 20 front-end crates listed in [architecture](architecture.md).
- **Clippy**: `cargo clippy --locked --all-targets --all-features -- -D warnings`.
  **Format**: `cargo fmt --check`, default rustfmt.
- **Error handling**: `thiserror` for stable library error variants; `anyhow`
  only in front-end command plumbing and tests. Never `unwrap`/`expect` in
  library code outside tests.
- **Naming**: name host responsibilities and platform providers; hardware
  implementations belong in the core repository.
- **Language**: all code, comments, and identifiers in **English** (Polish only in
  chat with the maintainer — see AGENTS.md).
- **License headers**: each source file carries an SPDX line
  `// SPDX-License-Identifier: AGPL-3.0-or-later`.
- **Doc-comments** explain public modules and traits, their ownership and
  relevant front-end contracts. Hardware facts belong in the core's docs.
- **Dependencies**: cores, `unemu-core-api`, and the `machine-view` and
  `unemu-test-core` libraries from [unemu-org/unemu-host](https://github.com/unemu-org/unemu-host)
  are git dependencies pinned to exact revisions. The host libraries are not
  workspace members; land their changes upstream first, then re-pin here.
  Local work uses gitignored patches, never committed bare
  path dependencies; contract and core pins move together when needed.
- **Determinism**: host timing may pace execution but never becomes an input to
  emulation. Virtual time is measured by the core; disk bases remain immutable
  and SHA-256 identified, with writes behind the overlay interface.

## Test strategy

Test application behaviour at its boundary: headless UI models, synthetic
store and registry content, recipes, and sessions over the pinned cores. Keep
mockup inventories and recipe corpus/schema tests intact. Read PC fixtures
through `UNEMUPC_FIXTURE_ROOT`; hardware tests are maintained by
[unemupc](https://github.com/unemu-org/unemupc/blob/main/backlog/docs/testing.md).
Run the [affected-test plan](commands.md#affected-tests-and-the-bead-gate)
before its runner; a core re-pin requires the front-end goldens.

Keep process-spawning tests in separate test binaries from tests holding journal
locks or pipe/FIFO descriptors whose lifetime affects another test. On Unix,
fork duplicates every thread's open descriptors: `O_CLOEXEC` closes them only at
exec, so a child can briefly retain a sibling's released lock or postpone EOF.
Separate binaries prevent that inheritance without serializing the fast suite.
