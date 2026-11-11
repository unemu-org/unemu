# Commands

Everyday commands for this project. Keep this file **flat and copy-pasteable** — agents and humans both grep it.

## Build / run / test

```sh
cargo build                                  # build the workspace
cargo run -p cli -- --boot                   # Real16 timer demo; no args print usage
cargo run --offline -p cli -- --bios vendor/kiselev-bios/bios-xi8088.bin
cargo run --offline -p cli -- --bios /path/to/bios.bin --allow-unlisted-firmware
cargo run --offline -p window --bin unemupc98-window -- --allow-unlisted-firmware /path/to/bios.bin /path/to/floppy.img
cargo test --workspace                       # unit + golden + integration
cargo test --release -p devices              # release-only device behaviour (CI runs it too)
cargo test --workspace -- --ignored          # slow goldens: BIOS POST, FreeDOS boot, DOS sessions (~2 min)
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --workspace -- --ignored # fail if a fixture is missing
# Corrected per-clock oracle versus production deadlines (no environment gate):
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test scheduler_ab -- --include-ignored --nocapture
# Opt-in host timing only, BIOS POST and FreeDOS prompt, median of three per engine:
UNEMUPC98_REQUIRE_FIXTURES=1 UNEMUPC98_SCHEDULER_WALL_TIMES=1 cargo test --offline --release -p cli --test scheduler_ab scheduler_engine_wall_times -- --ignored --nocapture
# DOS sessions and their final console screens (requires fetched fixtures):
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test dos_interactive_golden -- --ignored --nocapture
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test dos_write_golden -- --ignored --nocapture
cargo test -p devices                        # one crate's tests
cargo build --offline -p window --bin unemupc98-window
cargo run --offline -p window --bin unemupc98-window # default vendor BIOS + FreeDOS image
cargo run --offline -p window --bin unemupc98-window -- vendor/kiselev-bios/bios-xi8088.bin vendor/freedos/freedos.img
cargo tree --offline -i window               # window is a leaf frontend
```

System BIOS loads on IBM 5170 use a SHA-256 catalogue. Option ROMs are not
hashed or allowlisted yet; their checks belong to the option-ROM implementation. An unknown image is rejected
with its full hash and the known entries' fetch instructions. For development,
`--allow-unlisted-firmware` accepts unknown bytes and prints exactly one
`warning: running unlisted firmware (sha256 HASH)` line to stderr. Placement
(positive size divisible by 8 KiB, ending at `0x100000`) is still checked; listed
images still meet board, CPU and required-part constraints. This run-only flag
is never saved in machine files or used by goldens. The CLI `--bios PATH`
boots the complete board with a volatile blank 1.44 MB floppy and writes COM1
bytes to stdout. It prints one `stopped: ...` line to stderr and exits with
status 1 for unsupported instructions, execution/exception failures, or
configuration/policy/observation/persistence errors. Halt, reset-request and
budget-exhausted outcomes succeed: reaching the budget is a normal bounded
run's end. The window retains its existing BIOS/floppy arguments.
Catalogue boards must have a system BIOS ROM, start at their power-on reset
vector and keep all RAM and other images outside that ROM's range. The escape
flag does not relax these requirements; custom-entry Rust rigs use `Bare`.

The window binary requires a graphical desktop and fetched BIOS/FreeDOS fixtures.
It uses a fixed 2× pixel scale and keeps advancing through the FreeCOM date/time
prompts. Press Enter twice to accept them, then type a DOS command such as `dir`
and Enter. Physical key positions use the US layout: letters, digits,
punctuation, Enter/Backspace/Tab/Escape/Space, both Shift/Ctrl/Alt keys, locks,
F1–F12, the numeric keypad, arrows and navigation keys are supported. Pause/Break,
PrintScreen, system and media keys are ignored. Host repeat sends plain make
codes; fake-Shift wrappers are sent only on the first make and final break.
Guest typematic settings do not change the host repeat rate. On macOS, winit 0.30
does not deliver CapsLock key events, so Caps Lock does not reach the guest.
Focus loss releases all held keys, including modifiers. Keys are ignored after
the machine stops. NumLock wrappers follow the guest's keyboard LED state,
including guest reboots and Ctrl+NumLock (Pause), never the host OS lock state.

Input is queued until the next paced run boundary; a full 8042, a disabled
interface or a pending serial byte defers input without drops. The window uses
`Instant` only in its event loop, targets PIT time from elapsed real time and
paints at 60 Hz with `ControlFlow::WaitUntil` between frames. Each paint advances
at most 50 ms of guest time (plus scheduler instruction-boundary rounding).
Excess backlog is discarded permanently, so a slow host runs the guest slower
than real time without accumulating catch-up work. A resize can request an
extra paint but does not grant extra guest time. An occlusion notification pauses
the guest and switches to `Wait`, ignoring queued redraws. Resuming discards the
entire hidden interval, with no catch-up; suspension follows the same policy. Headless execution
and goldens keep their virtual-time budgets and never use this pacing.
See [`design/observation.md`](design/observation.md#window-pacing) for the exact
frequency, rounding, input replay contract and tests. No pacing flags are needed.
A natural halt, CPU failure or exhausted virtual-time range stops advancement
and retains the last screen until closed. Blink stays visible; its deterministic
phase can derive from virtual time later. Workspace and presenter tests remain
headless. The input log is test-owned, with no user-facing log flag.

Default fixture paths are resolved against the workspace location recorded at
build time, independently of the current directory; explicit paths remain
relative to the current directory. From outside the workspace, Cargo also needs
`--manifest-path /path/to/workspace/Cargo.toml`. Missing-file diagnostics name the
path and the relevant `vendor/kiselev-bios/fetch.sh` or `vendor/freedos/fetch.sh`.
`--help`, `--version` and `--about` include the embedded fonts' attribution.
On Linux the window requires the host's X11 or Wayland client libraries at run
time; a bare Nix devShell may not put them on the dynamic loader's search path.

The ignored set includes firmware/DOS goldens and corrected PerClock-vs-Deadline
comparisons, including final RAM/register/device state. The per-clock engine is
available through the non-default `reference-scheduler` feature enabled by CLI
dev-dependencies, never through `MachineConfig`. Legacy comparisons and
`UNEMUPC98_SCHEDULER_AB` are retired. Step budgets include REP iterations; the
command budget is calibrated to the final DOS file-write listing. No arguments
print usage to stdout with status 0; unknown or extra arguments print usage to
stderr with status 2. The informative
`scheduler_engine_wall_times` test skips with one line unless
`UNEMUPC98_SCHEDULER_WALL_TIMES=1` is set, including in ordinary slow/CI runs.

## Offline floppy media

```sh
cargo run -p cli -- media generations --base disk.img --journal disk.img.upfj
cargo run -p cli -- media verify --base disk.img --journal disk.img.upfj
cargo run -p cli -- media verify --base disk.img --journal disk.img.upfj --repair-tail --backup before-repair.upfj
cargo run -p cli -- media rollback --base disk.img --journal disk.img.upfj --to 1 --backup before-rollback.upfj
cargo run -p cli -- media commit --base disk.img --journal disk.img.upfj --to 1 --output restored.img
```

Inspections replay read-only under a shared stable journal lock. An existing
lock is opened read-only, so a 0444 lock file is supported. If the lock is absent
and its directory forbids creation (EACCES, EPERM or EROFS), inspection proceeds
without a lock and prints one line saying so. A writing session must already
have created that lock; an existing lock is never bypassed. Mutating tools still
require an exclusive lock with write access. A live journal session excludes all
tools. Never delete `<journal>.lock`: it is not a stale-PID file; deletion during
a session defeats the single-writer guarantee for both sessions and tools.
`generations` includes the base SHA-256, retained
anchor, closed ids/reasons/end offsets/counts and any unsealed tip. A closed
marker does not certify filesystem consistency or bootability.

`verify` reports binding/frame/grammar failures, recoverable-tail bytes and the
latest reconstructible state. Exit codes: **0** fully valid/closed, **1**
corruption, wrong base or I/O failure (including a missing journal and lock
conflicts, a dangling journal symlink or a
post-lock canonical-path change), **2** usage/config error, **3** recoverable tail
or unsealed commands. Reserved journal temporary names in any input/output,
including the base and resolved symlink targets, are configuration errors (exit 2).
Missing base/journal inputs are checked before locking,
leave no new files and report the input's role and path. Non-regular inputs,
including FIFOs, are refused without blocking. `--repair-tail` takes an exclusive
lock, synchronizes a full backup at an unused path, then performs normal mount
recovery/sealing. Interior damage and incomplete anchors are refused.

Rollback requires a retained closed generation; generation 0 is available only
before retention introduces a positive anchor. An unavailable id is exit 2.
After synchronizing the full backup and its directory, rollback truncates after
the chosen marker (128 bytes for 0) and synchronizes the journal. Later history
and any open tip are removed; their ids can be reused on subsequent mounts.
Keep the immutable base alongside backups. Existing backup paths (including
aliases of inputs) and reserved `.create-` filenames are refused; tools never
delete backups. On Unix a backup inherits the journal's exact permission bits
before any bytes are copied. A copy or backup-sync failure reports the retained
partial, invalid backup's path; do not use it as a valid recovery source. All I/O
errors identify the affected base, journal, lock or backup path. Tail diagnostics name
the safe truncation offset and discarded byte count separately from the failure
offset. Journal and image bytes stay unchanged on inspection.

`media commit` (alias `compact`) exports a retained closed generation, including
available generation 0, to a new raw image. It requires a shared lock and a valid
sealed tip; repair/seal interrupted history first. An occupied output, including
an input alias, is exit 2. The same-directory temporary is read back for size and
SHA-256 verification, synchronized, published by a no-clobber hard link, then its
directory is synchronized. Neither input changes. Select the new base and a fresh
journal path explicitly when continuing from an export; config is never edited.

Journal mode applies `keep_generations` after each successful checkpoint/session
close. The count includes the oldest retained point encoded as an anchor; keep=2
over ids 1–5 retains anchor 4 and id 5. Later ids/reasons survive; sequences restart
at 1. Retention verifies all retained states before file sync, atomic journal
rename under the stable lock, and directory sync. Maintenance failures
produce one warning per affected drive after every drive has been finished; they
do not turn a durable session close into a run failure. Pre-publication failures preserve the old
journal and permit retry at a later close; publication/sync uncertainty requires
reopening and verifying before further persistent work. Retention keeps exact
journal mode bits, but the replacement is owned by its creator. The next exclusive
mount removes this journal's regular decimal-named retention leftovers after safe
identity checks; it never follows a symlink or mounts a leftover. Reserved
`.retain-` and `.export-` names, like `.create-`,
cannot be mounted as journals or used for backups. Temporaries are never imported,
and backups are never pruned.

### Journal session walkthrough

Journal mode is available in Rust `MachineConfig` data and in TOML/YAML
machine-file floppy drive tables with `write = "journal"` and an explicit
`journal` path. The CLI and window frontends do not load machine files yet;
that integration is in progress, and their run arguments have no journal flag.
This runnable Rust example uses the small WRITE DATA preset: it programs DMA
channel 2, writes one sector, drains the controller result and halts. Run from
the repository root in the devShell. Generated media and the example project's
build output stay in a new temporary directory; `cargo build -p cli` uses the
repository's `target/` directory.

```sh
repo_dir="$PWD"
demo="$(mktemp -d)"
mkdir "$demo/src"
cat > "$demo/Cargo.toml" <<EOF
[package]
name = "journal-demo"
version = "0.1.0"
edition = "2021"

[dependencies]
assembly = { path = "$repo_dir/crates/assembly" }
EOF
cat > "$demo/src/main.rs" <<'RS'
use assembly::{assemble, at_fdc_write, ImageSource, RunPolicy, WriteMode};
use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(args.next().expect("media directory"));
    let value: u8 = args.next().expect("byte value").parse()?;
    let mut guest = vec![0xFA, 0xFC];
    for (port, byte) in [
        (0x3F2u16, 0x1C), (0x0A, 6), (0x0C, 0), (0x0B, 0x4A),
        (4, 0), (4, 6), (0x81, 0), (5, 0xFF), (5, 1), (0x0A, 2),
    ].into_iter().chain([5, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF].map(|b| (0x3F5, b))) {
        guest.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, byte, 0xEE]);
    }
    guest.extend([0xEC; 7]);
    guest.push(0xF4);
    let mut config = at_fdc_write(guest, vec![value; 512], 0x600);
    let base = directory.join("disk.img");
    if !base.exists() {
        std::fs::write(&base, vec![0; config.drives[0].kind.image_bytes() as usize])?;
    }
    config.drives[0].image = ImageSource::File(base);
    config.drives[0].write = WriteMode::Journal {
        path: directory.join("disk.upfj"), keep_generations: 10,
    };
    let mut machine = assemble(&config)?;
    println!("{:?}", machine.run_session(&RunPolicy::natural(Some(10_000), None))?);
    machine.finish_persistence()?;
    Ok(())
}
RS
cargo run --offline --manifest-path "$demo/Cargo.toml" -- "$demo" 17
cargo run --offline --manifest-path "$demo/Cargo.toml" -- "$demo" 34
cargo build --offline -p cli
(
  cd "$demo"
  "$repo_dir/target/debug/unemupc98" media generations --base disk.img --journal disk.upfj
  "$repo_dir/target/debug/unemupc98" media verify --base disk.img --journal disk.upfj
  "$repo_dir/target/debug/unemupc98" media rollback --base disk.img --journal disk.upfj --to 1 --backup before-rollback.upfj
  "$repo_dir/target/debug/unemupc98" media commit --base disk.img --journal disk.upfj --to 1 --output restored.img
  "$repo_dir/target/debug/unemupc98" media compact --base disk.img --journal disk.upfj --to 1 --output restored-alias.img
)
```

**MEASURED:** both guest runs print `Halted`. These excerpts were captured from
that generated-media run (all commands exited 0); `verify` prints the same
closed-generation report as `generations`:

```text
base sha256: 36bd753facc985aad613c884a2040210d208b1aa520e957b31ba2e1e19cd4185
retained anchor: 0
generation 0: immutable base, end 128, commands 0, sectors 0
generation 1: session end, end 2048, commands 1, sectors 1
generation 2: session end, end 3968, commands 1, sectors 1
recoverable-tail bytes: 0
latest reconstructible state: generation 2 (closed), end 3968
status: fully valid/closed
closed markers do not establish filesystem health or bootability
```

Rollback prints the warning and changes the report to generation 1:

```text
rollback to generation 1: later generations and any open tip will be removed
generation 1: session end, end 2048, commands 1, sectors 1
latest reconstructible state: generation 1 (closed), end 2048
status: fully valid/closed
```

The two export commands print:

```text
exported generation 1 to restored.img
exported generation 1 to restored-alias.img
```

The base stays all zeroes. Both exports contain byte 17 throughout sector 0 and
zeroes elsewhere; `before-rollback.upfj` retains the complete two-generation
journal. Keep the immutable base with that backup. To continue from the export,
select `restored.img` as the new base and choose a fresh journal path in Rust.
See [Supported host guarantees](design/persistent-media.md#supported-host-guarantees)
for the measured macOS/APFS behavior and storage assumptions.

## Typecheck / lint / format

```sh
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --locked -p cli --bin unemupc98 -- -D warnings # production, no reference-scheduler
cargo fmt --all
cargo fmt --all -- --check                   # CI check
cargo deny --all-features list               # inspect dependency licences
cargo deny --all-features check licenses bans sources
```

The `deny.toml` allowlist covers the all-features graph: AGPL-3.0-or-later for
workspace crates, MIT, Apache-2.0, Unicode-3.0, BSD-2-Clause, BSD-3-Clause
and ISC for dependencies. CC-BY-SA-4.0 is allowed only for the `window` crate
(a crate-scoped exception), whose embedded font assets carry it; see
`crates/window/assets/fonts/NOTICE.md`. New licences
require review before extending it. Unknown registries and git sources are denied;
only the pinned x86jit repository is explicitly allowed. Wildcard dependencies
are denied and duplicate versions warn. These checks do not need the advisory database.

## CI

Forgejo Actions runs `.forgejo/workflows/ci.yml` on pushes to `main` and manual
dispatches on the x86-64 runner. From the repository root, run the same gates locally:

```sh
bash scripts/ci-gate.sh devshell
bash scripts/ci-gate.sh fmt
bash scripts/ci-gate.sh guard
bash scripts/ci-gate.sh clippy
bash scripts/ci-gate.sh deny                     # licences, bans, sources; all features
bash scripts/ci-gate.sh tests
bash scripts/ci-gate.sh fixtures                 # build/hash-check BIOS, FreeDOS, test386; fetch missing inputs
bash scripts/ci-gate.sh goldens                  # slow tests (~2 min); requires fixtures
```

Fixture-dependent tests print a skip message when a fetched vendor file is missing.
Set `UNEMUPC98_REQUIRE_FIXTURES=1` to make a missing fixture fail with its path and
fetch script instead. The `goldens` gate always sets this variable, so a failed or
incomplete fixture fetch cannot silently pass CI. The µPD765 FAT12 test follows
the same policy when `mformat` is missing and points to `nix develop`.

All three CI fixture scripts check pinned output SHA-256 hashes and rebuild offline
when their pinned source commits and release archives are cached. FreeDOS also
fixes the volume serial (`2895-9853`), label (`FREEDOS`) and all filesystem
timestamps (1980-01-01 UTC). A hash mismatch requires checking the sources and
toolchain and deliberately rebuilding the goldens' expectations before changing
the expected hash. See each `vendor/*/README.md` for cache paths and hashes.

The XTIDE Universal BIOS AT option-ROM fixture is built separately from the CI
`fixtures` gate:

```sh
vendor/xtide-universal-bios/fetch.sh           # r638 AT binary, padded/checksummed to 8 KiB
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p assembly --lib xub
```

It checks both the download and output SHA-256, reusing
`vendor/xtide-universal-bios/build/ide_at.bin` for offline rebuilds. The output is
`vendor/xtide-universal-bios/ide_at-r638-8k.bin`. Its catalogue entry is `Listed`;
building it does not add an ATA device or hard-disk boot profile. See
`vendor/xtide-universal-bios/README.md` for provenance, padding and checksum rules.

The hard-disk data fixture builds without downloads or cached inputs:

```sh
vendor/hdd/build.sh                          # partitioned FAT16 data disk; pinned SHA-256
mdir -i vendor/hdd/fat16.img@@8704 ::          # UNEMUHDD, MARKER.TXT (29 bytes, 1980-01-01)
```

It uses mtools 4.0.49 with a fixed serial, label and FAT-epoch timestamps.
See `vendor/hdd/README.md` for the IBM AT type-2 geometry, partition/BPB layout,
hash and licence. This fixture is built separately from the CI `fixtures` gate.

Each gate uses `nix develop --no-update-lock-file --command` (except `guard`, which
runs directly), preserves its exit status, and tees combined output to the gitignored
`ci-logs/<step>.log`. Set `CI_LOG_DIR` to choose another log directory. Cargo builds/tests
use `--locked`; the clippy gate lints all features and then the production CLI
binary without dev-dependency feature unification; formatting uses `cargo fmt --check`. CI runs every gate even after failures,
saves the Cargo cache, and reports failing steps in commit status `ci / x86_64`.
See [`dev-setup.md`](dev-setup.md#ci) for the runner and cache setup.

## x86jit (CPU core)

```sh
# develop against a local x86jit checkout: a gitignored override, never a path dep in Cargo.toml
cat > .cargo/config.toml <<'TOML'
[patch."<the git URL x86jit-core is pinned to in Cargo.toml>"]
x86jit-core = { path = "<your x86jit checkout>/x86jit-core" }
TOML
git -C <your x86jit checkout> rev-parse HEAD    # must match the rev in Cargo.lock for goldens to be valid
```

A change the machine needs from the CPU lands in x86jit under its own beads and gate; this repo
then re-pins and re-runs the slow goldens. See `dev-setup.md` § CPU core.

## Guest test-ROMs

```sh
nasm -f bin boot.asm -o boot.bin             # flat binary for the guest
# (or nasm -f elf + ld + objcopy for larger programs)
```

## Pre-commit

```sh
pre-commit install                                  # one-time, per clone
pre-commit run --all-files                          # run active hooks
pre-commit run --all-files --hook-stage manual      # run staged-as-manual hooks too
```

## beads (`bd`) — open work

```sh
bd memories                                      # the shared working rules — read first
bd ready --exclude-type epic                     # unblocked work, highest priority first
bd list --status in_progress                     # what is claimed
bd show <id>                                     # one bead
bd list --type epic                              # the themes
bd create --title "<t>" --description "<d>" --acceptance "<ac>" --type task --priority P2 \
          --parent <epic-id> --no-inherit-labels --labels <labels>
bd update <id> --claim                           # before writing code
bd update <id> --append-notes "<progress>"       # never --notes on an existing bead
bd close <id> --reason "<commit> — <what landed>"
bd list --external-ref backlog:TASK-41           # the bead an old task id became
```

Details: [`working-on-tasks.md`](working-on-tasks.md).

## Backlog.md — docs and decisions only

```sh
backlog doc create "<title>"                      # → docs/doc-N - Title.md
backlog decision create "<title>"                 # → decisions/decision-N - Title.md
```

Never `backlog task create`: tasks moved to beads.
