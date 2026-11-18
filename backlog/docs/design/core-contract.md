# Core contract and the front-end split

Status: **Accepted** by the maintainer on 2026-11-17 (design step of
`upc-dfs.24`, with `upc-3bm`), with the recommended answer to each question
of §11. Steps 1 to 3 of §7.5 and steps 1 and 2 of §8 are implemented (the
contract crate of §3 and the PC core of §4 in its stage A came with §8 step
2); the other steps land one at a time. §8.1, the plan of steps 3 to 6 of
§8, was accepted by the maintainer on 2026-11-18. The decision record is
[decision-40](<../../decisions/decision-40 - Front ends drive emulator cores through one contract crate.md>).

Markers: **[REPO]** = read in this repository at `896c5d9`, with the path;
**COUNTED** = counted for this document by a throwaway script over the
sources (reference counts are approximate: the script reads `crate::path`
uses and cuts each file at its first test module); **PRELIMINARY** = read
from the measurement logs of the change-targeted testing work (`upc-x83.19`)
on 2026-11-17, taken on a shared development host under load, and replaced
by that work's own write-up when it lands; **ASSUMED** = not verified.

## 0. The design in ten lines

1. One small crate, working name **`unemu-core-api`**, is the whole contract
   between a front end and an emulator core: a `Core` that describes itself
   and creates machines, and a `Machine` the front end drives.
2. A machine is driven by five kinds of call: run to a limit, read the
   picture and the sound, send input, change media, and control (reset,
   save and load state, finish). Pause is the absence of a call.
3. Time is the core's own: virtual **ticks** at an exact rational rate, and
   **steps** in the core's own unit. The contract has no wall clock.
4. What a core can do is **described as data**: the core, each platform it
   provides, and each machine (displays, sound, input ports, media slots,
   resets, saved state). A front end enables a control from the description.
5. The contract has **no dependencies**, no generics and no platform words.
   Every type is `#[non_exhaustive]`, so a minor version only adds.
6. A core states the contract version it **implements**; the front end
   refuses another major, and shows the platform disabled with the reason.
7. Everything a platform knows about artifacts stays where it is: the
   provider traits of `image-store` (`ConfigValidator`, `MediaFormats`,
   `FirmwareRules`) and the provider crate (`pc-image`). Platform-only
   commands and dialogs stay in per-platform modules.
8. The PC core implements the contract in an adapter crate, working name
   **`unemu-pc-core`**, over today's `assembly`; scan codes, the 8042 queue
   and the CP437 rasteriser move below the contract with it.
9. `cli` splits into **`cli-core`** (neutral), **`cli-pc`** (PC provider
   commands), the **`upcli`** binary, and **`pc-harness`** (the golden
   runners and the 32 test binaries that use nothing else); `window` stops
   depending on `cli`.
10. A script reading `cargo metadata` keeps the layers apart in CI. Every
    step lands on `main` with the full gate and no behaviour change.

## 1. Direction and scope

The maintainer's direction is binding and restated here. The desktop GUI
and the command line leave unemupc for one front-end repository that pins
emulator cores by exact version, as unemupc pins x86jit. A NES emulator
(unemunes) comes next. The agreed shape:

1. A small, stable, versioned **core contract crate**: the machine trait
   the front end drives, and a description of the core's capabilities. The
   front end depends only on it; every emulator implements it. There is no
   grab-bag `unemu-core` crate.
2. The platform-neutral library and store crates that exist (`image-store`,
   `image-registry`, the media builders, the coming recipe crate) stay
   separate crates on the front-end side.
3. Shared emulation building blocks (scheduler, clock, device traits,
   `testkit`) are shared only when a second emulator needs them.

The command line splits into a platform-neutral core and per-platform
provider commands, both on the front-end side. Platform-specific GUI parts
stay in per-platform modules and are generalised only when a second
platform needs it. **Success criterion: a CLI-only change builds and tests
in under five minutes.**

In scope: the inventory of today's coupling (§2), the contract (§3), the
PC core on it (§4), the front end on it (§5), a sanity check against a NES
core (§6), the CLI split (§7), the window split (§8), the road to two
repositories (§9), and the questions the maintainer answered (§11).

Out of scope: designing the NES emulator; a plug-in system or cores loaded
at run time (cores are compiled in, at pinned versions); sharing device or
scheduler crates; a transport for the contract (it is a Rust API; nothing
in it prevents a later process boundary, and nothing here needs one); the
recipe engine ([recipes.md](recipes.md)); new features of any kind.

## 2. Inventory: today's coupling

### 2.1. Crate graph

Seventeen workspace crates, about 300 000 lines with tests (COUNTED). The
workspace dependencies below are the normal dependencies of each
`Cargo.toml` [REPO: `crates/*/Cargo.toml`]; test-only ones are named in the
text where they matter.

| Crate | Lines | Workspace dependencies | Layer today |
|---|---:|---|---|
| `device` | 1 571 | none | emulation |
| `bus` | 2 325 | `device` | emulation |
| `cpu` | 2 240 | none (`x86jit-core`) | emulation |
| `video` | 1 000 | `device` | emulation |
| `audio` | 1 627 | `device` | emulation |
| `devices` | 38 179 | `device` | emulation |
| `chipset` | 1 147 | `device`, `devices` | emulation |
| `machine` | 5 785 | `device`, `cpu`, `bus` | emulation |
| `testkit` | 446 | `device`, `cpu` | emulation (tests) |
| `assembly` | 36 796 | `machine`, `cpu`, `bus`, `device`, `devices`, `video`, `audio`, `chipset`, **`media`** | emulation |
| `media` | 12 079 | none | neutral |
| `image-store` | 17 283 | none | neutral |
| `image-registry` (`crates/registry`) | 12 124 | `image-store` | neutral |
| `machine-file` | 17 184 | `assembly`, `media`, **`image-store`** | PC |
| `pc-image` | 26 792 | `assembly`, `machine-file`, `media`, `image-store` | PC provider |
| `cli` | 29 019 | `pc-image`, `image-store`, `image-registry`, `machine-file`, `assembly`, `device`, `bus`, `cpu`, `chipset`, `machine`, `media`, `devices`, `video` | front end |
| `window` | 94 972 | `image-store`, `image-registry`, `pc-image`, **`cli`**, `media`, `audio`, `device`, `devices`, `machine-file`, `video`, `assembly`, `machine` | front end |

Read as layers, bottom to top:

```text
emulation   device  bus  cpu  video  audio  devices  chipset  machine  testkit
            assembly ───────────────────────────────┐ (journals at run time)
neutral     media   image-store   image-registry    │
PC          machine-file (assembly, media, image-store)
            pc-image     (assembly, machine-file, media, image-store)
front end   cli     (everything above)
            window  (everything above, and cli)
```

Three edges cross the lines the split needs, and are marked bold in the
table:

- `assembly` → `media`: the running machine owns its journal sessions
  [REPO: `crates/assembly/src/persistence.rs`].
- `machine-file` → `image-store`: a machine file may take firmware from a
  package in the store [REPO: `crates/machine-file/src/firmware_package.rs`,
  `crates/machine-file/src/lib.rs`].
- `window` → `cli`: the desktop takes shared helpers from the command-line
  crate (§2.5).

### 2.2. What is already neutral, and the seams that exist

- **`image-store`** and **`image-registry`** depend on no emulator crate
  [REPO: their `Cargo.toml`]. The store's module documentation states the
  rule: "Platform providers own config validation and ordered graph
  traversal; this crate owns no emulator, registry, disk reconstruction,
  or GC" [REPO: `crates/image-store/src/lib.rs`]. Some of their vocabulary
  is PC-shaped (disk and floppy payload media types), but it carries no
  emulator behaviour.
- **Provider traits** already route by platform, all in `image-store`
  [REPO: `crates/image-store/src/store.rs`, `media_set.rs`, `firmware.rs`]:
  - `ConfigValidator`: `artifact_type`, `platform`, `payload_types`,
    `validate(config, descriptors, blobs) -> Vec<Digest>`,
    `validate_resolved`, `required_release` (decision-30).
  - `MediaFormats`: `platform`, `validate_name`, `validate_format`.
  - `FirmwareRules`: `platform`, `validate`, `validate_name`.
  - `BlobSource`: `read_verified`, `open_verified`: verified content by
    descriptor, the store's side of every provider call.
- **Platform ids** are compared through `image_store::platform::same`
  only; the alias table lives in the shared crate
  ([machine-images.md](machine-images.md) §1, decision-32).
- **The provider list** is one function,
  `with_image_providers(|&[&dyn ConfigValidator]| …)`, which both front
  ends call "so they accept the same artifacts" [REPO:
  `crates/cli/src/image/registry.rs`]. It lives in `cli`, which is why
  `window` depends on `cli`.
- **`media`** depends on no workspace crate. It holds two things: the
  journal and overlay format with its offline tools (`sink`, `overlay`,
  `scan`, `offline`, `layer`), and the FAT12 builder and image creation
  (`fat12`, `create`).
- **Inside the emulator** the swap seams of decision-7 hold: `Cpu`,
  `Device`, `InterruptController`, `FrameSink`, `AudioSink`, and
  `device::SectorMedia`, the hardware-neutral mounted medium a controller
  reads and writes [REPO: `crates/device/src/media.rs`].
- **The capabilities sketch** of machine-images §6 exists as PC code:
  `assembly::descriptors` (setting domains) and `assembly::capabilities`
  (choices and diagnostics over pending edits).

What does not exist: any seam between a front end and a *running* machine.
Both front ends hold an `assembly::AssembledMachine` and reach into it.

### 2.3. `cli`: module by module

One library and the `upcli` binary. Line counts are COUNTED; the uses are
[REPO] in the named file.

| Module | Lines | PC crates it uses, and why | Kind |
|---|---:|---|---|
| `lib.rs` | 2 471 | `assembly`, `machine`, `cpu`, `device`, `devices`, `video`: the golden runners (`run_bios*`, `run_freedos*`, `run_boot`, `run_keyboard`, `run_rtc`, `run_fdc_write`, `run_vga_text`, `run_test386`), the hand-encoded guest programs, key scripts, `StopReason`, and `finish_persistence` | test harness |
| `command.rs` | 1 037 | Usage text and the top-level parser. PC: `--boot`, `--bios`, `machine check`, `run --machine` (`assembly`, `machine_file`, `machine`, `cpu`, `video` for `--print`). Neutral: `media generations`, `verify`, `rollback`, `commit` (`media::offline` only) | mixed |
| `create.rs` | 427 | `media create floppy`, `hd`: `media::fat12`, `media::create`, and `assembly::HardDiskGeometry` for the CHS policy | PC |
| `image.rs` | 611 | Neutral: `image ls`, `workspace ls`, `workspace discard`. Through a provider: `compress` (picks a validator by artifact type), `sweep` (`pc_image::workspace::sweep_materialized`). PC: `import` (machine file to image), `run` (`assembly::assemble_with_verified_bases`), `publish`, `abandon-publication` | mixed |
| `image/media.rs` | 701 | Neutral: `image media ls`, `show`. PC: `image media import` through `pc_image::media_import` | mixed |
| `image/firmware.rs` | 870 | Neutral: `image firmware ls`, `show`. PC: `import`, `edit` match ROMs against the firmware catalogue and the ROM index (`assembly::FirmwareEntry`, `assembly::firmware_discovery`, `pc_image::firmware_import`) | mixed |
| `image/registry.rs` | 1 066 | `push`, `pull`, `remote ls`, `login`, `registry set` are neutral. PC enters in two places only: the provider list (`with_image_providers`) and the catalogue marks of the push guard (`guard_marks`, `pc_image::firmware::proprietary`) | neutral with two hooks |
| `image/template.rs` | 331 | `image template build`: the official PC templates (`pc_image::template`) | PC |
| `image/machine.rs` | 425 | `image machine *`: every action is a `pc_image::machine` function; `run` assembles | PC |
| `preferences.rs` | 245 | none: the path of the shared preferences file | neutral |
| `about.rs` | 77 | `devices::OPL3_NOTICE` in the notices | neutral with one notice |
| `main.rs` | 17 | host paths: `assembly::firmware_discovery::firmware_cache_path`, `image_store::location::default_store_root` | binary |

`bus` and `chipset` appear only in the test code of `cli` (COUNTED), though
they are declared as normal dependencies.

### 2.4. `cli` tests

53 integration test binaries plus the library's unit tests (COUNTED from
`crates/cli/tests`). They fall into three groups:

- **Emulation goldens, 32 binaries**: BIOS POST, FreeDOS boot and
  sessions, floppy, IDE, VGA, EMS, the sound cards, the scheduler A/B
  tests. They use the runners of `lib.rs` or the emulation crates directly,
  never the command line, the binary or the store: `at_pm_test_golden`,
  `bios_boot_golden`, `bios_floppy_boot_golden`, `boot_golden`,
  `dos_interactive_golden`, `dos_write_golden`, `fdc_write_golden`,
  `fixture_policy`, `floppy_boot_golden`, `floppy_swap_golden`, `fpu_bios`,
  `freedos_boot_golden`, `ide_freedos_golden`, `keyboard_golden`,
  `landmark`, `lotech_ems_golden`, `media_device`, `media_fixtures`,
  `open_vga_golden`, `opl3_golden`, `quantum`, `rtc_golden`, `sb16_golden`,
  `sb16_word_golden`, `scheduler_ab`, `serial_mouse_golden`,
  `speaker_golden`, `test386_golden`, `vga_bios`, `vga_scrolling_golden`,
  `vga_text_golden`, `xms_warm_boot_golden`. Eleven of them load machine
  files through `machine_file`.
- **Goldens that need a front end, 7 binaries**: `at_games`,
  `freedos_install_golden`, `ide_journal_golden` and `media_files_golden`
  call `cli::command` beside the emulator; `firmware_catalogue` and
  `timing_adoption` run the `upcli` binary; `machine_freedos_golden` runs a
  machine from the image store through the provider.
- **Command tests, 14 binaries**: `image_commands`,
  `image_firmware_commands`, `image_machine_commands`,
  `image_media_commands`, `image_registry_commands`,
  `image_template_commands`, `platform_alias`, `firmware_host_guard`,
  `machine_check`, `machine_commands`, `media_create`,
  `media_create_files`, `media_fifo`, `media_tools`.

So 32 of the 53 binaries a change to a command module rebuilds and relinks
today are emulator goldens that it cannot affect.

### 2.5. `window`: module by module

One small library (`font`, `raster`, `keyboard`, `mouse`, `WindowSink`) and
the `unemupc` binary, which holds everything else as modules of `main.rs`:
the whole desktop is **one compilation unit and one test binary** [REPO:
`crates/window/src/main.rs`, `crates/window/Cargo.toml`].

| Group | Modules (lines) | PC crates used in production code, and why |
|---|---|---|
| Machine window | `main.rs` `Presenter` (3 387 with tests), `pacing` (501), `host_audio` (504), `keyboard` (1 035), `mouse` (524), `raster` (511), `font` (269), `lib.rs` (157), `drive_chips` (1 030), `system_keys`, `macos_keys` | `assembly::AssembledMachine` and `machine::RunOutcome` drive the run loop (§2.6); `devices::I8042` and the set-1 tables in `keyboard`; `video::Frame` in `raster`; `device::clock::PIT_HZ` in `pacing`; `audio` sink types in `host_audio` |
| Shell | `shell` (14 527, of which about 8 800 are tests), `shell_model` (1 321), `shell_icons`, `app_menu`, `macos_menu`, `logo`, `file_dialog`, `text_clipboard`, `preferences`, `preferences_sheet` | `assembly`: assembling on Start (`assemble_with_verified_bases`, `ObservationPolicy`), `FloppyAction`, `WriteMode`, `ImageSource`, `DeviceId`, `firmware_discovery`; `machine_file::PreparedMachineFile` for machine files; `pc_image::machine::State`; `cli::preferences`, `cli::about`; `devices::OPL3_NOTICE` |
| Library and Remotes | `library_view` (6 307) with `machines`, `firmware`, `firmware_metadata`, `hardware`, `install_media`, `pane`, `remotes`, `remotes/details`; `library_worker` (3 667); `remote_browse` (3 989); `remote_pull`; `registry_transfer`, `registry_transfer_view`; `registries_model`; `save_image`; `layers_dialog`; `image_settings`; `settings_model`, `settings_sheet`; `new_machine`, `new_machine_view`; `firmware_choices` | `pc_image` throughout: `machine::*` (machine instances), `workspace`, `revision`, `inspection`, `create`, `template`, `firmware`, `firmware_import`, `media_set`, `is_pc`, `platform_name`, `media_repository`. `assembly::capabilities` and `descriptors` in settings and New machine; `assembly::FirmwareEntry` in the firmware views; `machine_file::parse_toml` and `resolve` in settings. `cli::with_image_providers`, `push_plan`, `registry_client`, `registry_probe_client` |
| Media picker and dialogs | `media_picker` (4 161), `media_dialog` (2 571), `media_create` (1 718), `created_media` (910), `catalogue_menu` (1 190) | `assembly::FloppyGeometry`, `FloppyMedium`, `FloppyMediaList`, `DriveKind`, `WriteMode`, `ImageSource`, `descriptors`; `media::fat12`, `VerifiedBase`; `pc_image::machine`, `workspace`, `media_import`; `machine_file::schema`, `resolve`; `cli::create` (the image-creation request and its executor) |

Fifteen modules name no emulation or PC crate in production code:
`app_menu`, `file_dialog`, `font`, `logo`, `macos_keys`, `macos_menu`,
`shell_icons`, `system_keys`, `text_clipboard`, `library_view/pane`,
`registries_model`, `preferences_sheet`, `registry_transfer_view`,
`save_image` and `library_view/remotes/details` (COUNTED). Every other
module names at least `assembly`, `pc_image`, `machine_file` or an
emulation crate.

The modules reference each other in cycles: `library_view` uses 20 sibling
modules and `shell` 22, and `library_worker`, `media_picker`,
`registry_transfer` and `new_machine` each point back at `library_view` or
`shell` (COUNTED). Most arrows into `shell` are for six skin helpers:
`skin_visuals`, `configure_context`, `gradient_title`, `escape_pressed`,
`bevel`, `bevel_ring` (95 references with tests, COUNTED).

Production uses of `cli` in `window` [REPO]:

| Use | Where | What it is |
|---|---|---|
| `cli::with_image_providers` | `library_worker.rs` | the provider list |
| `cli::push_plan` | `registry_transfer.rs`, `library_view/firmware_metadata.rs` | push plan with the catalogue marks |
| `cli::registry_client`, `registry_probe_client` | `registry_transfer.rs`, `remote_browse.rs` | a registry client for a host and its account |
| `cli::create::{CreateCommand, parse, execute, …}` | `media_dialog.rs` | blank and formatted images |
| `cli::preferences::path` | `shell.rs` | the shared preferences file |
| `cli::about::licence_notice` | `shell.rs`, `main.rs` | the About text |

The tests of `window` also call the whole command line
(`cli::command::run`) to prepare fixtures in six places [REPO:
`library_test_support.rs`, `shell.rs`, `media_dialog.rs`,
`library_view/install_media.rs`].

### 2.6. How the window drives a machine today

The session is `Presenter` in `crates/window/src/main.rs`. Per run chunk it
makes these calls [REPO]; the contract of §3 is cut along them.

| Purpose | Today's call |
|---|---|
| Create | The library worker thread prepares the session (`pc_image::machine::prepare` or `workspace::prepare_pinned`: the configuration and its verified bases) and hands it over as a `SessionTransfer`; the UI thread calls `assembly::assemble_with_verified_bases(config, policy, options, bases)`. A machine file goes through `machine_file::prepare_for_assembly_with_store_root` |
| Time | `machine.clock()` (ticks at the PIT input rate), `machine.virtual_time()` (retired instructions plus halted clocks) |
| Run | `pacing::run_cap(target, clock, time, rate)`, then `machine.run_bounded(Some(cap))` → `RunOutcome::{RetiredCapReached, Halted, ResetRequested}` |
| Guest reset request | `assembled.before_cpu_reset()`, `assembled.reset_cpu(ResetCause::Requested)` |
| Picture | `observation.subscribe(display, sink)`, then `assembled.capture_text(display)`; `WindowSink` rasterises the `video::Frame` with the bundled CP437 fonts |
| Sound | `observation.subscribe_audio(sink)`, `assembled.capture_audio()`; 48 kHz stereo `i16` |
| Keyboard | `HostKeyboard::key_event` (winit key code to set-1 scan codes, typematic, NumLock-aware wrappers), then `inject_at_boundary` (8042 back-pressure) |
| Mouse | `HostMouse` (counts, button queue), `assembly::inject_mouse` at the boundary |
| Media | `assembled.change_floppy(DeviceId::Fdc, unit, FloppyAction::{Eject, Insert(i), Next})`, `append_floppy_verified`, `floppy_name`; slots from `media_dialog::FloppySlot::from_config(&MachineConfig)` |
| Drive lights | downcast to `devices::Upd765` and `devices::Ata`: `drive_active(unit)`, `activity(unit)` |
| Host reset | `reset_halted`: `before_cpu_reset`, `reset_cpu(ResetCause::Host)`, only while halted |
| Power cycle | the shell stops the session and starts it again |
| Persistence | `check_persistence`, `flush_persistence`, `finish_persistence`, `PersistenceHandle::maintenance_failure` |
| Diagnostics | `unlisted_firmware`, `observation.take_speaker_trace`, lines on standard error |
| Pause | the shell stops calling `advance` and re-anchors `pacing` on resume |
| Save state | a disabled menu item; the machine has no saved state yet |

### 2.7. What crosses the future repository line

For two repositories, each of these must be cut or pointed the other way.
None of them blocks the splits inside the workspace (§7, §8).

| Edge | Why it exists | What removes it |
|---|---|---|
| `window` → `cli` | shared helpers live in the command-line crate | §7 step 2 |
| `cli` runners used by goldens and by command code | one library holds both | §7 step 3 |
| `window` library (fonts, raster, scan codes) → `video`, `devices`, `assembly` | PC presentation and input encoding live in the desktop | §4, with the adapter |
| `window` and `cli` → `assembly::AssembledMachine` | no run-time seam | the contract, §3 |
| `pc-image` → `assembly`, `machine-file` | the PC config codec, the firmware catalogue, capabilities | stays: a provider crate may use its own core's crates (§3.12, §11 Q3) |
| `machine-file` → `image-store` | firmware packages from the store | a small lookup trait in `machine-file` that the provider implements over the store (§9) |
| `assembly` → `media` | journals written while the machine runs | §9 and §11 Q4 |

### 2.8. Build and test time today

All figures here are PRELIMINARY (shared host under load, six compile jobs,
four test threads).

- A clean build of every test binary took 200 s. The largest single units
  were the `unemupc` test binary (22 s), `assembly`'s tests (18 s), the
  `unemupc` binary (13 s) and `machine-file` (11 s each for the library
  and its tests). Each `cli` test binary took about 1.3 s; there are 53.
- Running every test binary, slow goldens included, took 460 s of test
  time. The `unemupc` binary's tests took 82 to 88 s with four threads
  (203 s with one); the per-test times add up to 294 s, of which the
  library views with their layout tests and the library worker are about
  150 s, the settings, New machine and transfer dialogs about 60 s, the
  three sound-level tests 44 s, and the shell 18 s.
- With the affected-set runner of `upc-x83.19` at its fast tier, a change
  to `crates/window/src/pacing.rs` took 134 s, a change to
  `crates/cli/src/image.rs` took 154 s (it selects `cli` and `window`),
  and a change to `crates/image-store/src/library.rs` took 291 s.

So the runner already brings a CLI-only change under five minutes at the
fast tier, but most of that time is the desktop being rebuilt and
re-tested for a change that cannot affect it. The split removes that work
by construction, where the runner can only select around it.

## 3. The contract crate

Working name `unemu-core-api` (§11 Q2); directory `crates/core-api` while it
lives in this workspace (§11 Q1).

### 3.1. Rules of the crate

- **No dependencies.** `std` only. Errors implement `std::error::Error` by
  hand. This departs from the `thiserror` convention for library errors on
  purpose: nothing in the contract may force a version of another crate on
  a core.
- **Object-safe, no generics.** The front end holds `&dyn Core` and
  `Box<dyn Machine>`. Cores are compiled in; the traits also describe a
  later process boundary without change.
- **Additive.** Every struct and enum is `#[non_exhaustive]`. A struct a
  core constructs has a `new` with its required fields and `with_…`
  setters. A minor version adds fields, variants, and trait methods with a
  default body; it removes and changes nothing.
- **No platform words.** No port, IRQ, scan code, floppy geometry,
  cartridge mapper or firmware catalogue appears in it.
- **No host clock, no randomness, no threads.** No `std::time` type
  appears in any signature.
- **No presentation policy.** Pacing, speed, pause, frame rate, window
  size, fonts, key bindings and audio devices are the front end's.
- `#![forbid(unsafe_code)]`.

### 3.2. Version

```rust
/// A version of the contract between front ends and cores.
///
/// `major` changes when a core or a front end has to be edited to keep
/// working. `minor` changes when something is added: a method with a default
/// body, a field, an enum variant. While `major` is 0 every minor may break.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContractVersion {
    pub major: u16,
    pub minor: u16,
}

/// The contract this source defines.
pub const CONTRACT: ContractVersion = ContractVersion { major: 0, minor: 1 };

impl ContractVersion {
    /// Whether a front end written for `self` may drive a core that
    /// implements `core`: the same major, and during major 0 the same minor.
    /// Within one major, a core that implements an older minor is driven
    /// with the newer additions treated as absent.
    pub const fn drives(self, core: ContractVersion) -> bool {
        self.major == core.major && (self.major != 0 || self.minor == core.minor)
    }
}
```

How the front end checks a core:

1. **At build time, by Cargo.** The front end and every core name the
   crate with a SemVer requirement. Compatible requirements resolve to one
   copy, so the trait a core implements is the trait the front end calls.
   Requirements of two majors give two copies, and the core's
   `impl Core` does not satisfy the front end's trait: the build fails at
   the line that lists the core. This only works when every consumer takes
   the crate from **one source** (§11 Q1): Cargo treats the same crate from
   two git revisions as two crates.
2. **At start-up, by value.** `CoreDescription::implements` is a literal
   the core's author writes, never the `CONTRACT` constant, because the
   constant follows whichever copy Cargo resolved. The front end calls
   `CONTRACT.drives(core.description().implements)`. A core that fails is
   listed with its platforms disabled and the reason "core NAME implements
   contract A.B; this program needs C.D", the disabled-with-reason pattern
   of decision-30.
3. **In the front end's tests.** One test asserts that every compiled-in
   core passes the check, so a wrong pin fails CI and not a user.
4. **For additions.** Before the front end relies on behaviour a minor
   added, it compares `implements.minor`. A method added later has a
   default body that returns `ErrorKind::Unsupported`.

### 3.3. The core and its capabilities

```rust
/// One emulator core: a stateless factory for machines of the platforms it
/// provides.
///
/// A front end holds every compiled-in core for the life of the program and
/// may call it from any thread.
pub trait Core: Send + Sync {
    /// What this core is and what it can do. Constant for the life of the
    /// program.
    fn description(&self) -> &CoreDescription;

    /// Build a machine, stopped at virtual time zero.
    ///
    /// Reads the config document, takes every blob it names from
    /// `request.content`, and writes only below `request.session`. Fails
    /// with `ErrorKind::Config`, `Content` or `Unsupported`; a failed create
    /// leaves nothing running and nothing half-written that a later create
    /// could mistake for a session.
    fn create(&self, request: &CreateRequest<'_>) -> Result<Box<dyn Machine>, Error>;
}

/// A core, as data. A front end lists cores and enables controls from this
/// alone, before any machine exists.
#[non_exhaustive]
pub struct CoreDescription {
    /// The contract version the core's author wrote the core against: a
    /// literal, not [`CONTRACT`].
    pub implements: ContractVersion,
    /// Stable id of the core, such as `unemupc`.
    pub id: &'static str,
    /// The core's name for people.
    pub name: &'static str,
    /// The core's release, `MAJOR.MINOR.PATCH`.
    pub version: &'static str,
    /// The machine families the core provides.
    pub platforms: &'static [PlatformDescription],
    /// Licence notices of the third-party components compiled into the core,
    /// for the front end's About text.
    pub notices: &'static [Notice],
}

/// One machine family of a core.
#[non_exhaustive]
pub struct PlatformDescription {
    /// The platform id artifacts name, in its current spelling. Retired
    /// spellings are the front end's concern; the core sees the current one.
    pub id: &'static str,
    /// The platform's name for people.
    pub name: &'static str,
    /// Media types of the config documents `create` reads.
    pub config_types: &'static [&'static str],
    /// The most a machine of this platform can do; one machine may do less
    /// and says so in its [`MachineDescription`].
    pub features: Features,
    /// Run-only options `create` accepts, outside machine identity.
    pub options: &'static [OptionDescription],
}

/// What machines can do. Every field has a "no" value, so a newer field is
/// "no" for an older core.
#[non_exhaustive]
pub struct Features {
    /// Kinds of reset the machine accepts.
    pub resets: &'static [ResetKind],
    /// Whether state can be saved and loaded.
    pub saved_state: bool,
    /// Whether removable media can change while the machine runs.
    pub media_changes: bool,
    /// Whether a run can stop when a display finishes a frame.
    pub frame_stops: bool,
}

/// A third-party component inside a core and its licence notice.
#[non_exhaustive]
pub struct Notice {
    pub component: &'static str,
    pub text: &'static str,
}

/// A run-only option: a permission or a diagnostic switch that never enters
/// a machine's identity, its config or a stored artifact.
#[non_exhaustive]
pub struct OptionDescription {
    pub key: &'static str,
    pub summary: &'static str,
}
```

Capabilities are described at three levels, each narrower than the last:
the **core** (which platforms), the **platform** (`Features`: what any of
its machines may do), and the **machine** (`MachineDescription`, §3.5: what
this one has). A front end enables a control from a description and never
by trying a call. A call the description promised that answers
`Unsupported` is a core bug.

### 3.4. Creating a machine

```rust
/// Everything a core needs to build one machine.
#[non_exhaustive]
pub struct CreateRequest<'a> {
    /// The platform id, in its current spelling.
    pub platform: &'a str,
    /// The resolved machine configuration: the document a stored image keeps
    /// as its config, after any local settings were applied to it.
    pub config: ConfigDocument<'a>,
    /// Verified content by digest: firmware, disk bases, media.
    pub content: &'a dyn ContentSource,
    /// A directory the front end owns for this machine. The core keeps
    /// there what must outlive the run: written sectors, non-volatile
    /// memory, saved battery memory. The core never writes outside it and
    /// never changes a file it took from `content`.
    pub session: &'a Path,
    /// Run-only options, by the keys the platform describes. An unknown key
    /// is refused.
    pub options: &'a [(&'a str, &'a str)],
}

/// A config document with its media type.
#[non_exhaustive]
pub struct ConfigDocument<'a> {
    pub media_type: &'a str,
    pub bytes: &'a [u8],
}

/// Content addressed by SHA-256.
///
/// The source has hashed the whole content, never a prefix and never a
/// sample, before it hands it over, however large it is. The core trusts
/// the receipt and does not hash again.
pub trait ContentSource {
    /// The exact bytes with this digest and length.
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error>;
}

/// Verified read-only content.
#[non_exhaustive]
pub enum Content {
    /// Small content, in memory.
    Bytes(Vec<u8>),
    /// Large content, opened for random access. The core reads it and never
    /// writes it.
    File(std::fs::File),
}
```

The shape follows today's flow (§2.6). The front end verifies the closure
on a worker thread, as `prepare` does now, and its `ContentSource` then
answers from receipts, so `create` on the UI thread does not hash a disk.
The list of digests to verify comes from the provider's
`ConfigValidator::validate`, which already returns it.

`create` takes one input form: the platform's stored config document. Any
other way to describe a machine is a provider matter (§3.12): a PC machine
file with host paths, ROM folders and `extends` is turned into a machine
by the PC provider, which hands the front end the same `Box<dyn Machine>`.

**Media design constraint.** The contract keeps layered disk images
possible ([deferred.md](../deferred.md), Layered disk images): a base is
read-only content identified by its SHA-256, hashed in full by the source;
written sectors go to the session directory and never into the base;
nothing in the contract rewrites a base in place (committing a disk is a
store operation on the front-end side that writes a new base); and a
medium reaches the core as one `Content` plus one write target (§3.8), so a
chain of hash-linked layers can stand behind the same two values later.

### 3.5. The machine: description, time and run

`Machine` is one trait. It is shown here in parts, by subject, over this
and the next five sections.

```rust
/// One running machine.
///
/// A machine lives on the thread that created it; it need not be `Send`.
/// It advances only inside [`Machine::run`]. Between calls nothing happens:
/// pause is the front end not calling `run`.
pub trait Machine {
    /// What this machine has. Constant for the machine's life.
    fn description(&self) -> &MachineDescription;

    /// Virtual time now, in ticks at `description().clock`.
    fn now(&self) -> Ticks;

    /// Work done so far, in the core's own step unit
    /// (`description().step_unit`), for budgets that must not depend on the
    /// clock rate.
    fn steps(&self) -> u64;

    /// Whether the machine can run.
    fn status(&self) -> Status;

    /// Advance until a limit or an event, whichever comes first.
    ///
    /// Input and media changes made since the last return take effect
    /// first, at `now()`. A deadline is met at the core's next boundary, so
    /// `now()` may pass `limit.until` by less than one step. A core that
    /// turns the deadline into a budget of steps may also answer
    /// `Stop::Deadline` with `now()` short of `limit.until` by less than
    /// one step; the front end runs again while it is short. Otherwise a
    /// run never returns before a limit without an event. Fails with
    /// `ErrorKind::Guest` when the emulated machine cannot continue and
    /// with `ErrorKind::Storage` when a host write failed; either leaves
    /// the status `Failed`, except that a `Storage` failure while the guest
    /// halts may leave it `Halted`, so the session can still be finished.
    /// `stop_on_frame` on a machine whose features do not list
    /// `frame_stops` fails with `ErrorKind::Unsupported`.
    fn run(&mut self, limit: RunLimit) -> Result<Stop, Error>;

    // Output: §3.6. Input: §3.7. Media: §3.8. Control: §3.9.
}

/// What one machine has.
#[non_exhaustive]
pub struct MachineDescription {
    /// The rate of [`Ticks`], exact.
    pub clock: Rate,
    /// A word for one step, for messages: `instruction`, `cycle`.
    pub step_unit: &'static str,
    pub displays: Vec<DisplayDescription>,
    pub audio: Option<AudioDescription>,
    pub inputs: Vec<InputPortDescription>,
    pub slots: Vec<SlotDescription>,
    /// What this machine can do, within its platform's `Features`.
    pub features: Features,
}

/// A point in virtual time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ticks(pub u64);

/// An exact rate per second, as a fraction in lowest terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub numerator: u64,
    pub denominator: u64,
}

/// Where a run stops if nothing else stops it first. Built with
/// `RunLimit::until`, `RunLimit::steps` and `with_…` setters.
#[non_exhaustive]
pub struct RunLimit {
    /// Stop at this virtual time.
    pub until: Option<Ticks>,
    /// Stop when `steps()` reaches this value.
    pub steps: Option<u64>,
    /// Stop when a display finishes a frame.
    pub stop_on_frame: bool,
}

/// Why a run returned.
#[non_exhaustive]
pub enum Stop {
    /// `limit.until` was reached.
    Deadline,
    /// `limit.steps` was reached.
    Steps,
    /// A display finished a frame.
    Frame(DisplayId),
    /// The guest stopped executing and nothing scheduled can wake it; the
    /// status is `Halted`. A reset or a media change may still follow.
    Halted,
    /// The guest switched the machine off.
    PoweredOff,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// `run` advances the machine.
    Ready,
    /// The guest waits for a reset; `run` returns `Stop::Halted` at once.
    Halted,
    /// The guest switched the machine off.
    Off,
    /// A `Guest` or `Storage` error ended the run; only messages, the last
    /// frame and dropping the machine remain.
    Failed,
}
```

Two units, because today's machine already has both and they mean
different things [REPO: `crates/machine/src/lib.rs`, `clock`,
`virtual_time`]. **Ticks** are the clock that pacing, sound and the picture
follow: the front end turns elapsed host time into a tick deadline with
`description().clock`, as `pacing::virtual_ticks` does with `PIT_HZ` today.
**Steps** are the budget of a headless run (`--max-steps`), in the unit the
core counts work in. A core for which they coincide says so by using one
rate for both.

Guest-requested resets are not events. Today the window answers
`RunOutcome::ResetRequested` by calling the reset itself; under the
contract the core performs it inside `run` and reports it as a message
(§3.10), because no front end has a choice to make there.

### 3.6. Picture and sound out

```rust
pub trait Machine {
    /// The picture of a display at the last completed boundary.
    ///
    /// After an error the last picture stays readable. The borrow ends
    /// before the next call on the machine.
    fn frame(&mut self, display: DisplayId) -> Result<Frame<'_>, Error>;

    /// Move the sound rendered up to `now()` since the last call to the end
    /// of `out`, at `description().audio` rate.
    fn drain_audio(&mut self, out: &mut Vec<[i16; 2]>) -> Result<(), Error>;

    /// Say which outputs the front end consumes now. A core need not render
    /// or keep history for an output nobody takes.
    fn set_outputs(&mut self, outputs: Outputs);
}

/// One picture.
#[non_exhaustive]
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    /// `width * height` pixels, rows top to bottom, each `0x00RRGGBB`.
    pub pixels: &'a [u32],
    /// The shape of the whole picture on the emulated display, width to
    /// height, such as 4:3. The front end scales to it.
    pub aspect: (u32, u32),
    /// Changes whenever the pixels changed, so an unchanged picture is not
    /// uploaded again.
    pub serial: u64,
}

#[non_exhaustive]
pub struct DisplayDescription {
    pub id: DisplayId,
    /// The display's name for people, such as `VGA`.
    pub label: String,
}

#[non_exhaustive]
pub struct AudioDescription {
    /// Stereo frames per second, exact.
    pub rate: Rate,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outputs {
    pub audio: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayId(pub u8);
```

The picture is **pixels**, in the format the window already uses
("pixels are `0x00RRGGBB` u32 values" [REPO: `crates/window/src/lib.rs`]).
A text mode is a PC matter: its cell grid, attributes and fonts stay below
the contract, and the PC adapter rasterises (§4). Tests and `--print` that
want the text grid use the PC crates directly.

Output is **pulled**. Today's sinks are `Rc`-shared callbacks
(`WindowSink`, `WindowAudioSink`); a pull needs no shared ownership across
the contract and keeps the machine free of the front end's types. Sound
keeps its bounded host ring, mute, speed rules and resampling in the front
end [REPO: `crates/window/src/host_audio.rs`]; `set_outputs` carries what
`AudioSink::enabled` tells the machine today.

### 3.7. Input

```rust
pub trait Machine {
    /// Queue one input event. It reaches the guest at the start of the next
    /// `run`, in order, at `now()`. The core owns everything after that:
    /// encoding, repeat, device back-pressure, coalescing and overflow.
    fn input(&mut self, event: InputEvent);
}

#[non_exhaustive]
pub enum InputEvent {
    /// A key of a keyboard port, by physical position.
    Key { port: PortId, key: Key, state: KeyState },
    /// Relative pointer movement in device counts, positive `dy` up.
    PointerMotion { port: PortId, dx: i32, dy: i32 },
    /// A pointer button, numbered from 0: left, right, middle.
    PointerButton { port: PortId, button: u8, pressed: bool },
    /// A control of a game pad.
    Pad { port: PortId, control: PadControl, pressed: bool },
    /// The front end lost focus: every held key, button and control of
    /// every port is released.
    ReleaseAll,
}

/// A key by its position: the usage id of the USB HID keyboard page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key(pub u16);

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    Pressed,
    /// The host repeated a held key. A core with its own repeat ignores it.
    Repeated,
    Released,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PadControl {
    Up, Down, Left, Right,
    A, B, X, Y,
    L, R,
    Start, Select,
}

#[non_exhaustive]
pub struct InputPortDescription {
    pub id: PortId,
    pub kind: InputKind,
    /// The port's name for people, such as `Keyboard` or `Controller 1`.
    pub label: String,
}

#[non_exhaustive]
pub enum InputKind {
    Keyboard,
    /// A mouse or similar; the front end offers pointer capture when a
    /// machine has one.
    RelativePointer { buttons: u8 },
    Pad { controls: Vec<PadControl> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortId(pub u8);
```

The front end maps its toolkit's key codes to `Key` and decides what is a
host shortcut and what goes to the guest. Pointer capture, the release
chord and the swallowed first click stay in the front end. Sending
Ctrl+Alt+Del is three `Key` events and their releases.

### 3.8. Media slots and catalogues

A **slot** is a place a medium goes: a floppy drive, a fixed disk, a
cartridge port, a tape deck. A slot holds an ordered **catalogue** of
media and at most one of them is inserted. The front end changes what is
inserted; the core tells the guest in the way its hardware does.

```rust
pub trait Machine {
    /// Every slot's state now. Reading it takes the activity seen since the
    /// last call, for drive lights.
    fn slot_states(&mut self) -> Vec<SlotState>;

    /// Add a medium to a slot's catalogue and return its index. It is not
    /// inserted by this call.
    fn add_medium(&mut self, slot: SlotId, medium: NewMedium<'_>) -> Result<usize, Error>;

    /// Change what a slot holds, at the next run boundary.
    fn change_medium(&mut self, slot: SlotId, action: MediaAction) -> Result<(), Error>;
}

#[non_exhaustive]
pub struct SlotDescription {
    pub id: SlotId,
    pub kind: SlotKind,
    /// The slot's name for people, in the platform's words: `A:`, `C:`,
    /// `Cartridge`.
    pub label: String,
    /// Whether the catalogue and the inserted medium can change at all.
    pub removable: bool,
    /// Format ids of the media the slot takes, in the vocabulary of the
    /// platform's media formats. The front end offers only these.
    pub formats: Vec<String>,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    Floppy,
    FixedDisk,
    Optical,
    Cartridge,
    Tape,
}

#[non_exhaustive]
pub struct SlotState {
    pub id: SlotId,
    pub catalogue: Vec<MediumInfo>,
    /// Index into `catalogue` of the inserted medium.
    pub inserted: Option<usize>,
    /// Whether the slot was busy at any boundary since the last call.
    pub active: bool,
    /// Whether `change_medium` is accepted now.
    pub accepts_changes: bool,
}

#[non_exhaustive]
pub struct MediumInfo {
    /// A name for people; never a host path.
    pub name: String,
    pub sha256: [u8; 32],
    pub size: u64,
    pub format: String,
    pub writes: WriteKind,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteKind {
    /// The guest sees a write-protected medium.
    ReadOnly,
    /// Guest writes are kept in memory and lost with the session.
    Discarded,
    /// Guest writes are kept in the session, separate from the base.
    Kept,
}

/// A medium the front end hands to a running machine.
#[non_exhaustive]
pub struct NewMedium<'a> {
    pub name: &'a str,
    /// The read-only base, verified in full by the front end.
    pub content: Content,
    pub sha256: [u8; 32],
    pub size: u64,
    pub format: &'a str,
    pub writes: WriteTarget<'a>,
}

#[non_exhaustive]
pub enum WriteTarget<'a> {
    ReadOnly,
    Discarded,
    /// Keep written sectors in this file below the session directory. The
    /// core owns its format; the base is never changed.
    Kept { file: &'a Path },
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaAction {
    Eject,
    /// Insert a catalogue entry.
    Insert(usize),
    /// Insert the next entry, wrapping to the first; an empty slot takes
    /// the first.
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotId(pub u8);
```

The three actions and the append are exactly today's
`assembly::FloppyAction` and `append_floppy_verified` [REPO:
`crates/assembly/src/floppy.rs`], with the controller role and unit folded
into a `SlotId`. A fixed disk is a slot that is not removable; it still
reports activity.

The contract models the media of a **running machine**. The catalogue of a
stored image, a machine's pending inserts, installation media sets and the
picker's library are store data (`image-store`) and provider rules
(`MediaFormats`); they do not pass through the core.

### 3.9. Reset, pause, saved state, finish

```rust
pub trait Machine {
    /// Reset the machine. Fails with `Unsupported` for a kind the
    /// description does not list, and with `Usage` when the kind is not
    /// possible in the current status. A `Storage` or `Guest` failure of the
    /// reset itself leaves the status as it was (a halted machine stays
    /// `Halted`, so its session can still be finished).
    fn reset(&mut self, kind: ResetKind) -> Result<(), Error>;

    /// A token naming everything a saved state depends on: the core's
    /// release, the machine's config and the identity of its media. A state
    /// loads only into a machine with an equal token. `None` when the
    /// machine cannot save state.
    fn state_identity(&self) -> Option<String>;

    /// Write the whole machine state at the current boundary.
    fn save_state(&mut self, out: &mut dyn Write) -> Result<(), Error>;

    /// Replace the machine state with a saved one. On failure the machine
    /// is unchanged.
    fn load_state(&mut self, state: &mut dyn Read) -> Result<(), Error>;

    /// End the session cleanly: make everything written durable and close
    /// it as complete. Not valid once the status is `Failed`. Dropping a
    /// machine without `finish` is the equivalent of a power cut: what was
    /// already durable stays, and nothing is marked complete.
    fn finish(&mut self) -> Result<Finished, Error>;
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetKind {
    /// What the machine's reset line or button does: the processor
    /// restarts, memory, devices and media keep their state. Takes no
    /// virtual time.
    Soft,
    /// Power off and on: everything volatile starts fresh; media stay
    /// inserted and non-volatile memory stays.
    Hard,
}

/// What a clean end reports.
#[non_exhaustive]
pub struct Finished {
    /// Problems that did not fail the session, such as a retention pass
    /// that could not run.
    pub warnings: Vec<String>,
}
```

- **Pause** has no call. The front end stops calling `run` and re-anchors
  its pacing when it resumes, as the shell does today.
- **Reset.** The PC lists `Soft` and accepts it while `Halted`, which is
  today's `reset_halted`; a power cycle stays a front-end composition
  (finish, then create) until a core lists `Hard`.
- **Saved state.** The PC lists none yet; the three calls return
  `Unsupported`. `state_identity` is the `compatibility` field
  machine-images §2 reserves for a stored state. The state's bytes are the
  core's own format.
- **Finish** carries today's rule that "dropping this handle never writes a
  clean generation marker" [REPO: `crates/assembly/src/persistence.rs`]
  and that a failed execution is left unsealed.

### 3.10. Errors and messages

```rust
pub trait Machine {
    /// Move the core's messages since the last call to the end of `out`. A
    /// core never writes to the process's standard streams.
    fn drain_messages(&mut self, out: &mut Vec<Message>);
}

/// The one error type of the contract.
#[derive(Debug)]
#[non_exhaustive]
pub struct Error {
    pub kind: ErrorKind,
    /// One sentence a person can read, without a host path the user did
    /// not supply.
    pub message: String,
    /// For `Config`: where in the document, when the core knows.
    pub diagnostics: Vec<Diagnostic>,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The config document is malformed or names something this core does
    /// not have.
    Config,
    /// Content is missing or is not what its digest promised.
    Content,
    /// The core, the platform or this machine cannot do what was asked.
    Unsupported,
    /// The emulated machine cannot continue.
    Guest,
    /// A host write or read below the session directory failed. The guest
    /// never sees it.
    Storage,
    /// A saved state does not fit this machine.
    State,
    /// The call is not valid in the machine's current status.
    Usage,
}

#[non_exhaustive]
pub struct Diagnostic {
    /// A field path in the config document.
    pub path: String,
    pub message: String,
}

#[non_exhaustive]
pub struct Message {
    pub level: MessageLevel,
    pub text: String,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageLevel {
    Note,
    Warning,
}
```

One error type with a kind keeps every signature the same and lets a minor
version add kinds. A core keeps its own typed errors inside
(`AssemblyError`, `PersistenceError`, `RunError`) and maps them at the
adapter; the message is the typed error's text. Host failures stay out of
guest-visible state, as persistent-media § Host errors and determinism
requires: they surface as `Storage` at a run boundary.

Messages carry what the window prints on standard error today: firmware
accepted under the unlisted escape, a guest-requested reset, speaker trace
lines, journal maintenance warnings.

### 3.11. Determinism

- No call takes or returns host time. A core reads no clock, no random
  source and starts no thread on the execution path.
- A machine advances only inside `run`. Input and media changes take
  effect at run boundaries, at a virtual time the front end can read.
- **Replay rule.** The same `CreateRequest` (config bytes, content by
  digest, an equal session directory, options) followed by the same
  sequence of calls with the same arguments gives the same guest and the
  same pictures, sound, messages and stops. A front end that records its
  calls with `now()` and `steps()` at each boundary can replay a session.
- The contract does not promise that two different ways of slicing a span
  into runs give the same guest. Today's machine is exact on active steps
  and rounds a halted stop to a whole tick [REPO:
  `crates/machine/src/lib.rs`, `run_bounded`]; a front end that needs
  slice independence drives by `steps`.
- Sound and picture never pace execution. Host failures never reach the
  guest.

### 3.12. What stays platform-specific, behind providers

The contract is the run-time seam. Everything a platform knows beyond it
stays in **provider** code on the front-end side, which may use its own
core's crates directly (§11 Q3):

| Platform-specific today | Stays where |
|---|---|
| Config schema, validation, the closure walk | `ConfigValidator` implementation (`pc-image`) |
| Media formats and names, firmware rules and catalogue | `MediaFormats`, `FirmwareRules` implementations (`pc-image`, over `assembly`'s catalogue) |
| Machine instances: settings, pending inserts, commit, duplicate | `pc_image::machine` |
| Import of machine files, publication of sessions, templates | `pc_image::import`, `workspace`, `template` |
| Machine files with host paths, ROM folders and discovery | `machine-file`, `assembly::firmware_discovery`; the PC provider builds the machine and returns `Box<dyn Machine>` |
| Hardware choices and diagnostics for Settings and New machine | `assembly::capabilities`, `descriptors` (machine-images §6), used by the PC dialogs |
| The text grid, `--print`, console probes, scripted key stimuli, golden runners | PC crates and `pc-harness` |
| PC commands and PC dialogs | `cli-pc` (§7), the PC module of the desktop (§8) |

Three things are deliberately left out of version 0.1, each with the event
that would bring it in:

- **Validation through the core** (`Core::validate`), so a simple core
  needs no provider crate. Add when the NES provider's `ConfigValidator`
  proves to be a thin wrapper around its core (§6).
- **Settings as data** (the `describe`, `choices`, `validate_draft`,
  `preview` calls of machine-images §6). Add when a second platform needs
  a settings dialog.
- **Storage handed in as objects** instead of a session directory, so a
  core does no host I/O. Add with layered disk images or a build without a
  file system (§11 Q4).

## 4. The PC core on the contract

A new crate, working name **`unemu-pc-core`** (`crates/pc-core`), on the
emulation side. It depends on `unemu-core-api`, `assembly`, `machine`,
`cpu`, `device`, `devices`, `video` and `audio`, and on nothing from the
store.

```rust
/// The PC core's entry in a front end's list of cores.
pub struct PcCore;

/// A running PC behind the contract.
pub struct PcMachine { /* an AssembledMachine and the host-side encoders */ }

impl PcMachine {
    /// Wrap a machine the caller assembled. The PC provider uses this for
    /// machine files and for every start until `PcCore::create` can read a
    /// stored config itself; tests use it with presets.
    pub fn from_assembled(
        config: &assembly::MachineConfig,
        assembled: assembly::AssembledMachine,
    ) -> Result<Self, unemu_core_api::Error>;

    /// The assembled machine, for PC code that needs more than the
    /// contract: the text grid, device state in tests.
    pub fn assembled(&mut self) -> &mut assembly::AssembledMachine;
}

impl unemu_core_api::Machine for PcMachine { /* … */ }
impl unemu_core_api::Core for PcCore { /* … */ }
```

How each call maps onto today's code, and what moves:

| Contract | PC implementation | Moves from |
|---|---|---|
| `description().clock` | `device::clock::PIT_HZ` times `TICKS_PER_PIT_CLOCK` | `window/pacing.rs` (the constant use) |
| `now`, `steps` | `machine.clock()`, `machine.virtual_time()` | |
| `run` | deadline to cap (`run_cap`), `check_persistence`, inject queued input, `run_bounded`, guest reset (`before_cpu_reset`, `reset_cpu`), sample drive activity | `window/main.rs` `advance_with_slice_clock`, `window/pacing.rs` `run_cap` |
| `frame` | `capture_text(display)` and the rasteriser with the bundled fonts | `window/lib.rs` `WindowSink`, `raster.rs`, `font.rs`, the font assets |
| `drain_audio`, `set_outputs` | `capture_audio` into an internal sink | the sink half of `window/host_audio.rs` |
| `input` `Key` | HID usage to set-1 scan codes, typematic, NumLock-aware wrappers, the 8042 queue | `window/keyboard.rs` minus its winit mapping |
| `input` pointer | count carry and button queue, `inject_mouse` | the device half of `window/mouse.rs` |
| `slot_states` | drives of the config, `inserted_medium`, `floppy_name`, `drive_active`, `activity` | `window/main.rs` `drive_views`, `sample_drive_busy`; `media_dialog::FloppySlot::from_config` |
| `add_medium`, `change_medium` | `append_floppy_verified`, `change_floppy` | |
| `reset(Soft)` | `reset_halted` | `window/main.rs` |
| `finish` | `finish_persistence`, `maintenance_failure` | `window/main.rs`, `cli/lib.rs` |
| `drain_messages` | unlisted firmware, speaker trace, reset lines | `window/main.rs` |
| `notices` | `devices::OPL3_NOTICE` | `cli/about.rs` |

`PcCore::create` (config document, content, session directory) needs the
PC config codec: the half of `pc-image` that decodes a stored config
through `machine-file` lowering and mounts bases and journals
(`provider.rs`, `encode.rs`, `defaults.rs`, `disk.rs`, `zero.rs`). That
code uses store types today, so it comes in two stages:

1. **Stage A.** `PcMachine` and its tests land. The desktop session drives
   `dyn Machine`. Every start goes through the PC provider as today
   (`pc_image::machine::prepare`, then `assemble_with_verified_bases`),
   wrapped by `PcMachine::from_assembled`. `PcCore::create` answers
   `Unsupported` and the PC's `PlatformDescription` lists no
   `config_types`, so no front end offers it.
2. **Stage B**, before the repositories separate. The config codec moves
   below the contract, over `ContentSource` instead of `Store`, and
   `PcCore::create` becomes the one start path for stored images.
   `pc-image` keeps the store operations and calls the same codec for
   validation.

Consequences to plan for:

- **Licences.** The MDA and VGA fonts are CC BY-SA 4.0 and move with the
  rasteriser. The crate-scoped exception in `deny.toml`, the licence field
  and `scripts/third-party-licenses.sh` name `window` today and must name
  the adapter crate for those assets (§11 Q5).
- **Byte-identical input.** The tests that pin the scan-code stream, the
  typematic repeats and the mouse packets move with the encoders and must
  pass unchanged; the HID mapping gets a table test against today's winit
  mapping.
- **Goldens do not use the contract.** They keep `assembly` and
  `pc-harness`. The adapter gets its own tests: one session through
  `dyn Machine` compared with the same session through `AssembledMachine`.

## 5. The front end on the contract

**The desktop session** (`Presenter`) becomes a platform-neutral crate that
holds a `Box<dyn Machine>`. Its loop, in today's terms:

```rust
fn advance(&mut self, elapsed: Duration) -> Result<(), Error> {
    let target = self.pacing.target(elapsed, self.machine.now());
    while self.running && self.below(target) && self.pacing.slice_open(/* … */) {
        for (slot, action) in self.media_events.drain(..) {
            self.machine.change_medium(slot, action)?;
        }
        let until = self.pacing.chunk_target(self.machine.now(), target);
        match self.machine.run(RunLimit::until(until))? {
            Stop::Halted => { self.running = false; self.halted = true; }
            Stop::PoweredOff => self.running = false,
            _ => {}
        }
        self.machine.drain_audio(&mut self.audio)?;
        self.machine.drain_messages(&mut self.messages);
    }
    let frame = self.machine.frame(self.display)?;
    /* upload when frame.serial changed; update drive lights from slot_states() */
    Ok(())
}
```

Pacing, speed, the paint cadence, the host audio ring, mute, pointer
capture and host shortcuts stay as they are; `pacing` takes the tick rate
from the description instead of `PIT_HZ`. The host clock is still read
only here.

**Starting.** The front end asks the platform's provider for a machine.
For a core whose `create` reads stored configs, the neutral path is:
verify the closure on the worker thread (digests from
`ConfigValidator::validate`), then `core.create(&request)` with a
`ContentSource` over the store's receipts and the machine's workspace as
the session directory. For the PC in stage A the provider returns the
`PcMachine` it built itself.

**The command line** drives no machine through the contract at first: the
commands that run a machine (`run`, `image run`, `image machine run`,
`--boot`, `--bios`) are PC provider commands with PC run policies
(`RunPolicy`, idle limits, `--print`). When a second platform has a
runnable image, `image run` and `image machine run` move to `cli-core` as
`core.create`, `run(RunLimit::steps(max))`, `finish`.

**The lists of cores and providers** are data the two binaries pass down.
No neutral crate names a platform.

## 6. Sanity check: a NES core

Not a design of unemunes; a check that the contract does not assume a PC.

| Need of a NES core | In the contract |
|---|---|
| Create from a config (region, expansion) and a cartridge image by digest | `create`: config document plus `ContentSource`. The cartridge's mapper comes from the image's own header, read by the core |
| Battery-backed cartridge memory | the session directory |
| Run frame by frame | `RunLimit::stop_on_frame`, `Stop::Frame` |
| 256×240 picture with 8:7 pixels | `Frame` with `aspect` |
| Sound at its own rate | `AudioDescription::rate`; the front end already resamples |
| Two controllers | two `InputKind::Pad` ports, `InputEvent::Pad` |
| Reset button, power | `ResetKind::Soft`, `Hard` |
| Cartridge port | a `SlotKind::Cartridge` slot with `accepts_changes` false while running |
| Disk system sides | a slot catalogue and `MediaAction::Next` |
| Saved state from the first day | `save_state`, `load_state`, `state_identity` |
| Exact master clock (236 250 000 / 11 Hz, NTSC) | `Rate` |
| No firmware catalogue, no settings dialog | nothing asked of it |

Gaps this found, none blocking 0.1:

- **A light gun** needs an absolute pointer position on the picture: one
  more `InputEvent` variant and `InputKind`, a minor addition.
- **Validation.** A NES provider's `ConfigValidator` would only parse the
  config and name the cartridge digest. If that holds, `Core::validate`
  (§3.12) lets a simple platform ship with no provider crate at all.
- **Rewind and run-ahead** want cheap repeated saves; `save_state` to a
  `Write` in memory serves, and a faster path can be added later.

## 7. The CLI split

### 7.1. Target crates

| Crate (directory) | Layer | Contents |
|---|---|---|
| `cli-core` (`crates/cli-core`) | neutral | The command model and top-level parser; usage assembly; exit-code policy. Commands: `image ls`, `image sweep`, `image compress`, `image workspace ls`, `image workspace discard`, `image media ls`, `show`, `image firmware ls`, `show`, `image push`, `pull`, `remote ls`, `login`, `registry set`, `recipe check`, `show`, `media generations`, `verify`, `rollback`, `commit`. The `CliPlatform` trait (§7.2). Depends on `image-store`, `image-registry`, `media`, `recipe`, `anyhow`, `toml` |
| `cli-pc` (`crates/cli-pc`) | PC | The PC's `CliPlatform`. Commands: `--boot`, `--bios`, `machine check`, `run --machine`, `image import`, `image run`, `image publish`, `image workspace abandon-publication`, `image machine *`, `image media import`, `image firmware import`, `edit`, `image template build`, `media create`. Depends on `cli-core`, `pc-image`, `machine-file`, `assembly`, `pc-harness`, and the emulation crates the run commands need |
| `upcli` (`crates/upcli`) | application | `main.rs`: host paths, the list of platforms, `cli_core::run`. Keeps the binary name `upcli` |
| `pc-harness` (`crates/pc-harness`) | PC | Today's `cli/src/lib.rs`: runners, guest programs, key scripts, `StopReason`, and `tests/support`, a directory the other crates' tests include with `#[path]` (a `test-support` feature would need `assembly` to depend on a `pc` crate). Its `tests/` are the 33 emulation golden binaries (§2.4 plus `user_media_volumes`). Depends on `assembly` and the emulation crates and, as a test dependency for the eleven goldens that load machine files, on `machine-file` |
| `platforms` (`crates/platforms`) | application | The one place that names every compiled-in platform, for both binaries: the provider list (`with_image_providers`), the program's licence notice with each core's notices. In the front-end repository this is the crate whose manifest pins the cores |

Moves into existing libraries, so that `window` needs nothing from a CLI
crate:

| Today in `cli` | Goes to | Why there |
|---|---|---|
| `registry_client`, `registry_probe_client` | `image-registry` (`accounts`) | They use registry types only |
| `push_plan`, `guard_marks` | `image-registry::push` takes the catalogue-restricted digests as an argument (`LocalMarks` already has a catalogue mark); `pc-image` states them | Splits the neutral plan from the PC knowledge |
| `preferences::path` | `image_store::location` | Beside `default_store_root`, the other host location |
| `create` (request model, `parse`, `execute`) | `pc-image` | PC diskette formats and CHS policy; a library both front ends call |
| `finish_persistence` | `cli-pc` (and gone with `Machine::finish`) | |

### 7.2. The provider seam of the command line

```rust
/// One platform's part of the command line.
pub trait CliPlatform {
    /// The platform id, in its current spelling.
    fn platform(&self) -> &str;
    /// Validators for every artifact type of the platform, for transfer and
    /// listing.
    fn validators(&self) -> Vec<&dyn ConfigValidator>;
    /// Digests of a manifest's blobs the platform knows must not be
    /// redistributed, for the push guard.
    fn restricted(&self, store: &Store, manifest: &Manifest) -> anyhow::Result<Vec<Digest>>;
    /// Publish `target` as a compressed copy of `source`, with the local
    /// records the platform keeps beside an image.
    fn compress(&self, store: &Store, source: &str, target: &str) -> anyhow::Result<Descriptor>;
    /// The images this platform's workspaces and machines pin, which a
    /// sweep of the materialized cache keeps.
    fn pinned(&self, store: &Store) -> anyhow::Result<Vec<Digest>>;
    /// The usage lines of the platform's own commands.
    fn usage(&self) -> &str;
    /// Parse the platform's own command, or `None` when the words are not
    /// this platform's.
    fn parse(&self, args: &[OsString]) -> Option<Result<Box<dyn PlatformCommand>, &'static str>>;
}

/// A parsed platform command.
pub trait PlatformCommand {
    fn execute(self: Box<Self>, io: &mut Io<'_>, host: &HostSources) -> anyhow::Result<u8>;
}
```

The hooks are the places §2.3 found: the provider list, the catalogue
marks of the push guard, `pc_image::compress_image` (which copies the PC
import record to the compressed image) and
`pc_image::workspace::sweep_materialized` (which reads what PC sessions
pin).

`cli_core::run(args, io, host, platforms: &[&dyn CliPlatform])` parses the
neutral words first, in today's order, then offers the rest to each
platform. With one platform every command keeps its exact words, output
and exit code. How a command names its platform once there are two (by the
artifact it names, where it names one) is decided with the second
platform, not here.

### 7.3. Dependency rules

Each workspace package declares its layer in its manifest:

```toml
[package.metadata.unemu]
layer = "neutral"   # contract | neutral | emulation | pc | application
```

| Layer | Crates (after the split) | May depend on |
|---|---|---|
| `contract` | `unemu-core-api` | nothing at all, also outside the workspace |
| `neutral` | `image-store`, `image-registry`, `media`, `cli-core`, later the recipe crate and the neutral window crates | `contract`, `neutral` |
| `emulation` | `device`, `bus`, `cpu`, `video`, `audio`, `devices`, `chipset`, `machine`, `testkit`, `assembly`, `unemu-pc-core` | `contract`, `emulation` |
| `pc` | `machine-file`, `pc-image`, `pc-harness`, `cli-pc`, later the PC window module | `contract`, `neutral`, `emulation`, `pc` |
| `application` | `upcli`, `platforms`, `unemupc` | anything |

Two more rules:

- No window crate depends on a CLI crate, and no CLI crate on a window
  crate.
- The rules cover normal, build **and** test dependencies, because a
  neutral crate whose tests need a PC crate is rebuilt by every emulator
  change and cannot leave the repository without it. Test fixtures that
  need a real platform live in a `pc` or application crate.

### 7.4. The boundary check

`scripts/crate-boundaries.sh`, bash with `cargo metadata` and `jq` (both
in the devShell; no new crate):

1. Reads `cargo metadata --format-version 1 --no-deps` once: the
   dependencies each workspace manifest declares, optional and
   target-specific ones included, not the resolved closure.
2. Fails when a workspace package has no `layer`, so a new crate cannot
   dodge the rules.
3. For every workspace package, walks those declared dependencies inside
   the workspace (its own of every kind, then normal and build ones of the
   crates it reaches, whose test dependencies it never builds) and fails on
   the first crate on each path that the table of §7.3 forbids, printing
   the path once: `cli-core → image-registry → … → assembly`. The walk
   does not cross an allow-listed edge, so a crate depending on `assembly`
   needs no entry of its own for `media`.
4. Fails when the `contract` package has any dependency.
5. Reads `scripts/crate-boundaries.allow`, a list of known violations, one
   `from → to (kind): reason` per line. A listed edge passes; **a listed
   edge that no longer exists fails**, so the list only shrinks. It starts
   with today's one forbidden edge, `assembly → media` (`emulation` →
   `neutral`), and the test dependencies found when the layers are first
   declared. `machine-file → image-store` is `pc` → `neutral` and allowed;
   it matters only for the extraction (§9).
6. `--self-test` runs the rules over fixture metadata files, each with one
   planted violation, and expects each to fail: the same approach the
   affected-set runner takes for its own mapping.

It runs in the `guard` lane of `scripts/ci-gate.sh`, on every push and
pull request, and in the supervisor's gate. It takes about a second
(measured: about 0.1 s for the check, a few seconds with the devShell's
start).

### 7.5. Steps

Each step is one bead: it lands on `main` with the full gate, changes no
behaviour, and moves tests with the code they test, weakening none. The
expected effects are ASSUMED, derived from the PRELIMINARY figures of
§2.8, for a CLI-only change measured as `upc-x83.19` measures it.

| # | Step | Expected effect |
|---|---|---|
| 1 | **Done.** **Boundary check.** Add `layer` to every manifest, `scripts/crate-boundaries.sh` with its self-test and the allow list of today's violations; wire it into the `guard` lane. No code moves | Gate +1 s. Later steps cannot regress |
| 2 | **Done.** **`window` off `cli`.** Move the shared helpers of §2.5 to `image-registry`, `image_store::location`, `pc-image` and the new `platforms` crate (§7.1). Replace the six `cli::command::run` fixture calls in `window`'s tests with the library calls they stand for. Remove `cli` from `window`'s manifest; add the "no window crate depends on a CLI crate" rule | The largest win. A CLI-only change no longer rebuilds or tests the desktop: from about 154 s to about 30–45 s. Clean build unchanged |
| 3 | **Done.** **Runners and goldens out of `cli`.** Create `pc-harness` from `cli/src/lib.rs` and move the 32 golden binaries and `tests/support` with it. `cli` depends on `pc-harness` for `--boot`, `--bios` and `run`. The seven goldens that need a front end stay with `cli` and stay selected by tier. As landed: 33 binaries moved (`user_media_volumes` was added since §2.4) and eight front-end goldens stay (`boot_sector_golden` too); `tests/support` stays a directory the other crates include by `#[path]`, because a `test-support` feature would make `assembly` (layer `emulation`) depend on a `pc` crate | A CLI-only change links 21 test binaries instead of 53: about 10–20 s less, and about 60–70 s less at the tier that includes the slow goldens, by construction and not by tier marks. MEASURED with the affected-test runner: 22 test binaries instead of 55 (24 instead of 57 with the unit tests), 7 ignored tests run by name instead of 32, 57 s instead of 95 s on a loaded machine |
| 4 | **Done.** **The provider seam, inside `cli`.** Add `CliPlatform` and `PlatformCommand`; route every PC command through one PC implementation; the neutral commands take providers as arguments. Still one crate. A test asserts the usage text and the parse of every documented command line are unchanged. As landed: the neutral command line is `cli::cli_core` (the traits, `HostSources`, `Io`, `parse`, `usage`, `run`, and the neutral commands with their tests), the PC's is `cli::cli_pc` (`PcPlatform`, built by `with_platform` from the provider list of `platforms`, and every PC command), and `cli::command` is the program that composes them; `upcli`'s `main` passes the platform list. Deviations from the sketch above: `restricted` returns `image_store::Result`, the type `push::plan_with_catalogue` takes; two more hooks, `recipes` (the recipe provider of `recipe check` and `show`) and `medium_format` (the format names of `image media show`); `usage` takes a `UsageSlot`, because the neutral and the PC lines interleave and the text keeps its order byte for byte; `PlatformCommand` is `Debug`, for the parse pin; `--version` and `--about` stay with the program (`cli::command`), which owns the notice; the platforms are offered a command line after `image` and before the journal commands, since a PC option such as `--rom-dir` on a `media` line was always the PC's refusal; a command that names one artifact (`image compress`, `media show`, `firmware show`) asks the first platform, the choice among several being left to the second platform. Neutrality inside `cli` is a source test (`tests/neutral_boundary.rs`): with comments and literals blanked, no file under `src/cli_core` starts a path or a `use` tree (also a leading `::` one, or one in a `use {…}` group over several lines) at an emulation, PC or application crate, names a `crate::` path outside `cli_core`, climbs out of `cli_core` with `super` (by the file's module depth and its inline modules), or uses `extern crate`, `#[path]`, `include!` or `#[macro_export]`. `tests/command_line_pin.rs` pins the usage texts and the parse of 215 lines: every usage form, every `upcli` example of commands.md (a copy kept with the test), and the refused lines whose words the two parts split; each of the 36 refusals reads as the binary before the split printed it (MEASURED). MEASURED: the binary's output, exit codes and written files are byte-identical before and after over 319 invocations | None in time. Makes step 5 a move |
| 5 | **Done.** **`cli-core`.** Move the neutral modules and their tests (`media_tools`, `media_fifo`, the neutral halves of the image command tests, with the store's synthetic providers). The rest of `cli` keeps working through re-exports for one step. As landed, together with step 6 in one change: `crates/cli-core` (layer `neutral`) is what was `cli::cli_core`, with its unit tests (the toy platform included); `media_tools` and `media_fifo` moved and run the neutral command line with no platform at all (`cli_core::run(…, &[])`). Deviations: no re-exports, since step 6 landed with it; the image command tests were not split, because every one of them builds its images through the PC import and the shared PC fixtures, which §7.3 keeps in a `pc` or application crate: they moved whole to `cli-pc`, and the neutral image commands keep their unit tests with the toy platform in `cli-core`; `media_tools`'s `usage_errors_are_two_and_boot_still_runs` runs `--boot`, so it moved to `cli-pc`, in a test binary of the same name. `tests/neutral_boundary.rs` shrank to what Cargo cannot see: no file under `cli-core/src` loads another file with `#[path]` or `include!` (`include_str!`, `include_bytes!`); the crate names, `crate::` paths and `super` climbs it checked before are now the manifest's (the boundary check) and the compiler's | A neutral command change rebuilds `cli-core` and its dependents; a PC command change no longer runs the neutral tests. A few seconds. MEASURED: see step 6 |
| 6 | **Done.** **`cli-pc` and `upcli`.** Rename what is left to `cli-pc`; create the `upcli` binary crate. The four test binaries that run `upcli` (`firmware_catalogue`, `timing_adoption`, `machine_commands`, `image_registry_commands`) move to the `upcli` package, because Cargo gives the binary's path only to the package that builds it. Update the package names in CI, the release workflow, `scripts/third-party-licenses.sh`, `scripts/ci-gate.sh`, `commands.md` and `architecture.md`. As landed: `crates/cli-pc` (layer `pc`) is what was `cli::cli_pc`, plus `cli_pc::command`, the command line with the PC platform alone (`parse`, `run`, `run_with_firmware_sources`), which the PC command tests and the front-end goldens drive (none of them passes `--version` or `--about`); `crates/upcli` (package `upcli`, layer `application`) builds the binary and has a small library, `about` and `command` (the composed command line with the program's notices), for `main`, its unit tests and `command_line_pin`, which moved there with its pinned files. Deviations: `cli-pc` may not depend on `platforms` (application), so the PC's provider list moved from `platforms::with_image_providers` to `pc_image::with_providers`, which `platforms` now calls, and `cli-pc` asks `pc_image::firmware::restricted` directly; `image_template_commands` plans its push with `image_registry::push::plan_with_catalogue` and the PC catalogue's restrictions, the body of `platforms::push_plan`. The affected-test runner treats all of `crates/cli-core/src/` as command line the front-end goldens drive, not only the dispatch, because `ide_journal_golden` runs the journal tools; in one crate those goldens ran by name on any CLI change. `scripts/bead-gate.sh` and `ci-gate.sh` lint `-p upcli --bin upcli`; the release workflow builds `-p upcli -p window`; the licence script starts from `upcli` and `window` (the same 352 crates, MEASURED). MEASURED: every test keeps its name (3929 before and after; `cli::cli_core::X` and `cli::cli_pc::X` became `cli-core`'s and `cli-pc`'s `X`); the binary's output, exit codes and written files over the 319 invocations of step 4 are byte-identical but for the firmware index, which records the ROM file's times and inode and differs between two runs of the same binary too | None in time. The neutral command line is PC-free by the boundary check (MEASURED: a planted `cli-core → pc-image` or `cli-pc → platforms` dependency fails it). MEASURED with the affected-test runner on a shared host, a one-line change to the journal tools of the neutral command line: before the split it ran the `cli` crate's 196 fast tests and its 7 front-end goldens by name in a second step, 77 s in all; after it, the goldens tier of `cli-core`, `cli-pc` and `upcli`, the same 203 tests in one run, 48 s |

After step 3 the criterion holds with a wide margin and without the
desktop: build and test of a CLI-only change in well under a minute, and a
whole bead gate (format, clippy on the affected crates, tests, guard) in
about two minutes (ASSUMED). Steps 4 to 6 are for the boundary, not for
time.

Order: 2 before 3 because it is the larger win and unblocks the window
split; 3 before 4 to 6 because it shrinks the crate they reshape.

## 8. The window split

Shorter, because it follows the same rules. Today a window-only change
already takes about 134 s with the affected-set runner (PRELIMINARY), so
the aim here is that a change to one part of the desktop builds and tests
only that part, and that the neutral parts are provably PC-free.

Target crates:

| Crate | Layer | From today's modules |
|---|---|---|
| `ui-kit` | neutral | The skin and widget helpers now in `shell.rs` (`skin_visuals`, `configure_context`, `gradient_title`, `escape_pressed`, `bevel`, `bevel_ring`, the marks), `shell_icons`, `logo`, `file_dialog`, `text_clipboard`, `library_view/pane` |
| `machine-view` | neutral | The session over `dyn Machine` (`Presenter`), `pacing`, `host_audio`, `system_keys`, `macos_keys`, the key mapping to `Key`, pointer capture, drive lights. PC encoders and the rasteriser go to `unemu-pc-core` (§4) |
| `library-ui` | neutral, with PC views split off | The library model and worker (`library_worker`, `registries_model`, `registry_transfer`, `remote_browse`, `remote_pull`, `save_image`), then the views (`library_view` with `machines`, `install_media`, `remotes`, `registry_transfer_view`) |
| `media-ui` | neutral, with PC pieces split off | `media_picker`, `media_dialog`, `media_create`, `created_media`, `catalogue_menu`, `drive_chips` |
| `pc-ui` | PC | `new_machine`, `new_machine_view`, `settings_model`, `settings_sheet`, `image_settings`, `firmware_choices`, `layers_dialog`, `library_view/hardware`, `library_view/firmware`, `firmware_metadata` |
| `unemupc` | application | `shell`, `shell_model`, `app_menu`, `macos_menu`, `preferences`, `preferences_sheet`, `main` |

§8.1 plans steps 3 to 6 move by move. It changes the last four rows of
this table, for reasons counted there, and moves the PC dialogs and the
library's frame together in step 6; the maintainer accepted it on
2026-11-18 (§8.1.11), so §8.1.3's crates replace the last four rows above.

Order, each a series of small beads under the same rules as §7.5:

1. **Done.** **`ui-kit`.** Almost every arrow into `shell` is one of six helpers
   (§2.5); moving them out cuts most module cycles. No time win; it is
   the prerequisite for every later step. As landed: the six helpers,
   `window_title` (it shares the title strip) and the eject and speaker
   marks form `ui_kit::skin`; `shell_icons`, `logo`, `file_dialog`,
   `text_clipboard` and the layout half of `library_view/pane` move whole,
   with their tests. The DejaVu symbol font and the egui font notices move
   with `configure_context`; the kit reads the brand logo from
   `assets/brand/`. Staying in `window`: `machine_menu_mark` (it maps the
   shell's own commands), `outline_focused_field` and `toolbar_button`
   (not in the list), the skin tests of `shell.rs` (they share the mock-up
   tokens and contrast helpers with the focus and toolbar tests there),
   the PC firmware parts of the pane (the Firmware section, the newer
   version hint and the short digest, which read the firmware views; the
   window's `library_view/pane` re-exports the kit's pane for the library
   views) and the desktop-wide glyph and mark tests, in a test-only
   `shell_icons` module, because they read the window's sources and menu
   marks. Test names are unchanged apart from the crate (3902 tests).
2. **Done.** **The contract, `unemu-pc-core` (stage A) and `machine-view`.** The
   session has few inbound arrows (the shell and the media dialogs), so it
   comes out early, and it is where the contract first carries real
   traffic. Its tests go with it: the sound-level tests (44 s of test
   time), the host-audio and session tests. A library change stops
   running them, and a session change runs only them: about 80 s of
   desktop tests down to about 15–20 s. As landed:
   - `unemu-core-api` (`crates/core-api`, layer `contract`, no dependency of
     any kind) holds §3 as written, with `CONTRACT` 0.1 and `drives`, unit
     tests of the version rule and a scripted core and machine driven only
     as `&dyn Core` and `Box<dyn Machine>`.
   - `unemu-pc-core` (`crates/pc-core`, layer `emulation`) holds
     `PcMachine::from_assembled`, `PcMachine::assembled` and `PcCore`, whose
     platform lists no config type and whose `create` answers
     `Unsupported`. From `window` came the set-1 encoder with the 8042
     back-pressure and the reset chord (`keyboard`, now keyed by HID
     usage), the device half of the mouse (`mouse::PcMouse`: button
     transitions, coalescing, `inject_mouse` back-pressure), `run_cap`
     (`pacing`), the rasteriser, `font`, the CC BY-SA fonts with their
     notice and `FONT_NOTICE`, the sink (`RasterSink`, formerly
     `WindowSink`) and drive-activity sampling, with their tests and the
     `presentation` test binary. Its own tests compare a session through
     `dyn Machine` with the same keys and caps on the assembled machine.
   - `machine-view` (`crates/machine-view`, layer `neutral`, counted as a
     window crate by the boundary check) holds `Session`, the former
     `Presenter` loop over the contract, `pacing` (now at the rate the
     machine describes), `host_audio` (fed by `drain_audio`),
     `system_keys`, `macos_keys`, `keys` (winit key code to HID usage) and
     `pointer` (capture, release chord, swallowed click, count carry). Its
     tests: the host-audio ring, pacing, system-key and macOS-key tests,
     the key map, and session tests against a scripted machine.
   - `window` keeps a `Presenter` that is a `Session<PcMachine>` plus the
     PC catalogue model (the machine config and the floppy slots) that the
     media dialogs, catalogue menus and drive chips edit, and the hold of
     the status-bar drive lights, which belongs to the drive chips (step
     5). All 3902 test names are unchanged apart from the crate (47 tests
     moved to `machine-view`, 39 to `unemu-pc-core`), and 26 tests were
     added, among them a table test that composes the key map with the
     encoder and gives the window's former winit table key for key.

   Deviations from §3 to §5, each forced by keeping the step free of
   behaviour change or by the layer rules:
   - **Additions to the contract**, all additive: named HID usages as
     associated constants (`Key::A`, `Key::ARROW_UP`, …), so the key map
     and the encoder cannot disagree on a number; `new`/`with_…`
     constructors (`const` where a core builds a static description),
     `Features::NONE`, `Outputs::NONE`, `ContractVersion::new` and its
     `Display`; `Rate::new` reduces to lowest terms; the usual derives
     (`Clone`, `Debug`, `PartialEq`) on the data types.
   - **`Session<M: Machine + ?Sized = dyn Machine>`** instead of a session
     that can hold only `Box<dyn Machine>`. The session's code calls the
     contract alone, and its tests drive a scripted machine, but the
     window names `Session<PcMachine>`: the media dialogs, catalogue menus
     and drive chips still append floppies and read the catalogue through
     `PcMachine::assembled` until step 5 moves them behind `add_medium`.
     For the same reason `PcMachine::add_medium` answers `Unsupported` in
     stage A, and its slot catalogues carry each floppy's name and write
     kind (read-only, discarded or kept, as the controller holds the
     medium) but no digest, size or format yet: those stay in the
     provider's machine config.
   - **The PC core names its platform id** (`ibm-pc`) in its
     `PlatformDescription`; the platform-id scan of `image-store`
     (`tests/platform_ids.rs`) lists `pc-core/src/core.rs` among the files
     that may spell it out.
   - **PC calls beyond §4**: `assembled_ref`, `keyboard`, `ctrl_alt_del`
     (the keyboard-owned reset chord reaches an inhibited keyboard whole;
     three key events would not), `inserted_floppy`, `drive_activity`, the
     reset counters and summary, `RASTER_SCALE` for integer scaling, and
     test hooks behind a `test-support` feature (the delivered-input logs
     the replay tests read).
   - **What a core may answer** (contract text of §3.5 and §3.9 changed to
     match, the contract being 0.1 and unpublished): `Stop::Deadline` may
     come back with `now()` short of `until` by less than one step (the
     PC's cap rounds halted time down; the session runs again while it is
     short); a `Storage` failure while the guest halts, and a failure of
     `reset` itself, leave the status `Halted` rather than `Failed`, which
     the session relies on to still finish a halted machine's media.
   - **`Features::frame_stops`**, a new field: `RunLimit::stop_on_frame`
     is honoured only by a machine that lists it and refused with
     `Unsupported` otherwise. The PC does not list it.
   - **Where a run ends.** `PcMachine::run` ends at the retired-instruction
     cap a deadline gives, as each window chunk did; when halted time leaves
     `now()` a fraction of a tick short of `until`, it still answers
     `Stop::Deadline` and the session runs the next chunk, as the window
     loop always did. Exhausted virtual time answers `Stop::PoweredOff`. A
     guest-requested reset happens inside the run, which then continues to
     the same deadline, so after a guest reset the chunk ends where it was
     going to end rather than a millisecond after the reset.
   - **Media keys** change the first and the second removable slot of the
     description, for the PC A: and B:; a machine with a B: but no A: would
     now change B: with the unshifted chord.
   - **Pointer release** with several guest buttons held sends one packet
     per released button where one packet released them all; focus loss of
     a running session still releases everything at once
     (`InputEvent::ReleaseAll`).
   - **Tests that need a real PC stay in `window`**: the three sound-level
     tests, the six host-audio tests that run a PC, the session tests of
     `main.rs` and the mouse tests, which now join `pointer` with
     `PcMouse` in a test shim. A `neutral` crate may not have a PC crate
     among its test dependencies (§7.3), so `machine-view` cannot run a PC.
     `caps_respect_configured_rates_and_saturate_without_overflow` moved
     to `unemu-pc-core` with `run_cap`, and its one `frame_target` check
     joined `overload_drops_debt_once_and_an_ahead_guest_does_no_work`.
   - **Sound at 48 kHz only**: the session plays what `drain_audio` returns
     as 48 kHz stereo, the rate the PC describes, and resamples only to the
     host device's rate. A machine that describes another rate gets no host
     device and one `audio disabled` warning; resampling such a source
     comes with the core that has one.
   - **Pictures** are copied out of the machine only when `Frame::serial`
     moved, and the PC moves it only when the rasterised pixels changed:
     MEASURED over 30 paints of an idle screen, 30 copies before, 1 after
     (`an_unchanged_screen_is_copied_once_across_paints`).
   - **The lights' hold** (`drive_chips::DriveLights`) stays with the drive
     chips in `window`; `machine-view` takes the activity
     (`Session::take_slot_activity`) and the window feeds its lights.

   The expected time effect did not come with this step, because the tests
   that need a real PC stay in `window` and `window` depends on
   `machine-view`. MEASURED with the affected-test runner on a loaded
   development host: a change to `crates/machine-view/src/session.rs` runs
   `machine-view` and `window` (787 tests, one ignored test by name) in
   114 s; a change to `crates/window/src/shell_model.rs` runs `window` (731
   tests, three ignored tests by name) in 114 s. What moved out is the 47
   fast tests that need no machine, so a window change no longer builds or
   runs them; the 44 s of sound-level tests move only with a PC-side crate
   of their own, or once the media dialogs leave the window.
3. **The library model and worker**, without egui: types the views and
   the worker share move down first, which cuts the
   `library_worker` ↔ `library_view` cycle.
4. **The library and Remotes views.** The largest part: with the model
   and worker about 150 s of the 294 s of per-test time. After this a
   change to the shell or to the session no longer runs them.
5. **The media picker and dialogs.**
6. **The PC views** into `pc-ui`. What remains in `unemupc` is the shell
   and the composition.

`library-ui` and `media-ui` reach PC knowledge through provider objects
passed in by the application, as the command line does. A view that only
one platform has stays whole in `pc-ui`; nothing is generalised before a
second platform needs it.

The contract crate and the adapter can start as soon as this design is
approved, in parallel with §7: they add crates and move nothing until
`machine-view` exists.

### 8.1. Steps 3 to 6 in detail

Status: **Accepted** by the maintainer on 2026-11-18 (planning step
`upc-dfs.25`), with the recommended answers of §8.1.11: Q1 (a) and the
crate names of Q2. It keeps the rules of §7.3 and of §8 and changes the
last four rows of §8's table, for the reasons counted in §8.1.1 and
§8.1.2. The provider list already moved to `pc-image` with the CLI split
(`pc_image::with_providers`, step 6 of §7.5), so move 3.4 only uses it.

Markers in this section: **[REPO]** = read on `main` at `705b1af`, with
the path; **COUNTED** = counted for this section by throwaway scripts over
`crates/window/src` (a file's production part ends at its first test
module; a reference is a path that starts at a crate's name, so a `use`
that brings in several names counts once and the figures are lower
bounds); **MEASURED** = one `cargo nextest run -p window` on this commit,
four tests at once, on a loaded development host: 732 tests and 4 ignored,
75.8 s of wall time, 287.7 s when each test's own time is added up. 57 of
them failed at once because one firmware fixture was not fetched in that
checkout (38 of the 57 are tests of the PC dialogs, 11 of the frame, 8
elsewhere), so every figure marked `*` is a lower bound; **ASSUMED** = not
verified.

#### 8.1.1. Inventory of the library, media and PC modules

"PC references" are references in production code to `assembly`,
`pc_image`, `machine_file`, `devices`, `device`, `machine`, `video`,
`audio`, `cpu`, `unemu_pc_core` or `platforms`. `media`, `image_store`,
`image_registry`, `unemu_core_api`, `machine_view` and `ui_kit` are
neutral or the contract and are not counted. "View lines" are the lines of
the top-level items that name `egui`, `eframe`, `ui_kit` or `winit`; a
file with none builds without a toolkit. Seconds are each module's tests
added up (MEASURED); the lines and references are COUNTED.

| Group | Module | Lines with tests | Production | View lines | PC references | Tests | Seconds |
|---|---|---:|---:|---:|---:|---:|---:|
| Registries and Remotes | `registries_model` | 1 978 | 1 145 | 0 | 0 | 16 | 1.6 |
| | `registry_transfer` | 2 889 | 1 548 | 0 | 7 | 18 | 15.2 |
| | `remote_browse` | 3 993 | 1 939 | 0 | 6 | 28 | 9.8 |
| | `remote_pull` with `remote_pull/tests` | 1 790 | 947 | 0 | 3 | 9 | 0.6 |
| | `save_image` | 879 | 629 | 158 | 0 | 11 | 0.7 |
| | `registry_transfer_view` | 1 078 | 789 | 763 | 0 | 1 | 0.6 |
| | `library_view/remotes` with its `tests` | 4 577 | 1 803 | 1 046 | 2 | 39 | 5.3 |
| | `library_view/remotes/details` | 896 | 896 | 705 | 0 | 0 | 0 |
| | **sum** | 18 080 | 9 696 | 2 672 | **18** | 122 | 33.8 |
| Local library | `library_worker` | 4 632 | 2 207 | (6 uses of `egui::Context`) | 124 | 30 | 44.4 |
| | `library_view` (the frame) | 6 320 | 3 998 | 3 093 | 10 | 37 | 48.0 `*` |
| | `library_view/layout_tests` | 2 517 | 0 | | | 27 | 40.1 |
| | `library_view/machines` | 2 499 | 1 962 | 1 249 | 14 | 12 | 1.2 |
| | `library_view/firmware` | 2 871 | 1 806 | 1 065 | 13 | 14 | 9.6 |
| | `library_view/firmware_metadata` | 2 585 | 1 521 | 847 | 10 | 9 | 6.9 |
| | `library_view/install_media` | 2 844 | 1 665 | 929 | 15 | 16 | 12.6 |
| | `library_view/hardware` | 710 | 504 | 0 | 15 | 5 | 1.8 |
| | `library_view/pane` (the PC half; the rest is in `ui-kit`) | 135 | 135 | 109 | 0 | 0 | 0 |
| | **sum** | 25 113 | 13 798 | | **201** | 150 | 164.6 `*` |
| PC dialogs | `new_machine` | 2 295 | 1 416 | 0 | 20 | 21 | 1.2 `*` |
| | `new_machine_view` | 1 207 | 1 207 | 1 167 | 5 | 0 | 0 |
| | `settings_model` | 1 782 | 735 | 0 | 6 | 25 | 2.8 `*` |
| | `settings_sheet` | 1 461 | 1 067 | 939 | 13 | 6 | 0.4 `*` |
| | `image_settings` | 1 536 | 805 | 0 | 13 | 15 | 1.1 `*` |
| | `firmware_choices` | 511 | 447 | 0 | 9 | 2 | 0.1 |
| | `layers_dialog` | 1 919 | 957 | 450 | 8 | 14 | 4.1 `*` |
| | **sum** | 10 711 | 6 634 | | **74** | 83 | 9.7 `*` |
| Media | `media_picker` | 4 161 | 2 445 | 1 567 | 1 | 27 | 3.7 |
| | `media_dialog` | 2 559 | 561 | 0 | 10 | 22 | 8.3 |
| | `media_create` | 1 718 | 1 451 | 837 | 8 | 4 | 0.4 |
| | `created_media` | 916 | 744 | 0 | 26 | 5 | 0.7 |
| | `kept_catalogue` | 1 394 | 1 083 | 0 | 18 | 7 | 0.6 |
| | `catalogue_menu` | 1 196 | 624 | 172 | 6 | 11 | 0.9 |
| | `drive_chips` | 1 033 | 545 | 0 | 1 | 14 | 1.0 |
| | **sum** | 12 977 | 7 453 | | **70** | 90 | 15.6 |
| Shell and session | `shell`, `shell_model`, `main`, `app_menu`, `macos_menu`, `preferences`, `preferences_sheet` and the test-only `host_audio`, `mouse`, `key_table_tests`, `sound_levels_tests`, `shell_icons` | 22 349 | 8 258 | | 84 | 287 | 64.0 `*` |

`kept_catalogue` is newer than §2.5 and belongs to the media group. The
library's test fixture, `library_test_support` (471 lines), is in no row.
The preliminary figures of §2.8 put the three sound-level tests at 44 s;
here two of the three failed at once.

What each group takes from the PC crates, and why [REPO, in the named
file]:

- **Registries and Remotes: 18 references of six kinds.** None of them is
  PC behaviour; each is a name or a list a platform can state.
  - The push guard's catalogue: `registry_transfer::run`, `plan` and
    `run_push` take `catalogue: &[assembly::FirmwareEntry]` and call
    `platforms::push_plan` (5 references).
  - Two labels of blob kinds, `pc_image::CMOS` and `pc_image::DISK_ZERO`,
    in the transfer's list of blobs (2, `registry_transfer.rs`).
  - Which platform this program runs: `pc_image::PLATFORM_NAME` and
    `pc_image::is_pc` in `TagRecord::other_platform` (2,
    `remote_browse.rs`).
  - The name of a medium's format: `pc_image::media_set::format_name` (1,
    `remote_browse.rs`).
  - The order of version tags: `pc_image::template::precedence` (3 in
    `remote_browse.rs`, 2 in `library_view/remotes.rs`). It is a SemVer
    comparison over `Store::is_version_tag` and names nothing of a PC
    [REPO: `crates/pc-image/src/template.rs`].
  - Which drives take a pulled set: `assembly::FloppyGeometry` and
    `assembly::descriptors::FLOPPY_TYPES` in `remote_pull::Drive` and
    `drives_for` (3).
- **The library worker: 124 references to about 70 distinct PC items.** Of
  its 33 requests, 24 are answered by PC code, and so is each of the 16
  machine jobs; 21 of its 41 events carry a PC type (`ImageDetails`,
  `SessionTransfer`, `revision::Plan`, `machine::Settings`, the firmware
  and media-import reports, the created and kept disks). What is neutral
  in it is the loop: the thread, the channels, the cancel counter, the
  summary cache and the snapshot after each job. The nine neutral requests
  are `Query`, `Delete`, `Edit`, `Finish`, `Assembled`, `ReadHost`,
  `Shutdown`, and `Transfer` and `Browse` through the modules above. Its
  three start flows (an image, a machine, a Gaming-mode copy of a machine)
  each call PC code before, during and after the session they serve
  (`prepare_pinned`, `machine::prepare`, `prepare_copy`, `resume_copy`,
  `created_media::apply`, `kept_catalogue::restore`, `machine::confirm`).
- **The frame, `LibraryView`: 10 references of its own, PC by what it
  owns.** Of its 64 fields, 28 are typed by a PC crate or by a PC module
  of the window (the readiness of the selected image, the launched
  `SessionTransfer`, the wizard, the settings sheet, the layers dialog,
  the firmware and media dialogs, the machine rows) and 4 by the registry
  dialogs and Remotes. It is implemented in six files: `library_view.rs`,
  `machines.rs`, `firmware.rs`, `install_media.rs`, `remotes.rs` and
  `remotes/details.rs` each hold an `impl LibraryView` block.
- **The collections.** `machines` holds `pc_image::machine::Listing` rows
  with the floppy drives, created and kept disks of each machine;
  `firmware` and `firmware_metadata` are the PC firmware catalogue and
  `pc_image::firmware_import`; `install_media` is
  `pc_image::media_import` (15 of its references); `hardware` reads
  `assembly::capabilities` and has no view line at all.
- **The media group: 70 references, no neutral module.** `media_dialog`,
  `created_media`, `kept_catalogue` and `drive_chips` have no view line
  and are the PC's floppy catalogue (`assembly::FloppyMedium`,
  `WriteMode`, `ImageSource`, `pc_image::machine`,
  `pc_image::workspace::PreparedSession`). `media_picker` names a PC crate
  once, but stands on `media_create` and `created_media`.
- **The PC dialogs** are what §8 says they are; three of them
  (`new_machine`, `settings_model`, `image_settings`) and
  `firmware_choices` have no view line.

How the modules point at each other (COUNTED, production code):

- `library_worker` takes from `library_view`: `ImageRow`, `Query`,
  `snapshot`, `delete_tag`, `host_last_run`, and from its children the
  model halves of `firmware` (`ScanReport`, `Outcome`, `scan_folder`,
  `preview_set`, `import_selected`), `firmware_metadata` (`Verdict`,
  `EditSource`, `PackageDetails`, `open_edit`, `preview_edit`,
  `publish_edit`, `read_package`), `install_media` (`Pick`, `Control`,
  `Sources`, `Outcome`, `inspect`, `import`) and `machines`
  (`MachineRow`). `library_view` takes back `Request`, `Event`,
  `MachineJob`, `Worker`, `Recovery`, `Finish`, `WorkspaceKind`,
  `CheckedImage`, `regular_name`. That is the cycle of §8 step 3.
- The worker also takes `created_media` and `kept_catalogue` (22
  references), `image_settings::describe`, `layers_dialog::SourceImage`,
  `save_image::{Target, local_tags}`, `registry_transfer::run` and
  `remote_browse::run`.
- The media models form their own knot: `created_media` and
  `kept_catalogue` name `media_dialog`, and model items that live in two
  view files (`catalogue_menu::{SetDisk, set_disk, short_name}`,
  `media_picker::{SetEntry, CatalogueDisk, drive_name}`); `media_dialog`
  names `created_media::Verified`.
- `human_size` (in `library_view`) is named by ten modules.
- The library views and `catalogue_menu` push the shell's `Command` (they
  name it 36 times, tests included), and the frame lists the shell's
  `LibraryEntry`; both are defined in `shell_model`.
- The session, `Presenter` in `main.rs`, names `drive_chips` in two
  methods (`drive_views`, `observe_drive_lights`); no media or library
  module names `Presenter` outside its tests.

#### 8.1.2. What the counts decide

1. **The registry side is neutral and gets a small seam.** 9 696
   production lines with 18 references that a platform can answer as
   data. This is the part of the desktop that matches the neutral half of
   the command line (`image push`, `pull`, `remote ls`, `login`,
   `registry set`), and its seam shares four hooks with `CliPlatform`:
   the platform, `validators`, `restricted` and `medium_format`.
2. **The worker's loop is neutral, its jobs are not, and they are not
   pulled apart now.** Making the worker neutral means one of two things.
   Either its PC requests and events become values the worker cannot read
   (the pattern of `PlatformCommand`), with the three start flows turned
   inside out so that PC code calls the neutral loop while it holds a
   session; or machines, settings, firmware import, media import and
   readiness become neutral data, which is option (b) of §11 Q3, refused
   there, and the "settings as data" that §3.12 leaves out of version 0.1.
   Both are generalisation for one platform, with nothing but the PC's
   own tests (the worker's 30, the frame's 64) to check the new seam
   against. The worker stays whole and moves to the `pc` layer; §8.1.4
   says how it grows a seam when a second platform has a library.
3. **The frame is PC and moves last.** It owns the PC dialogs as fields,
   so it cannot sit below them, and a neutral frame would need every
   collection behind a trait. `machines` and `install_media`, which §8
   listed with the neutral views, follow it: their counts are 14 and 15.
4. **Nothing in the media group is neutral.** §8's `media-ui` keeps its
   name and its contents and changes its layer to `pc`.
5. **Model and view are mixed inside files**, not only between them.
   Step 3 therefore splits nine files (`save_image`, `library_view`,
   `firmware`, `firmware_metadata`, `install_media`, `machines`,
   `layers_dialog`, `catalogue_menu`, `media_picker`), and the compiler
   draws the line: an item is model when it builds without `egui` and
   `ui-kit`.
6. **Most tests of the neutral modules need a real PC.** The test modules
   of `registry_transfer`, `remote_browse`, `remote_pull`, Remotes and the
   transfer view (95 of the 122 tests of the registry side) take their
   fixtures from `pc_image` (`PcProvider`, `PLATFORM`, the synthetic
   firmware), from the worker or from the frame: the fixture of
   `registry_transfer`'s tests holds a `LibraryView`. §7.3 forbids a
   neutral crate a PC test dependency, so they stay in `pc` crates, as the
   tests that need a real PC stayed in `window` in step 2.

#### 8.1.3. Target crates

These rows replace the rows `library-ui`, `media-ui`, `pc-ui` and
`unemupc` of §8's table; `ui-kit` and `machine-view` stand as landed.

| Crate | Layer | Toolkit | From today's modules |
|---|---|---|---|
| `library-model` (new) | neutral | none | `registries_model`, `registry_transfer`, `remote_browse`, `remote_pull`, the model half of `save_image`, the listing types of `library_view` (`ImageRow`, `Listed`, `listed`, `Source`, `source`, `human_size`, `delete_tag`, `host_last_run`), and the provider seam (`DesktopPlatform`, `PlatformFacts`, §8.1.4) |
| `library-ui` | neutral | egui | The table helpers of `library_view` (`Cell`, `Columns`, `fit_columns`, `table_width`, `sort_header`, `draw_cells`, `draw_cells_with_hints`, `group_row`, `name_cell`, `Lead`), the view half of `save_image`, `registry_transfer_view`, `library_view/remotes` and `remotes/details` |
| `pc-media` (new) | pc | none | `media_dialog`, `created_media`, `kept_catalogue`, `drive_chips`, and the model items of `catalogue_menu` and `media_picker` those four name |
| `pc-library` (new) | pc | none | `library_worker`, `firmware_choices`, `hardware`, `image_settings`, the model halves of `firmware`, `firmware_metadata`, `install_media` and `machines`, `Query`, `Filter`, `Snapshot` and `snapshot`, `new_machine`, `settings_model`, the machine-file list of `shell_model` (`LibraryEntry` and what builds it), the PC's `DesktopPlatform`, and the library's test support behind a `test-support` feature |
| `media-ui` | pc | egui | `media_picker`, `media_create`, the view of `catalogue_menu` |
| `pc-ui` | pc | egui | The frame (`library_view` with the views of `machines`, `firmware`, `firmware_metadata`, `install_media`, the PC half of `pane`, `layout_tests`), `settings_sheet`, `new_machine_view`, `layers_dialog`, and the shell's command vocabulary (`Command`, `State`, `Enablement`) |
| `unemupc` (today the package `window`) | application | egui | `shell`, the rest of `shell_model`, `app_menu`, `macos_menu`, `preferences`, `preferences_sheet`, `main`; the session (`Presenter`) until `upc-dfs.25.7` gives it a crate (§8.1.10) |

What each depends on inside the workspace:

```text
library-model   image-store, image-registry, unemu-core-api
library-ui      library-model, ui-kit
pc-media        pc-image, assembly, machine-file, media, image-store
pc-library      pc-media, library-model, pc-image, assembly, machine-file,
                unemu-pc-core (the platform's description)
media-ui        pc-media, ui-kit
pc-ui           pc-library, pc-media, library-ui, library-model, ui-kit,
                machine-view (the `Speed` a command carries)
unemupc         all of the above, machine-view, unemu-pc-core, platforms
```

What changed against §8's table, and why:

| §8 | Here | Why |
|---|---|---|
| `library-ui`, neutral, holds the model, the worker and the views | Two neutral crates, `library-model` without a toolkit and `library-ui` with egui | Step 3 is "without egui". A model crate that links no toolkit keeps the registry models and their tests out of every view change, and lets the `pc` model crates below the views depend on it |
| `library_worker` in neutral `library-ui` | `pc-library`, layer `pc` | 124 PC references; 24 of 33 requests and 21 of 41 events are PC (§8.1.2, 2) |
| `library_view`, `machines`, `install_media` in neutral `library-ui` | `pc-ui` and `pc-library` | 28 of the frame's 64 fields are PC; 14 and 15 PC references (§8.1.2, 3) |
| `media-ui`, neutral with PC pieces split off | `media-ui`, layer `pc`, and its models in `pc-media` | 70 PC references; the one module that names a PC crate only once, `media_picker`, stands on two that name them 8 and 26 times. The worker and the session need the media models below them, without a toolkit |
| `drive_chips`, `created_media` in `media-ui` | `pc-media` | No view line; the worker names `created_media` and `kept_catalogue`, the session names `drive_chips` |
| `image_settings`, `firmware_choices`, `library_view/hardware`, `new_machine`, `settings_model` in `pc-ui` | `pc-library` | No view line; the worker names the first three |
| The PC views move in step 6, the library views in step 4 | The PC dialogs and the frame move together in step 6; step 4 moves the neutral views | The frame owns the dialogs as fields, so it cannot land in a crate below them |

Six crates come out of the window instead of three. Each line between
them is one the rules force: neutral against `pc` (§7.3), with a toolkit
against without (step 3), and the media views beside the library views,
which do not name each other (§8).

#### 8.1.4. The desktop's provider seam

The neutral crates ask a platform five things. The trait lives in
`library-model`, and whoever calls a neutral function hands the platforms
in, as `upcli` hands `&[&dyn CliPlatform]` to the neutral command line. In
steps 3 to 6 the caller is the PC library, whose worker runs the transfers
and whose frame owns Remotes, so the list holds the PC alone; a list the
application builds arrives with a neutral worker (the last part of this
section).

```rust
use image_store::{ConfigValidator, Digest, Manifest, Store};
use unemu_core_api::PlatformDescription;

/// One platform's part of the desktop library: what the platform-neutral
/// registry and Remotes code asks about artifacts it cannot read itself.
///
/// The desktop's counterpart of the command line's `CliPlatform`
/// (core-contract.md §7.2): `validators`, `restricted` and `medium_format`
/// mean what they mean there. The caller hands the platforms in; no
/// neutral crate names one.
pub trait DesktopPlatform {
    /// The platform as its core describes it: the id artifacts name, in its
    /// current spelling, and the name people read. Ids are compared with
    /// `image_store::platform::same` only.
    fn description(&self) -> &'static PlatformDescription;

    /// Validators for every artifact type of the platform. Push and pull
    /// pick the one whose artifact type and platform match the manifest.
    fn validators(&self) -> Vec<&dyn ConfigValidator>;

    /// Digests of a manifest's blobs the platform knows must not be
    /// redistributed, whatever the image's own marks say: the input of the
    /// push guard, for the dry run and for the push.
    fn restricted(&self, store: &Store, manifest: &Manifest) -> image_store::Result<Vec<Digest>>;

    /// The name of a medium's format, from its table in a media set's
    /// config, as Remotes lists a set's media; `None` for a table the
    /// platform does not read.
    fn medium_format(&self, format: &toml::Table) -> Option<&'static str>;

    /// The platform's own payload media types with a name for each, as a
    /// transfer lists the blobs of an image. A type no platform names is
    /// shown by its media type.
    fn payload_labels(&self) -> &'static [(&'static str, &'static str)];
}

/// What the views and the registry models read of the compiled-in
/// platforms without calling them. Plain data: the frame makes it once and
/// the dialogs clone it, so nothing on the UI thread borrows a platform.
#[derive(Clone, Debug, Default)]
pub struct PlatformFacts {
    platforms: Vec<&'static PlatformDescription>,
    payload_labels: Vec<(&'static str, &'static str)>,
}

impl PlatformFacts {
    pub fn of(platforms: &[&dyn DesktopPlatform]) -> Self;

    /// Whether a compiled-in platform runs artifacts that name `id`.
    pub fn runs(&self, id: &str) -> bool;

    /// The name of the machines this program runs, as "this release runs
    /// …s" says it: the first platform's name.
    pub fn family(&self) -> &str;

    /// The name of a payload media type, when a platform states one.
    pub fn payload_label(&self, media_type: &str) -> Option<&'static str>;
}
```

The functions that take it, in `library-model`:

```rust
// registry_transfer: `platforms` replaces `providers` and `catalogue`.
pub fn run(
    store: &Store,
    request: TransferRequest,
    platforms: &[&dyn DesktopPlatform],
    send: &(dyn Fn(TransferEvent) + Sync),
    repaint: &(dyn Fn() + Sync),
);
pub fn kind_label(facts: &PlatformFacts, descriptor: &Descriptor) -> String;

// remote_browse
pub fn run(
    store: &Store,
    request: BrowseRequest,
    platforms: &[&dyn DesktopPlatform],
    send: &dyn Fn(BrowseEvent),
);
impl TagRecord {
    pub fn other_platform(&self, facts: &PlatformFacts) -> Option<String>;
    pub fn not_runnable(&self, facts: &PlatformFacts) -> Option<String>;
}

// remote_pull: a drive of a stopped machine, as data.
pub struct Drive {
    /// The machine's id and name.
    pub machine: String,
    pub name: String,
    pub unit: u8,
    /// The drive as its platform names it, such as `A:`.
    pub slot: String,
    /// The drive's own format name, such as `1.44M`.
    pub format: String,
    /// Format names of the media the drive takes, its own first: what
    /// `SlotDescription::formats` (§3.8) says of a running machine's slot.
    pub takes: Vec<String>,
    pub holds: Option<String>,
}
```

How each hook is cut, with the words on the screen unchanged:

| Today | Becomes |
|---|---|
| `platforms::push_plan(store, tag, target, entry, catalogue)` | `image_registry::push::plan_with_catalogue(store, tag, target, entry, \|manifest\| …)` with the digests every platform's `restricted` returns, joined |
| `providers: &[&dyn ConfigValidator]` | every platform's `validators`, in the order of the list |
| `pc_image::CMOS => "CMOS"`, `pc_image::DISK_ZERO => "disk base (zero)"` | `PlatformFacts::payload_label`, after the neutral media types |
| `pc_image::is_pc(id)`, `pc_image::PLATFORM_NAME` | `PlatformFacts::runs(id)`, `PlatformFacts::family()`. The PC core already describes itself as `ibm-pc`, "IBM PC compatible" [REPO: `crates/pc-core/src/core.rs`]; a test in `pc-library` asserts that these equal `pc_image::PLATFORM` and `PLATFORM_NAME` |
| `pc_image::media_set::format_name(table).ok()` | the first platform's `medium_format(table)` |
| `pc_image::template::precedence` | `image_store::tag::precedence`: the function moves to the neutral crate that already holds `tag::compare` and `tag::newest_first`, and `pc_image::template` re-exports it |
| `Drive::geometry` with `accepted_media()` and `FLOPPY_TYPES` | `Drive::format` and `Drive::takes`, filled by the PC code that lists a stopped machine's drives (`shell.rs`, two places, and `stopped_drives` of the frame) |

With one platform every text is byte for byte today's. A hook that names
one artifact asks the first platform, as the command line does (§7.5
step 4); the choice among several is made with the second platform. No
new file spells the platform id out: the neutral crates read it from the
description, so the platform-id scan of `image-store` needs no new entry.

The PC's implementation, in `pc-library`:

```rust
/// The PC's part of the desktop library.
pub struct PcDesktop<'a> {
    validators: &'a [&'a dyn ConfigValidator],
    catalogue: &'a [assembly::FirmwareEntry],
}

impl<'a> PcDesktop<'a> {
    /// `validators` is the PC's provider list; `catalogue` is the firmware
    /// catalogue whose proprietary images restrict a push.
    pub fn new(
        validators: &'a [&'a dyn ConfigValidator],
        catalogue: &'a [assembly::FirmwareEntry],
    ) -> Self;

    /// The PC's facts, for the frame and its dialogs.
    pub fn facts() -> PlatformFacts;
}

impl DesktopPlatform for PcDesktop<'_> { /* description: PcCore's platform;
    restricted: pc_image::firmware::restricted(manifest, self.catalogue);
    medium_format: pc_image::media_set::format_name(table).ok();
    payload_labels: (pc_image::CMOS, "CMOS"), (pc_image::DISK_ZERO, "disk base (zero)") */ }
```

The worker builds it where it runs a transfer, from the two things its
`Backend` already supplies there, so the tests' synthetic catalogue and
the injected crash keep working and a transfer still looks the catalogue
up once [REPO: `library_worker.rs`, `Request::Transfer`]:

```rust
Request::Transfer(job) => backend.with_providers(|providers| {
    let pc = PcDesktop::new(providers, backend.firmware_catalogue());
    library_model::registry_transfer::run(&store, job, &[&pc], &send_transfer, &repaint)
}),
```

Two supporting moves keep the layers legal:

- **The PC's provider list moves from `platforms` to `pc-image`**
  (`pc_image::with_providers`, landed with the CLI split), and `platforms` forwards to it. A
  `pc` crate may not depend on `platforms`, which is `application`, in
  its tests either; the worker and the tests of the registry models need
  the list. The future `cli-pc` needs the same move (§7.1).
- **The worker's wake-up is a function, not an `egui::Context`**:
  `pub type Wake = Arc<dyn Fn() + Send + Sync>`. The frame passes
  `move || ctx.request_repaint()`. This is the worker's only use of the
  toolkit (6 references).

**What the seam does not cover, and what brings each part in.** A neutral
library would also list local images, show their details, act on them and
start them. In steps 3 to 6 the neutral crates do none of the four; the
PC library does, as today.

| Part | Stays PC because | Enters the seam when |
|---|---|---|
| Listing local images | `snapshot` folds the firmware and installation-media collections and the firmware replacement hints into one answer (`firmware_choices`, the catalogue). The row type and the table are neutral already (`library-model`, `library-ui`) | a second platform lists images: `snapshot` splits into the neutral rows and one hook for the hints |
| Details | a readiness check answers `pc_image::inspection::ImageDetails` and a `capabilities::Draft` | a second platform has a details pane: the check returns the platform's own value, and the platform draws it |
| Actions | 24 requests and 16 machine jobs with PC arguments | a second platform has a worker job: the platform's requests and events become associated types the loop passes through |
| Launch | the three start flows interleave PC calls with the session they serve; the shell assembles the machine from a `SessionTransfer` and keeps a `Session<PcMachine>` with the PC catalogue model beside it (§8 step 2) | §4 stage B, when `PcCore::create` reads stored configs: the neutral start of §5 |

For that later step the shape is recorded here so that nothing in steps 3
to 6 works against it; it is **not** part of them:

```rust
/// A platform whose library the neutral worker serves. Sketch.
pub trait LibraryPlatform: DesktopPlatform + Send + 'static {
    /// The platform's own requests and answers; the worker passes them
    /// through unread, as the command line passes a `PlatformCommand`.
    type Job: Send + 'static;
    type Event: Send + 'static;
    /// A verified start, made on the worker thread. A `Machine` need not
    /// be `Send` (§3.5), so the machine itself is made on the UI thread
    /// from this value: by `Core::create` over the receipts it carries, or
    /// by the provider, which returns its own session.
    type Prepared: Send + 'static;

    fn run(&self, store: &Store, job: Self::Job, send: &dyn Fn(Self::Event)) -> Result<(), String>;
    fn prepare(&self, session: &Workspace<'_>, image: &Descriptor) -> Result<Self::Prepared, String>;
}
```

#### 8.1.5. Rules of every move

Each move below is one bead under the rules of §7.5. In addition:

1. **Names stay.** A module that moves whole keeps its name in its new
   crate. A module that is split leaves its view half in its file, which
   re-exports the model half (`pub use`), so no other file changes its
   paths in the same bead. Renaming is never part of a move.
2. **A test moves with its module when it builds there.** A test that
   names a module still above its module's new crate, or, in a neutral
   crate, any PC crate, stays where it is, in a test-only module of the
   same path (as `shell_icons` stayed in step 1), and follows in the move
   that brings the last module it names down. No test is rewritten,
   dropped or weakened. A test's body changes only where a function it
   calls gained a parameter the move states.
3. **The name proof.** `cargo nextest list --workspace` before and after:
   with the package and binary prefix removed, and the renames the move
   states applied, the two sorted lists are equal. Today the window has
   732 tests and 4 ignored ones (MEASURED).
4. **The layer proof.** The new crate declares its layer;
   `scripts/crate-boundaries.sh` passes with the allow list unchanged. No
   move adds a line to the allow list: a test that would need one stays
   behind by rule 2. The move that creates a crate adds its name to the
   window family of the script (the `family` function, the comment above
   it and a self-test fixture), so the rule against CLI crates covers it.
5. **The runner.** A move whose tests read `vendor/`, `machines/` or
   `crates/pc-harness/tests/support/` adds the new crate to those data
   rules of `scripts/affected-tests.sh`; the runner's self-test fails on a
   path no rule covers. The bead's report gives
   `scripts/affected-tests.sh --plan` for one file of the moved module,
   before and after.
6. **Visibility.** `pub(crate)` and `pub(super)` become `pub` only for
   what another crate names. What only a test left behind names goes
   behind the crate's `test-support` feature, as in `ui-kit`,
   `machine-view` and `unemu-pc-core`.
7. **Time is reported, not promised.** Every time below is ASSUMED from
   the MEASURED per-module seconds of §8.1.1; at four tests at once the
   wall time is about a quarter of the sum. A move that adds more than a
   few seconds to the clean build says why (§10).
8. **A new crate** follows `conventions.md`: the SPDX line, a `//!` that
   says what the crate holds and names this section, and a `description`
   in its manifest.

#### 8.1.6. Step 3: the models and the worker, without egui

Twelve moves. 3.1 to 3.5 make the registry side neutral; 3.6 to 3.12 take
the PC models and the worker out from under the views, which cuts the
cycle.

| # | Move | Cut before it moves | Tests | Proof and effect |
|---|---|---|---|---|
| 3.1 | **Done.** **Two helpers into `image-store`.** `Precedence` and `precedence` go from `pc_image::template` to `image_store::tag`, re-exported where they were. A `TempDir` like `assembly::test_support::TempDir` (which stays for the emulation crates) goes behind `image-store`'s `test-support` feature | none | the tests that call only `precedence` move with it | The five call sites in `remote_browse` and `remotes` name `image_store::tag`. Plan: `image-store` and its dependents, as any `image-store` change |
| 3.2 | **Done.** **`library-model` starts with `registries_model`** (1 145 lines, no sibling module, no PC reference). The window re-exports it | none | 16 move, with the temp directory of 3.1 in place of `assembly`'s (6 uses); the one test of `merge_tests` names the application's `preferences` and stays | The first neutral desktop model by Cargo. The script's window family gains `library-model` |
| 3.3 | **Done.** **Listing and the save form.** `ImageRow`, `Listed`, `listed`, `Source`, `source`, `human_size`, `delete_tag`, `host_last_run` and the short digest (`firmware::short`) go to `library_model::listing`; the model of `save_image` (457 of 629 lines: `Target`, `TagLevel`, `TagCheck`, `check_tag`, `next_version`, `local_tags`, `SaveAsImage`) to `library_model::save_image`. `library_view` and `save_image` re-export them | none | the 11 tests of `save_image` move; the listing has none of its own | Cuts ten arrows into `library_view` (`human_size`) and the arrows of `image_settings` and the worker to `ImageRow` |
| 3.4 | **Done.** **`registry_transfer` on the seam, into `library-model`.** `DesktopPlatform`, `PlatformFacts` and `PcDesktop` appear (the last in the window until 3.11); the provider list moves to `pc-image` | the catalogue and the two labels (7 references), §8.1.4 | its 18 tests (15.2 s) hold a `LibraryView` and build stores with `PcProvider`; they stay, in a test-only `registry_transfer` module | The tests that pin the plan's lines, the refusals and the blob labels pass unchanged; the test that the PC core's platform equals `pc-image`'s is new. As landed: see below the table |
| 3.5 | **Done.** **`remote_browse` and `remote_pull`, into `library-model`** | the platform's name and id, the format name, the tag order (6); the drive's geometry (3), §8.1.4 | the 28 tests of `remote_browse` name the worker and stay; the 9 of `remote_pull/tests` draw the transfer view and stay with it | After this move about 6 100 production lines of registry models have no PC dependency. Plan for a file of them: `library-model`, `window`. As landed: see below the table |
| 3.6 | **Done.** **`pc-media` starts.** `media_dialog`, `created_media`, `kept_catalogue`, `drive_chips` move whole, with the model items of `catalogue_menu` (`SetDisk`, `set_disk`, `short_name`) and of `media_picker` (`Disk`, `SetEntry`, `CatalogueDisk`, `format_name`, `drive_name`, `title_parts`, `label_without_file`) that they name; both view files re-export them. The rest of `catalogue_menu` that builds without a toolkit (`Catalogue`, its rows and marks) may come in the same move | `BlankDisk::default()` becomes a constructor that takes the machine-file directory, and the shell passes `machines_dir()` (1 use) | `created_media` 5, `kept_catalogue` 7, `drive_chips` 14 move; the 22 of `media_dialog` name `Presenter` and the library fixture and stay | The four move together because they name each other. Plan for a media model file: `pc-media`, `window`. As landed: `crates/pc-media` (layer `pc`, no toolkit) holds `created_media`, `kept_catalogue` and `drive_chips` whole, `media_dialog` but for two methods, the named items of `media_picker` (`SetEntry`'s `words` and `facet` became `pub` for the picker's search) and of `catalogue_menu` with `own_name`, which `short_name` calls and the picker's `disk_name` names; the rest of `catalogue_menu` stayed. `BlankDisk::new` takes the directory as an `Option`; `for_machine` passes none, since it sets its own. Deviations: the two methods of `media_dialog` that need the desktop stay in the window's `media_dialog.rs`, which re-exports the model: `InsertDialog::library_ui` draws with egui (a view line §8.1.1 missed, as it sits after the file's first test module) and `PendingMedia::apply` takes the session's `Presenter`; they are methods of two window traits (`LibraryInsertView`, `ApplyPending`), so their two callers in `shell.rs` keep their calls and import the traits. Of the 22 tests of `media_dialog`, the 12 of `media_dialog::tests` name neither `Presenter` nor the fixture and moved by rule 2; the 10 of `media_set_tests` and `review_tests` stay, and for them `LibraryMessage`, `LibraryInsert::{refresh, snapshot, pending}` and `PendingMedia::inserts` are `pub`, and the test-only `LibraryInsert::entries` is behind `test-support`. The glyph test of the window reads `pc_media::source_dir()` too (`test-support`, as in `ui-kit`). MEASURED: 38 tests in `pc-media`; plan for `crates/pc-media/src/drive_chips.rs`: `pc-media`, `window`, where it was `window` alone |
| 3.7 | **Done**, in part. **`pc-library` starts** with `firmware_choices`, `hardware` (with `CheckedImage` from the worker and `firmware::is_rom`) and `image_settings` | none: after 3.3 they name only each other and `library-model` | `firmware_choices` 2 move; `hardware` 5 and `image_settings` 15 name the worker, the fixture or `settings_sheet` and stay | As landed: `crates/pc-library` (layer `pc`, no toolkit) holds `firmware_choices` with its 2 tests; the window's glyph test reads `pc_library::source_dir()` too. Deviation: `hardware` and `image_settings` stay in the window, with `CheckedImage` and `firmware::is_rom`: they name `human_size`, `ImageRow` and `SaveAsImage`, which reach `library-model` with 3.3, carried out beside this move and not yet under it; they follow once 3.3 has landed. MEASURED: plan for `crates/pc-library/src/firmware_choices.rs`: `pc-library`, `window` |
| 3.8 | **`Query` and `Filter`, then the model halves of `firmware` and `firmware_metadata`** (712 and 650 lines: the rows, facets and labels, `ScanReport`, `Outcome`, `scan_folder`, `preview_set`, `import_selected`, `Verdict`, `EditSource`, `PackageDetails`, `open_edit`, `preview_edit`, `publish_edit`, `read_package`) into `pc-library` | `firmware_metadata` plans its push with `plan_with_catalogue` and `pc_image::firmware::restricted` in place of `platforms::push_plan` (1) | stay with the views: they drive the frame | The fields of `Query` become `pub` |
| 3.9 | **The model halves of `install_media` and `machines`** (703 and 690 lines: `MediaRow`, `Availability`, `rows`, `all_rows`, `picker_set`, `Control`, `Pick`, `Sources`, `inspect`, `import`; `MachineRow`, `facets`, `visible`, `name_problem`, `id_for`, `setting_diffs`) into `pc-library` | none | stay with the views | |
| 3.10 | **`Snapshot` and `snapshot`** (with `facets_for`, `sort_value`) into `pc-library` | none | stay with the frame | After this move the worker names nothing of `library_view` |
| 3.11 | **The worker.** `library_worker`, `layers_dialog::SourceImage`, `PcDesktop` and the test support (`library_test_support` without its three egui helpers, the synthetic firmware catalogue, `test_wait`) into `pc-library`; the support goes behind `test-support` | `Wake` in place of `egui::Context` (6) | its 30 tests (44.4 s) move, but for those that call test helpers of `layers_dialog` and `image_settings`, which follow in 6.2 | The cycle is cut by Cargo: `pc-library` cannot name the window. `crates/pc-harness/tests/support/` and `vendor/` gain `pc-library` in the runner's rules |
| 3.12 | **The tests left behind whose modules are now all in or below `pc-library`** move there, under the same paths: those of `remote_browse` (20 after 3.5 moved 8 of the 28 to `library-model`; the 28 took 9.8 s), and of `hardware` and `image_settings` where they name no view. The 15 of `registry_transfer` left after 3.4 hold a `LibraryView` in their fixture and wait for the frame (6.4) | none | about 40 tests, about 12 s | No production code changes |

As landed, 3.1 to 3.3 (one bead each, no behaviour change; the workspace
lists the same 3958 test names apart from the crate, MEASURED with
`cargo nextest list --workspace --run-ignored all`):

- **3.1.** `Precedence` and `precedence` are in `image_store::tag`, and
  `pc_image::template` re-exports them; the five named call sites in
  `remote_browse.rs` and `library_view/remotes.rs` name `image_store::tag`.
  The two other callers (the frame in `library_view.rs`, `image_settings.rs`)
  keep the old path through the re-export; both move to `pc` crates. The
  temporary directory is `image_store::test_support::TempDir`, behind the
  feature. Deviation: no test moved. The one precedence test,
  `template::tests::versions_order_by_semantic_version_precedence`, also
  names `template::ALIAS`, so by rule 2 it stays in `pc-image`, where it
  runs through the re-export.
- **3.2.** `library-model` (`crates/library-model`, layer `neutral`,
  counted as a window crate by the boundary check, with a self-test
  fixture) holds `registries_model`; the window's `registries_model.rs`
  re-exports it and keeps `merge_tests`. 15 tests moved, not 16: the
  sixteenth is the one of `merge_tests`, which stays as planned. All six
  uses of the temporary directory, the stayed test's included, are the
  store's. The stayed test imports by name what it took from the old
  module's own `use` lines; its body is unchanged. `RegistriesPage::wait`
  is behind `library-model`'s `test-support` feature, since the window's
  tests call it. `ENV_HINT` and `checks` stay private to the crate.
- **3.3.** `library_model::listing` holds `ImageRow`, `Listed`, `listed`,
  `Source`, `source`, `human_size`, `delete_tag`, `host_last_run` and
  `short`, with the private `tags` and `last_run_utc` they call;
  `library_model::save_image` holds the form's model with `Prompt`, its
  own `short` and its private helpers. `library_view.rs` re-exports the
  listing but `Source`, which no window file names (the import would be
  unused); `library_view/firmware.rs` re-exports `short`; `save_image.rs`
  keeps `form`, `prompts` and their helpers and re-exports the model.
  Visibility: `ImageRow::repository`, `Listed::references` and
  `SaveAsImage::source_repo` became `pub`, since the frame and the form
  name them. 10 tests of `save_image` moved, not 11:
  `the_form_and_both_questions_render_without_a_window` draws the form with
  egui and stays in the window's `save_image::tests`, with copies of its
  two fixture helpers (`digest`, `tags`). The window's test that formats a
  last run reaches `last_run_utc` through
  `library_model::listing::test_support`, behind the feature.

Time of 3.1 to 3.3 (MEASURED on a loaded development host): the 25 moved tests take
2.4 s added up (`cargo nextest run -p library-model`). The runner's plan
for a change to `crates/library-model/src/registries_model.rs` or
`save_image.rs` is `library-model` and `window`; for the window's
`save_image.rs` it is `window` alone, which no longer builds or runs them.

As landed, 3.4 and 3.5 (one bead each, no behaviour change; MEASURED with
`cargo nextest list --workspace --run-ignored all`: the workspace lists the
same 3958 test names apart from the crate, plus the four new tests below,
3962 in all; `library-model` has 38 tests, the window 662):

- **3.4.** `library_model::platform` holds `DesktopPlatform` and
  `PlatformFacts` as §8.1.4 has them, with two helpers that join every
  platform's `validators` and `restricted` in list order;
  `library_model::registry_transfer` holds the model, whose `run` takes
  `platforms` and plans a push with `plan_with_catalogue` over the joined
  restrictions, and whose `kind_label` takes the facts. `PcDesktop` is the
  window's `pc_desktop` module; the worker builds it per transfer from its
  `Backend`'s providers and catalogue, as planned. The provider list was
  already `pc_image::with_providers`; the worker still reaches it through
  `platforms::with_image_providers`, which forwards to it. Deviations: the
  push and pull dialogs keep the facts for their blob labels, so
  `PushDialog::open` and `PullDialog::open` take them; the frame makes them
  once (`LibraryView::platform_facts`) and its dialogs clone them. 3 of the
  18 tests moved, not none, by rule 2: they name neither the window nor a
  PC crate (`a_refused_local_tag_says_what_is_wrong_with_it`,
  `blob_status_names_the_chunk_and_what_a_cancel_waits_for`, and
  `the_default_registry_follows_the_environment_and_a_bad_one_stops_both_dialogs`
  with `image-store`'s temporary directory and empty facts); the texts the
  15 stayed tests compare with are behind `test-support`
  (`registry_transfer::test_support`). The new tests: the PC core's platform
  equals `pc-image`'s, and the PC's payload blobs keep their names, both in
  the window beside `PcDesktop` until it moves (3.11); and two of the facts
  and the joined restrictions in `library-model`, over two toy platforms.
- **3.5.** `library_model::remote_browse` and `remote_pull` hold the two
  models. `remote_browse::run` takes `platforms`: a read tag's media
  formats come from the first platform's `medium_format`, and
  `TagRecord::other_platform` and `not_runnable` take the facts. Remotes
  browses with a `PcDesktop` over no validators and no catalogue: browsing
  asks only the description and the format names, and reading the
  catalogue would count against the worker tests' injected crash. `Drive`
  is the data of §8.1.4; the window's `remote_pull::pc_drive` fills it from
  a PC drive, `takes` with the standard format names of the drive's
  `accepted_media`, so `drives_for` answers as before. Deviations: `Pick`
  carries the facts (`refusal` and `RemotePull::open` keep their
  signatures; the dialog keeps a clone for its progress line); the views
  that state a tag's local state (`state_parts`, `repository_row`,
  `tag_row`, `state_text`) take the facts. The only PC code that lists a
  stopped machine's drives for this dialog is `stopped_drives` of the
  frame: the two places in `shell.rs` build the media picker's own `Drive`.
  8 of the 28 tests of `remote_browse` moved, not none, by rule 2, with the
  helpers only they use (`closed_port`, `row`, `view`, `grouped`, `digest`)
  and copies of `Setup` (over `image-store`'s temporary directory, browsing
  with no platform), `plain`, `open_registry`, `ended` and `names`:
  `roots_put_the_default_registry_first_and_contact_none`,
  `the_catalog_is_read_fifty_repositories_a_page`,
  `an_unreachable_registry_lists_nothing`,
  `log_in_is_offered_only_for_a_basic_challenge`,
  `whole_registry_entries_group_repositories_by_their_first_path_component`,
  `local_names_carry_the_host_except_for_the_family_default_registry`,
  `local_states_compare_within_one_host_qualified_name` and
  `a_failed_next_page_keeps_the_pages_before_it`. The 20 others build PC
  artifacts or browse through the worker and stay; so do the 9 of
  `remote_pull/tests`, which all build records of PC artifacts. For them
  `representative` and `Remotes::touch` are behind `test-support`, with the
  page sizes and annotation keys (`remote_browse::test_support`). Test
  bodies changed only where a function gained the stated parameter
  (`run`'s platforms, the facts), where a `Pick` gained its field and where
  a `Drive` literal became a `pc_drive` call (2).

Time of 3.4 and 3.5 (MEASURED on a loaded development host, `cargo nextest run -p
library-model`): the 38 tests of the crate take 3.9 s added up, under a
second of wall time. The runner's plan for `crates/library-model/src/remote_browse.rs`,
`registry_transfer.rs`, `remote_pull.rs` or `platform.rs` is `library-model`
and `window`; for the window's `registry_transfer.rs` it is `window` alone.

Effect of step 3 (ASSUMED from §8.1.1): about 115 tests and 55 to 60 s of
the 288 s leave the application's test binary (the worker 44 s, the tests
of `remote_browse` 10 s, the small models). A change to the shell or to a
view then runs about 230 s of tests added up, about 60 s of wall time
where it is 76 s now. A change to a model runs its crate and everything
above it, as now. Four test binaries are linked in place of one.

#### 8.1.7. Step 4: the neutral library views

| # | Move | Cut before it moves | Tests | Proof and effect |
|---|---|---|---|---|
| 4.1 | **`library-ui` starts** with the table helpers of `library_view`, the view of `save_image` (`form`, `prompts`) and `registry_transfer_view` (789 lines) | `kind_label` takes the facts, which the push and pull dialogs keep | the one test of `registry_transfer_view` builds a PC store and stays | The frame and the collections draw their tables from the crate |
| 4.2 | **Remotes off the frame, in place.** The `impl LibraryView` blocks of `remotes.rs` and `remotes/details.rs` (about 1 150 lines) become methods of `RemotesView`. What they read of the frame comes in as one borrowed value (the facts, the skin, whether the worker lives, the local tags, the stopped machines' drives as `remote_pull::Drive`, a function that sends a `BrowseRequest`); what they did to the frame goes out as a list of actions the frame applies (select an image, show a tag in the library, New machine from a pulled image, begin a pull, open a registry's login, open the picker on a set) | this move is the cut: the two blocks touch 19 fields of the frame besides `self.remote`, and four of its methods defined elsewhere (`send`, `select_image`, `pending_label`, `new_machine_named`) (COUNTED) | the 39 tests of `library_view::remotes::tests` drive the frame and pass unchanged | No crate changes; the diff is confined to the two files and the frame's calls into them |
| 4.3 | **Remotes into `library-ui`**: `remotes` and `remotes/details` | none after 4.2; the kit's pane items come from `ui_kit::pane` | stay with the frame, which re-exports the module | About 3 600 lines of registry views (Remotes, push, pull, the save form) have no PC dependency by Cargo |

Effect: none in time, since the tests of these views drive the frame. The
step is for the boundary.

#### 8.1.8. Step 5: the media picker and dialogs

| # | Move | Cut before it moves | Tests | Proof and effect |
|---|---|---|---|---|
| 5.1 | **The catalogue menu answers with its own choice.** `catalogue_menu::item` returns which disk was chosen, or "all disks", and the shell pushes `Command::CatalogueDisk` and `CatalogueAll` | the two uses of the shell's `Command` | the 11 tests of `catalogue_menu` pass unchanged or gain the returned value where they read the pushed command | In place |
| 5.2 | **`media-ui` starts** with `media_picker` (the picker's state and view), `media_create` and the view of `catalogue_menu`; the pane colours come from `ui_kit::pane` | none after 5.1 | `media_picker` 27, `media_create` 4, `catalogue_menu` 11 move, but for the one that names the library fixture | Plan for a media view file: `media-ui`, `window`: about 5 s of its own tests, then the shell's |

The 22 tests of `media_dialog` stay with the session until
`upc-dfs.25.7`.

#### 8.1.9. Step 6: the PC views, and what is left

| # | Move | Cut before it moves | Tests | Proof and effect |
|---|---|---|---|---|
| 6.1 | **The dialog models into `pc-library`**: `new_machine` (1 416 lines) and `settings_model` (735) | `SettingsModel::open` takes the machine-file directory, as `open_with_library` does already; the shell passes `machines_dir()` | `settings_model` 25 move, but for the one that names `machines_dir_from`; of `new_machine`'s 21 those that draw `new_machine_view` stay | |
| 6.2 | **`pc-ui` starts with the dialogs**: the PC half of `pane`, `settings_sheet`, `new_machine_view`, `layers_dialog`. The three egui test helpers of the library fixture (`settled_modal`, `Pointer`, `painted_at`) go to `ui-kit`'s `test-support`, since they name no PC | none | `settings_sheet` 6, `layers_dialog` 14, and the tests of `image_settings`, `new_machine` and the worker that waited for these views; those that drive the frame wait for 6.4 | |
| 6.3 | **Commands and machine files down.** `State`, `Command`, `Enablement` (159 lines, which name only `machine_view::pacing::Speed`) go to `pc_ui::commands`; `LibraryEntry` and what builds it (`spawn_library_check`, `machine_paths`, `select_machine`, the diagnostics) go to `pc_library::machine_files`. `shell_model` keeps the hotkeys, the status-bar widths and the texture, and re-exports the rest | none | the 17 tests of `shell_model` go with what they test; the move lists the paths that change | After this move the frame names nothing of the application |
| 6.4 | **The frame.** `library_view` with the views of `machines`, `firmware`, `firmware_metadata`, `install_media`, `layout_tests`, and every test left behind whose modules are now in or below `pc-ui` | none: a move of whole files, about 9 000 production lines and 20 000 lines of tests | `library_view` 37, `layout_tests` 27, the collections 51, Remotes 39, `remote_pull` 9, `registry_transfer` 18 and its view's 1; the few that name `preferences` or the shell's hotkeys stay | **This is where the desktop's test time splits.** 140 s of tests move to `pc-ui` with this move (the frame 48.0, its layout tests 40.1, the collections 30.3, the registry and Remotes tests that drive the frame 21.7; MEASURED, `*`), beside the dialogs' of 6.1 and 6.2. A shell change then runs the application alone: about 70 s added up as measured here, about 110 s with the sound-level tests at their preliminary 44 s, so 20 to 30 s of wall time where it is 76 s now. A change to a library view runs `pc-ui` and the application |
| 6.5 | **What is left is the application**: `shell`, `shell_model`, `app_menu`, `macos_menu`, `preferences`, `preferences_sheet`, `main`. Optionally the package takes the name `unemupc`, with the changes §7.5 step 6 lists for the command line (CI, the release workflow, `scripts/third-party-licenses.sh`, the reasons in `deny.toml`, the runner's rules, the script's window family, `commands.md`, `architecture.md`) | none | none move | No time effect |

Order. Step 3 comes first and step 4 needs only 3.1 to 3.5. Steps 5 and 6
do not depend on each other; step 6 brings the time, so it may go before
step 5. Inside step 3 the two halves (3.1 to 3.5, 3.6 to 3.11) are
independent until 3.11, which needs both, so two workers can hold one
each.

#### 8.1.10. Where `upc-dfs.25.7` fits

`upc-dfs.25.7` plans a `pc` crate for the session tests that need a real
PC and stayed in the window in step 2: the session tests of `main.rs` (50
tests, 16.8 s), the three sound-level tests (44 s, PRELIMINARY), the six
host-audio tests and the mouse tests. They are the tests of `Presenter`,
so that crate is where `Presenter` lives.

- It depends on this plan in one place: `Presenter` names `drive_chips`
  (`drive_views`, `observe_drive_lights`), which is in `pc-media` after
  3.6. So it can start after 3.6 and needs nothing else of steps 3 to 6.
- It should sit **below** `pc-media`, with those two methods becoming
  functions of `drive_chips` over the session. The other way round, every
  change to a media model would run the sound-level tests.
- The 22 tests of `media_dialog` that put pending media into a
  `Presenter`, and the shell's start of a session (`poll_images` in
  `shell.rs`: `SessionTransfer::restore`, `assemble_with_verified_bases`,
  `Presenter::from_assembled`), are its natural next contents; neither is
  moved by steps 3 to 6.
- With it, the application's own tests after 6.4 are the shell's: about
  45 s added up, about 12 s of wall time.

#### 8.1.11. Questions answered by the maintainer

Answered on 2026-11-18: Q1 (a), neutral by count; Q2, the names as
proposed. The options are kept for the record.

**Q1. How much of the local library becomes neutral now?** §8 placed the
library model, the worker and the views of machines and installation
media in a neutral `library-ui`. Counted, they are PC (§8.1.2).

- (a) **Neutral by count.** The registry models and views, the listing
  types, the tables and the save form become neutral, behind the five
  hooks of §8.1.4. The worker, the frame and the collections move to `pc`
  crates as they are. The worker gets its seam when a second platform has
  a library (§8.1.4, last part). This is the plan above.
- (b) **The worker's loop neutral now.** As (a), and after 3.11 the loop
  moves to `library-model` with the PC requests and events as associated
  types and the three start flows calling it. About four more beads in
  the worker and its 30 tests; no gain in time; the frame stays PC.
- (c) **A neutral frame now.** As (b), and every collection, pane and
  dialog of the frame behind a platform trait.

Recommendation: **(a)**. It is the direction as stated: platform GUI stays
in per-platform modules and is generalised when a second platform needs
it. (b) and (c) design a library seam against one platform, which is how
the CLI seam was *not* designed: there the neutral half existed first and
the hooks were found by counting. Under (a) every `pc` crate still leaves
for the front-end repository with the rest (§9), so nothing here blocks
the extraction.

**Q2. The names** `library-model`, `pc-media`, `pc-library`, and
`media-ui` for a crate of layer `pc`. They are working names; a rename is
one bead at any time.

Recommendation: keep them. `pc-media-ui` would say the layer, but §8
named the crate, and the layer is in its manifest.

## 9. Towards two repositories

Not part of the first beads; recorded so that no step works against it.

| Destination | Crates |
|---|---|
| The contract's own source (§11 Q1) | `unemu-core-api` |
| The PC core repository (unemupc) | `device`, `bus`, `cpu`, `video`, `audio`, `devices`, `chipset`, `machine`, `testkit`, `assembly`, `unemu-pc-core`, `pc-harness`, and by the recommendation of §11 Q4 `machine-file` and the journal half of `media` |
| The front-end repository | `image-store`, `image-registry`, the media builders, the recipe crate, `cli-core`, `cli-pc`, `upcli`, `platforms`, `pc-image`, the window crates |

What must be true first:

1. The boundary check passes with an empty allow list.
2. `PcCore::create` reads stored configs (§4 stage B), so the front end's
   generic start path works for the PC.
3. `machine-file` no longer depends on `image-store`: its firmware-package
   lookup takes a small trait the provider implements over the store.
4. `media` is settled (§11 Q4).
5. The contract is 1.0 and comes from one source.

Decision-30 then needs one sentence of care: `unemu.min-app-version` names
the oldest release of the program that provides the manifest's platform.
After the split that program is the **core**: the marker keeps meaning the
unemupc release, `CoreDescription::version`, and the front end has its own
version that images never name.

## 10. Risks

| Risk | Answer |
|---|---|
| The contract is frozen too early and is wrong | It stays 0.x until the PC adapter and a second core both run on it. 0.x minors may break, by rule |
| A move changes behaviour unnoticed | Every step moves tests with code, weakens none, and runs the full gate. The encoders' byte-level tests move unchanged. The adapter is compared against direct `AssembledMachine` sessions |
| More crates, longer clean builds and links | Expected to be neutral: the same code in more units compiles more in parallel. Measured per step with the figures of `upc-x83.19`; a step that adds more than a few seconds to the clean build says why |
| The neutral crates quietly regain PC dependencies | The boundary check, with a list of exceptions that can only shrink |
| Two versions of the contract in one build | One source for the crate; the front end's test over every compiled-in core |
| Pulling pictures and sound costs a copy | One frame borrow and one `Vec` append per paint, as today's sinks do; measured in the adapter's step |
| Font assets under CC BY-SA in a core crate | Licence metadata and the `deny.toml` exception move with them (§11 Q5) |

## 11. Questions answered by the maintainer

Answered on 2026-11-17: the recommendation of each question below was
accepted — Q1 (a), Q2 `unemu-core-api`, Q3 (a), Q4 (a) for both, Q5 (a).
The options are kept for the record.

**Q1. Where does the contract crate live, and how do cores and the front
end get the same copy?** Cargo treats one crate from two git revisions as
two crates, so pinning by revision in each repository breaks the trait.

- (a) A member of this workspace now; its own small repository once
  unemunes starts; published to crates.io at 1.0, with SemVer
  requirements.
- (b) Its own repository from the first day, every consumer pinning the
  same tag, with a CI check that they agree.
- (c) Inside the front-end repository for good, cores depending on that
  repository.

Recommendation: **(a)**. It costs nothing now, and SemVer through a
registry is the only arrangement in which Cargo itself guarantees one
copy. (c) makes every core depend on the repository that depends on it.

**Q2. The name.** `unemu-core-api` or `unemu-contract`?

Recommendation: **`unemu-core-api`**. It says whose API it is; "contract"
alone is also what we call the x86jit ABI rules and the IRQ injection
contract.

**Q3. May a provider crate on the front-end side depend on its core's
crates beyond the contract?** `pc-image`, `cli-pc` and the PC dialogs need
`assembly` (firmware catalogue, capabilities, config types) and
`machine-file`.

- (a) Yes, for crates in the `pc` layer only, checked by the boundary
  script; the neutral front end depends only on the contract.
- (b) No: everything a provider needs crosses the contract as data.

Recommendation: **(a)**. It matches "generalise only when a second
platform needs it": (b) means designing settings, firmware catalogues and
machine files as neutral data now, for one platform.

**Q4. Where do `machine-file` and the journal half of `media` live once
there are two repositories?**

- `machine-file`: (a) with the PC core, its store lookup behind a trait;
  (b) with the front end, and the goldens that load machine files with it.
- `media` (journals and overlays): (a) with the PC core for now, the
  front end taking it from the pinned core like any other PC crate, and
  the FAT12 builder and image creation split off to the front end;
  (b) with the front end, storage handed to the core as objects through
  the contract; (c) its own repository.

Recommendation: **(a) for both**, decided finally at extraction. The
goldens need machine files, and the config codec is built on
`machine-file`. Handing storage in as objects is the cleaner end state and
the contract leaves room for it (§3.12), but it re-plumbs `assembly`'s
persistence and belongs with layered disk images, not with this split.

**Q5. The PC text rasteriser and its fonts move below the contract, into
the PC adapter.** The contract then carries pixels only, and the CC BY-SA
font assets, their notices and the `deny.toml` exception move from
`window` to `unemu-pc-core`.

- (a) Move them, as designed.
- (b) Keep them in the desktop and let the contract carry a text grid.

Recommendation: **(a)**. A text grid with MDA and VGA attributes is PC
vocabulary in the contract, and every other front end would need the fonts
again.
