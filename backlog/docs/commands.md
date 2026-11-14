# Commands

Everyday commands for this project. Keep this file **flat and copy-pasteable** — agents and humans both grep it.

## Local machine images

```sh
cargo run --offline -p cli -- image import machines/at-freedos.toml --store /path/to/store --tag dos-lab:1.0 --create
cargo run --offline -p cli -- image import /path/to/machine.yaml --store /path/to/store --tag dos-lab:recovered --recover medium-0=tip
cargo run --offline -p cli -- image import /path/to/machine.toml --store /path/to/store --tag dos-lab:checkpoint --recover medium-0=generation:1
cargo run --offline -p cli -- image ls --store /path/to/store
```

Import is opt-in and creates a new local tag using compare-and-swap against
absence. Use another tag when the name exists. It resolves inheritance, verifies
firmware catalogue identities and full media SHA-256, and copies all required
bytes into the store. No input is rewritten. Interrupted journal tails require
`--recover MEDIUM=tip` (the command-complete recovered tip) or
`--recover MEDIUM=generation:N` (a retained closed generation, including zero only
when retained). Repeat the flag for independent choices; unnamed clean journals
use their tip, and an unnamed interrupted journal reports its medium id and
recoverable generations. `cmos` names NVRAM; disk/catalogue ids are `medium-0`,
`medium-1`, etc., in effective traversal order. The local record under
`local/imports/<image-digest>.toml` and CLI output record policy and generation
separately; readonly/volatile media have no generation.
Interior corruption blocks import. Frozen disks, floppies and CMOS are whole raw
blobs. `--store` is required. Import uses an existing store unless `--create`
explicitly allows initialization; listing never creates a store. Repeat
`--rom-dir DIR` to locate pinned missing firmware through the same discovery path
as machine-file runs. Tags are checked before ingest and again by CAS at publication.
No platform default, layers or registry is selected.

## Build / run / test

```sh
cargo build                                  # build the workspace
cargo run -p cli -- --boot                   # Real16 timer demo; no args print usage
cargo run --offline -p cli -- --bios vendor/kiselev-bios/bios-xi8088.bin
cargo run --offline -p cli -- --bios /path/to/bios.bin --allow-unlisted-firmware
cargo run --offline -p cli -- machine check machines/at-freedos.toml
cargo run --offline -p cli -- run --machine machines/at-freedos.toml --max-steps 12000000 --idle-limit 1000000 --print mda
cargo run --offline -p window --bin unemupc98-window -- --machine machines/at-freedos.toml --display mda
cargo test --workspace                       # unit + golden + integration
cargo test --release -p devices              # release-only device behaviour (CI runs it too)
cargo test --workspace -- --ignored          # slow goldens: BIOS POST, FreeDOS boot, DOS sessions
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --workspace -- --ignored # fail if a fixture is missing
# Corrected per-clock oracle versus production deadlines (no environment gate):
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test scheduler_ab -- --include-ignored --nocapture
# Opt-in host timing only, BIOS POST and FreeDOS prompt, median of three per engine:
UNEMUPC98_REQUIRE_FIXTURES=1 UNEMUPC98_SCHEDULER_WALL_TIMES=1 cargo test --offline --release -p cli --test scheduler_ab scheduler_engine_wall_times -- --ignored --nocapture
# DOS sessions and their final console screens (requires fetched fixtures):
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test dos_interactive_golden -- --ignored --nocapture
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test dos_write_golden -- --ignored --nocapture
# FreeDOS boots A: with the AT-reset BIOS and XT+ ROM, then lists readonly C:
UNEMUPC98_REQUIRE_FIXTURES=1 cargo test --offline -p cli --test ide_freedos_golden -- --ignored --nocapture
cargo test -p devices                        # one crate's tests
cargo build --offline -p window --bin unemupc98-window
cargo run --offline -p window --bin unemupc98-window # default machines/at-freedos.toml
cargo run --offline -p window --bin unemupc98-window -- --machine machines/local.toml --allow-unlisted-firmware
cargo tree --offline -i window               # window is a leaf frontend
```

The default `test` profile inherits the workspace's `dev` settings, so ordinary
tests and ignored goldens share compiled artifacts. Use the same profile for
both; `cargo test --profile test --workspace -- --ignored` makes the choice
explicit, as the CI gate does. Keep the separate release device run to cover
behaviour that intentionally differs when debug assertions are disabled.

Dependencies, including the pinned CPU core, use optimization level 3. The hot
workspace crates (`devices`, `machine`, `bus`, `cpu`, `assembly`, `audio`) also use
level 3. Debug information, debug assertions and overflow checks remain enabled;
frontends and the other workspace crates keep their unoptimized dev settings.
Optimized code can be harder to step through even with debug information. For a
focused device investigation, temporarily override just that crate:

```sh
cargo --config 'profile.dev.package.devices.opt-level=0' test -p devices
```

To investigate an intermittent fast-suite failure, run two independent targets
in parallel from the devShell. This captures every run's complete output and
exit code, including failures that pass on an immediate rerun. Additional host
load can run alongside this loop; keep the same load for before/after comparisons.

```sh
scratch="$(mktemp -d)"
for lane in a b; do
  (
    for iteration in $(seq 1 20); do
      if CARGO_TARGET_DIR="$scratch/target-$lane" cargo test --offline --workspace --no-fail-fast >"$scratch/$lane-$iteration.log" 2>&1; then
        echo 0 >"$scratch/$lane-$iteration.exit"
      else
        echo "$?" >"$scratch/$lane-$iteration.exit"
      fi
    done
  ) &
done
wait
rg -n 'FAILED|panicked' "$scratch" --glob '*.log'
rg -n '^[1-9][0-9]*$' "$scratch" --glob '*.exit'
```

System BIOS loads on IBM 5170 use a SHA-256 catalogue. Option ROMs also use catalogue hashes and format checks. An unknown image is rejected
with its full hash and the known entries' fetch instructions. For development,
`--allow-unlisted-firmware` accepts unknown bytes and prints exactly one
`warning: running unlisted firmware (sha256 HASH)` line to stderr. Placement
(positive size divisible by 8 KiB, ending at `0x100000`) is still checked; listed
images still meet board, CPU and required-part constraints. The CLI `--bios`
automatically top-aligns any positive image whose size is a multiple of 8 KiB
and at most 1 MiB: base = `0x100000 - size` (16 KiB gives `0xFC000`). Other
sizes are rejected. An explicit machine-file base is checked as supplied.
This run-only flag
is never saved in machine files or used by goldens. The CLI `--bios PATH`
boots the complete board with a volatile blank 1.44 MB floppy and writes COM1
bytes to stdout. It prints one `stopped: ...` line to stderr and exits with
status 1 for unsupported instructions, execution/exception failures, or
configuration/policy/observation/persistence errors. Halt, reset-request and
budget-exhausted outcomes succeed: reaching the budget is a normal bounded
run's end.
Catalogue boards must have a system BIOS ROM, start at their power-on reset
vector and keep all RAM and other images outside that ROM's range. The escape
flag does not relax these requirements; custom-entry Rust rigs use `Bare`.

`machine check FILE` parses, merges, lowers, validates and assembles to run
image-byte catalogue checks, without executing guest instructions. It exits
0 for valid input, 1 for load/validation/assembly errors and 2 for usage errors;
diagnostics retain the defining file and line, including inherited firmware.
Checks never create or repair journals or locks. Missing journals are valid with
"journal will be created"; recoverable tails/unsealed tips are valid with a
warning that recovery runs on the next mount. Interior damage and wrong base
bindings are invalid at the drive's file:line. A busy journal is valid with a
warning that it was not inspected. Existing locks use offline shared inspection;
a missing lock uses the same read-only scanner without a lock and reports it.
Journal drives assemble as volatile for the remaining catalogue checks.
`run --machine FILE` uses `RunPolicy::natural`: `--max-steps` defaults to
20,000,000 completed-step equivalents and `--idle-limit` to 1,000,000 halted PIT clocks.
Both accept unsigned decimal integers (zero is allowed). It uses the same stop
messages and exit codes as `--bios`, with usage errors exiting 2. `--print CARD`
selects a display text dump or UART TX bytes on stdout. Without it, the first
display is chosen, then the first UART, or no output if neither is configured.
UART capture is bounded to 1 MiB; overflow is an error. Normal halt and budget
exhaustion finish journal persistence and print maintenance warnings on stderr.
Execution errors leave unsealed commands for recovery on the next mount.
A configured card without printable output is an argument error (exit 2). Both file commands accept
`--allow-unlisted-firmware` for development. Existing `--boot`, `--bios` and
`media` commands retain their contracts. The FreeDOS prompt, DIR and file-write
goldens load `machines/at-freedos.toml`; BIOS goldens and synthetic rigs retain
Rust presets.

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

View > Speed selects 100% (the default on every Start), 200%, 400% or Unlimited.
Hold Ctrl+Alt+T for Unlimited; releasing T restores the selected mode,
even if Ctrl or Alt was released first. Focus loss or opening a menu/dialog also
ends the hold. The chord's T make, repeat and break never reach the guest;
previously forwarded modifiers receive balanced breaks. The status bar reports
the mode and measured virtual-time/host-time ratio over the last second.
This host pacing control is separate from the machine's configured instruction rate.

Input is queued until the next run boundary; a full 8042, a disabled
interface or a pending serial byte defers input without drops. Only the window
frontend reads `Instant`; it targets virtual time from elapsed host time scaled
by the selected mode. Guest time remains retired instructions plus halted PIT
clocks. The host budget follows the observed presentation period, clamped between
one 60 Hz period (about 16.7 ms) and 50 ms. Paced modes run until their virtual
target is reached or that full period is used up. Unlimited has no virtual-time
target and leaves 3 ms of the period for presentation (about 13.7 ms of execution
at 60 Hz). Budget checks occur between chunks of at most 1 ms of virtual time,
plus scheduler instruction-boundary rounding. A 100% frame needing 8 ms of host
work can finish in one presentation. Slower repaint cadences allow larger slices
within the 50 ms bound. Rendering and other host costs still
contribute to the measured achievable speed. Display
capture and texture upload run at most once on each 60 Hz presentation deadline;
extra UI repaints grant no guest work. Paced modes discard backlog above 50 ms
times the selected ratio, so a slow host accumulates no catch-up debt. Mode
changes, pause/resume and uncovering re-anchor pacing at the current guest clock.
Hidden, suspended, paused and modal time grants no catch-up work.
Host audio is muted outside 100%, including during the hold, without time-stretch.
Returning to 100% clears queued samples and converter history and prefills fresh
audio; explicit user mute remains in effect. Status distinguishes `Audio muted
(speed)` from user mute. During speed mute, the View menu shows whether
user mute is on or off; Ctrl+Alt+M toggles that preference for the return to 100%
and does not override the speed mute. Playback never drives virtual time.
Headless CLI execution and goldens retain their virtual-time budgets and never
use host pacing. See [`design/observation.md`](design/observation.md#window-pacing)
for board-clock conversion and the input replay contract.
A natural halt, CPU failure or exhausted virtual-time range stops advancement
and retains the last screen until closed. Its speed cell keeps the selected mode
without a stale ratio; Stop returns the cell to `Speed 100%`. Blink stays visible; its deterministic
phase can derive from virtual time later. Workspace and presenter tests remain
headless. The input log is test-owned, with no user-facing log flag.

The window loads `machines/at-freedos.toml` from the workspace recorded by
`CARGO_MANIFEST_DIR` at build time, independently of the current directory.
`--machine FILE` is relative to the current directory; image and inheritance
paths inside it are relative to their defining file. A missing default file is
an error naming that path. Running without the build workspace needs an explicit `--machine`.
`--display CARD` chooses a display by id; the default is the first display in
file order (MDA in the shipped machine). Positional BIOS/floppy paths are removed;
use an extending file as shown in `machines/README.md`. From outside the
workspace, Cargo also needs `--manifest-path /path/to/workspace/Cargo.toml`.
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

## Offline persistent media

```sh
cargo run -p cli -- media generations --base disk.img --journal disk.img.upfj
cargo run -p cli -- media verify --base disk.img --journal disk.img.upfj
cargo run -p cli -- media verify --base disk.img --journal disk.img.upfj --repair-tail --backup before-repair.upfj
cargo run -p cli -- media rollback --base disk.img --journal disk.img.upfj --to 1 --backup before-rollback.upfj
cargo run -p cli -- media commit --base disk.img --journal disk.img.upfj --to 1 --output restored.img
```

The same `media generations`, `verify`, `verify --repair-tail`, `rollback`,
`commit` and `compact` commands accept CMOS NVRAM journals. Pass the original
64-byte `cmos.bin` with `--base` and its separate journal with `--journal`.
Exports remain 64 bytes, preserving the original clock/control prefix and the
selected generation's general RAM (0Eh–3Fh except the base-supplied shutdown
byte 0Fh). See `design/persistent-media.md`
§ CMOS NVRAM journals.

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
cargo deny --all-features check advisories licenses bans sources
```

The `deny.toml` allowlist covers the all-features graph: AGPL-3.0-or-later for
workspace crates, MIT, Apache-2.0, Unicode-3.0, BSD-2-Clause, BSD-3-Clause
and ISC for dependencies. CC-BY-SA-4.0 is allowed only for the `window` crate
(a crate-scoped exception), whose embedded font assets carry it; see
`crates/window/assets/fonts/NOTICE.md`. New licences
require review before extending it. Unknown registries and git sources are denied;
only the pinned x86jit repository is explicitly allowed. Wildcard dependencies
are denied and duplicate versions warn.

The advisory check uses the RustSec database and covers the full dependency graph.
Vulnerabilities and yanked crates fail the gate. An unmaintained crate requires
an advisory-specific ignore entry in `deny.toml` with a dated reason and a review
date; review each entry on that date and remove or renew it with an updated reason.
Review dates are a maintainer policy, not an automatic cargo-deny expiry.
The current exception is RUSTSEC-2026-0192 (`ttf-parser` 0.25.1, unmaintained,
not a known vulnerability): winit's default Wayland client-side decorations keep
window frames on GNOME, and no safe upgrade is available. Review it on 2027-05-11.
CI fetches the advisory database. With a cached database and no network, run:

```sh
cargo deny --all-features check --disable-fetch advisories licenses bans sources
```

## CI

Forgejo Actions runs `.forgejo/workflows/ci.yml` on pushes to `main` and manual
dispatches on the x86-64 runner. From the repository root, run the same gates locally:

```sh
bash scripts/ci-gate.sh devshell
bash scripts/ci-gate.sh fmt
bash scripts/ci-gate.sh guard
bash scripts/ci-gate.sh clippy
bash scripts/ci-gate.sh deny                     # advisories, licences, bans, sources; all features
bash scripts/ci-gate.sh tests
bash scripts/ci-gate.sh fixtures                 # build/hash-check BIOS, FreeDOS, test386; fetch missing inputs
bash scripts/ci-gate.sh goldens                  # slow tests in the test profile; requires fixtures
```

Fixture-dependent tests print a skip message when a fetched vendor file is missing.
Set `UNEMUPC98_REQUIRE_FIXTURES=1` to make a missing fixture fail with its path and
fetch script instead. The `goldens` gate always sets this variable, so a failed or
incomplete fixture fetch cannot silently pass CI. The µPD765 FAT12 test follows
the same policy when `mformat` is missing and points to `nix develop`.

### User-supplied ROM tests

This local opt-in lane is never called by CI; CI never has proprietary firmware:

```sh
bash scripts/ci-gate.sh user-roms
# Equivalent command inside the devShell:
cargo test --locked --profile test --workspace --no-fail-fast -- --ignored user_rom_
```

These tests are `#[ignore]` with names starting with `user_rom_`. The shared
`assembly::require_user_firmware!(entry_id, board.firmware_catalogue())` helper
is available through assembly's `test-support` feature. It searches recursively
in `vendor/user-roms/` first, then the optional directory
`UNEMUPC98_USER_ROMS_DIR`. Set it to an absolute path to a folder of unpacked,
plain dump files, rather than a volume root or home directory. Archives such as
MAME `.zip` sets are not unpacked. Names and subdirectories do not matter;
catalogue SHA-256 identities select assembled
images or complete chip layouts. Only regular files of matching sizes are hashed,
once per canonical path per test process; keep dumps unchanged during a run.
Root, file and directory symlinks are followed; visited canonical directories
prevent loops. Earlier roots win, and parts can span both roots.
A set-but-missing, relative or non-directory external root fails configuration,
even if vendor firmware matches. Unreadable subdirectories fail scanning.
Private root names are redacted in discovery errors and display names. Use the
returned firmware's `redact`, `expect_redacted` and `expect_err_redacted` helpers
for downstream diagnostics that may embed paths.

Missing, incomplete or wrong layouts print exactly one visible line:

```text
SKIPPED: user-supplied firmware <entry id> not found (set UNEMUPC98_USER_ROMS_DIR or see vendor/user-roms/README.md)
```

If some parts match, the same line appends counts for each partial layout, such
as `; layout 0: 1 of 2 parts found`, without hashes or paths.

`UNEMUPC98_REQUIRE_USER_ROMS=1` makes absence panic with that same text;
other values leave skipping enabled. `UNEMUPC98_REQUIRE_FIXTURES` does not change
this policy. Discovery I/O errors fail with redacted diagnostics instead of skipping.
Require/skip panics point at the calling test.
The ignored `user_rom_ibm_1985_identity` checks assembled identity, per-chip hashes
and misplaced/bad-chip diagnostics through assembly and machine-file lowering,
without executing the BIOS. It accepts either the chip pair or an assembled image.
`user_rom_landmark_at_identity` similarly checks the Landmark AT assembled hash,
mirrored halves and per-chip hashes. The CLI test
`user_rom_landmark_at_real_mode_prefix` runs its first twelve diagnostics headlessly
through the same preset loader, assembler and session policy as `run --machine`,
with no disk and the intended SIDT stop. Adapter-gap reads now use board-owned
read-only zero mappings; its retired count and MDA plane hash retain
TODO-MEASURE sentinels until a new local run. See
`design/compatibility-diagnostics.md` §3. Run only the Landmark tests with:

```sh
cargo test --offline -p cli --test landmark -- --ignored user_rom_landmark_at_real_mode_prefix --nocapture
```

```sh
UNEMUPC98_REQUIRE_USER_ROMS=1 bash scripts/ci-gate.sh user-roms
```

See `vendor/user-roms/README.md` for catalogue hashes and the proprietary golden rule.

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

The SeaBIOS ISA fixture uses its own firmware devShell, separately from the CI
`fixtures` gate:

```sh
nix develop .#firmware --command bash vendor/seabios/fetch.sh
```

It exports pinned rel-1.17.0 from `vendor/seabios/src/` (offline when cached),
checks input hashes and builds `vendor/seabios/bios-1.17.0-isa.bin` with fixed
version metadata. The output SHA-256 is pinned after two matching independent
builds on 2026-11-11. No image is installed before verification. No board or
catalogue entry is added; that entry waits for `generic-386-isa` and the x86jit
386 CPU model.
See `vendor/seabios/README.md` for tools, configuration and provenance.

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
