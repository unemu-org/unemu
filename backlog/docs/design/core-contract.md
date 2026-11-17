# Core contract and the front-end split

Status: **Accepted** by the maintainer on 2026-11-17 (design step of
`upc-dfs.24`, with `upc-3bm`), with the recommended answer to each question
of §11. Steps 1 and 2 of §7.5 are implemented; the other steps land one at
a time. The decision record is
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
    /// `now()` may pass `limit.until` by less than one step; a run never
    /// returns before a limit without an event. Fails with
    /// `ErrorKind::Guest` when the emulated machine cannot continue and
    /// with `ErrorKind::Storage` when a host write failed; either leaves
    /// the status `Failed`.
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
    /// possible in the current status.
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
| `cli-core` (`crates/cli-core`) | neutral | The command model and top-level parser; usage assembly; exit-code policy. Commands: `image ls`, `image sweep`, `image compress`, `image workspace ls`, `image workspace discard`, `image media ls`, `show`, `image firmware ls`, `show`, `image push`, `pull`, `remote ls`, `login`, `registry set`, `media generations`, `verify`, `rollback`, `commit`. The `CliPlatform` trait (§7.2). Depends on `image-store`, `image-registry`, `media`, `anyhow`, `toml` |
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
| 4 | **The provider seam, inside `cli`.** Add `CliPlatform` and `PlatformCommand`; route every PC command through one PC implementation; the neutral commands take providers as arguments. Still one crate. A test asserts the usage text and the parse of every documented command line are unchanged | None in time. Makes step 5 a move |
| 5 | **`cli-core`.** Move the neutral modules and their tests (`media_tools`, `media_fifo`, the neutral halves of the image command tests, with the store's synthetic providers). The rest of `cli` keeps working through re-exports for one step | A neutral command change rebuilds `cli-core` and its dependents; a PC command change no longer runs the neutral tests. A few seconds |
| 6 | **`cli-pc` and `upcli`.** Rename what is left to `cli-pc`; create the `upcli` binary crate. The four test binaries that run `upcli` (`firmware_catalogue`, `timing_adoption`, `machine_commands`, `image_registry_commands`) move to the `upcli` package, because Cargo gives the binary's path only to the package that builds it. Update the package names in CI, the release workflow, `scripts/third-party-licenses.sh`, `scripts/ci-gate.sh`, `commands.md` and `architecture.md` | None in time. The neutral command line is PC-free by the boundary check |

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
2. **The contract, `unemu-pc-core` (stage A) and `machine-view`.** The
   session has few inbound arrows (the shell and the media dialogs), so it
   comes out early, and it is where the contract first carries real
   traffic. Its tests go with it: the sound-level tests (44 s of test
   time), the host-audio and session tests. A library change stops
   running them, and a session change runs only them: about 80 s of
   desktop tests down to about 15–20 s.
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
