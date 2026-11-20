# Architecture

This describes the implementation on `main`: a deterministic, headless IBM
PC/AT-class emulator with a real-mode CPU, device models, declarative assembly,
and BIOS/FreeDOS integration tests. Testability, swappability, and documented
hardware coverage guide the crate boundaries. Corrected timing and deadline
dispatch are adopted; design documents distinguish implementation from proposals.
The hardware specification is [`design/spec.md`](design/spec.md).

## Crate map and dependencies

These are the direct dependencies from each crate's `Cargo.toml`; test-only
dependencies are separate. Workspace membership and the exact `x86jit-core` git
revision are in the root [`Cargo.toml`](../../Cargo.toml). There is no firmware
crate: `machine-file` (through the lowering of `pc-config`) reads and hashes declared images into inline buffers,
`assembly` also supports direct Rust image sources, and guest assets live under `vendor/`.

| Crate | Responsibility | Runtime dependencies | Test-only dependencies |
| --- | --- | --- | --- |
| `pc-image` | IBM PC/AT provider: the store's half of resolved bundle-profile validation (descriptors, kinds, size limits, source links, layer chains, the zero-base rule, the closure), which validates a stored config as a `pc_config::launch` document of its content rows and default journal records, with a `ContentSource` over the blobs; machine-file import, disk-chain validation, isolated sessions and offline publication; the PC firmware-package rules and the machine file's firmware-package lookup over the store (`firmware::StorePackages`); the `ibm-pc` recipe provider (`pc_image::recipe::PcRecipes`) | `assembly`, `machine-file`, `media`, `pc-config`, `image-store`, `recipe`, `unemu-core-api`, `serde`, `toml`, `thiserror` | `cpu`, `machine` |
| `recipe` | Platform-neutral recipes ([design](design/recipes.md) §6): the typed model of Mediafiles, Firmwarefiles and work files, the TOML 1.0 parser with file, line, column and key path for every finding, the static checks of §6.11, the provider traits a platform implements (media formats and volume profiles, firmware rules, needs, software platforms, `[ids]` catalogues) and the JSON Schema written from the key tables and the providers. Builds nothing and reads no input | `image-store`, `serde_json`, `toml`, `toml_parser` | None |
| `image-store` | Platform-neutral OCI Image Layout, streaming SHA-256/zstd blobs, deterministic machine/media-set/firmware manifests, provider-validated closure and atomic compare-and-swap local tags, the orders of tags (set and package revisions, the Semantic Versioning precedence of machine-image versions), stable workspace ownership | `serde`, `serde_json`, `toml`, `sha2`, `zstd`, `thiserror`, `libc` (Unix) | None |
| `image-registry` | Platform-neutral OCI distribution client: Docker-style references against the default registry, preferences registries list, environment/Keychain credentials, anonymous-then-Basic authentication, resumable chunked push, parallel verified pull, push guard and paged listings | `image-store`, `ureq` (rustls, no default features), `rustls` (ring), `rustls-native-certs`, `serde`, `serde_json`, `toml`, `thiserror` | None |
| `media` | Immutable sparse disk layers, chain sector sources, floppy journal encoding, streaming recovery/replay, retention-anchor validation, durable host sink and stable single-writer locks ([format](design/persistent-media.md#chosen-format-byte-layout)) | `sha2`, `serde`, `toml`, `thiserror`, `libc` (Unix) | None |
| `device` | Hardware-neutral contracts: `Device`, `InterruptController`, `DmaMemory`, `VClock`, `NextEvent`, access traces, DMA messages, and the observation capabilities (text scanout, byte output, `ByteCapture`) | `thiserror` | None |
| `cpu` | `Cpu`/`CpuStop` seam and `X86JitCpu`, which owns the core's `Vm` and `Vcpu` | `x86jit-core`, `thiserror` | None |
| `bus` | Owns boxed devices and executes a port/MMIO `DecodeMap`; open-bus policy and ticking | `device`, `thiserror` | None |
| `devices` | UART 16550, PIC 8259, PIT 8253, 8042, CMOS/RTC, DMA 8237, uPD765 floppy controller, VGA and MDA text cards, system control, POST port and Lo-tech EMS board | `device`, `thiserror` | `testkit`, `video` |
| `chipset` | Shared/cascaded PIC composition, null interrupt controller, AT port constants; no `Chipset` trait | `device`, `devices` | `testkit` |
| `machine` | Generic `Machine<C: Cpu>` and execution/clock/IRQ/DMA loop | `cpu`, `bus`, `device`, `thiserror` | `testkit`, `devices` |
| `video` | `FrameSink`, text frames/grids, text-mode renderer and capture sink | `device` | None |
| `assembly` | `MachineConfig`, validation, presets, image/device construction, wiring, and `RunPolicy` driver | `machine`, `cpu`, `bus`, `device`, `devices`, `video`, `chipset`, `media`, `sha2`, `thiserror` | `media/test-support` |
| `unemu-core-api` (external) | Not a workspace crate: a git dependency pinned to release v0.2.0 (contract 0.2) of its own repository, [unemu-org/unemu-core-api](https://github.com/unemu-org/unemu-core-api) (`MIT OR Apache-2.0`). The contract between front ends and emulator cores ([core contract](design/core-contract.md) §3, §13.1): `Core` with its `CoreDescription`, the `Machine` a front end drives (run to a limit, picture and sound, input by USB HID usage, media slots, reset, saved state, finish, messages), the `CONTRACT` version and its `drives` rule; content and storage are objects the front end hands in (`ContentReader`, `Storage`, `Record`), never a path, and a core can identify a file a person opens. `std` only, object-safe, every type `#[non_exhaustive]` | None | None |
| `unemu-pc-core` | The PC core behind the contract (§4, §14.2.3): `PcMachine` wraps an assembled machine and runs it to tick deadlines, with the set-1 scan-code encoder and the 8042 back-pressure (`keyboard`), the pointing-device queue (`mouse`), the deadline-to-cap conversion (`pacing`), the CP437 text rasteriser and its CC BY-SA fonts (`raster`, `font`) and drive-activity sampling; `PcCore::validate` checks a launch document without content, and `create` / `create_machine` lower it through `pc-config` and assemble over contract content readers and storage records (`mount`); fixed MDA/VGA display order, run-only firmware permission and speaker trace, and the effective config through `PcMachine::config()` | `unemu-core-api`, `assembly`, `pc-config`, `audio`, `device`, `devices`, `machine`, `video` | None |
| `unemu-test-core` | A scripted core for the front end's tests ([core contract](design/core-contract.md) §13.3): `TestCore` implements all of contract 0.2 and emulates nothing; its content is a `utc` script (picture size, aspect and visible part, clock, sound rate, input ports, slots with a catalogue, features, battery memory, events at a virtual time), its picture a pattern of the frame count with a field of the held pad controls and a cross per pointer, its sound a square wave at exactly `floor(now × rate / clock)` frames. In no program's list of cores | `unemu-core-api` | `unemu-core-api/test-support` |
| `core-launch` | The front end's launch path ([core contract](design/core-contract.md) §13.3, §13.4): `Cores`, the list of cores a program was built with, each checked against the contract it drives (`usable`, `refused` with the reason of §3.2, `by_id`), the About lines both programs print, one per core, and the host file dialog's filters (`filters`); `prepare`, which hashes a file whole through a `unemu-core-files` reader, asks the cores (the named one, or those listing the extension first), checks the config the answering core wrote and gives `Outcome::Ready` or `Choose`, or a `LaunchError` with the sentence the application shows; `start`, which creates the machine over that one file and a `FileStorage` under `HostDirs::saves`/CORE/aa/DIGEST. A platform whose machines start from more than one file hands its start in as `Parts` (core, platform, title, notes, config, a `ContentPlan` and a `StoragePlan` made on a worker and opened on the machine's thread at every start, options) to `Prepared::from_parts`, whose file-only accessors (`kind`, `file_name`, `sha256`, `size`, `saves_dir`, `states_dir`) answer `None`; `with_options` adds the application's run-only options; `start_with` lets that platform's code build its own machine type from the same request (`start` is `start_with` over `Core::create`); `Cores::for_platform` finds the core of a platform id (core contract §14.2.5). Names no platform and no host location: each program hands it `platforms::cores()` and its directories | `unemu-core-api`, `unemu-core-files`, `sha2` | `unemu-test-core` |
| `player-ui` | The view of one machine started from a file, driven through the contract alone ([core contract](design/core-contract.md) §13.3, the approved `mockups/player.html`): `Player`, started from a `core_launch::Prepared` and holding the machine in a `machine_view::Session` on its first display; its Machine, Media (one section per removable slot) and View menus and its status line, built from the machine's description so a control it does not offer is absent; the screen through `machine_view::screen::layout` (overscan hidden by default, integer scaling, the absolute pointer aimed with a cross-hair); the Input dialog over `machine_view::pads::PadBindings`; the player's message boxes (`MessageBox`); its own command vocabulary (`Command`) and the `Request`s it hands back (stopped, full screen, new pad keys). Power cycle is a hard reset when listed, otherwise the machine finishes and starts again from the same `Prepared` (a file or parts). Save and Load state are offered only for a start that has a states directory, which a start from parts has not. The desktop (`window`) holds one beside the PC's presenter. A window crate for the boundary check | `unemu-core-api`, `machine-view`, `core-launch`, `ui-kit`, `egui`, `winit` | `unemu-test-core`, `machine-view/test-support`, `ui-kit/test-support` |
| `machine-view` | The desktop's session of one machine over `dyn Machine` alone (§5, §8, core-contract.md §13.3): `Session` with pacing against the host clock, the host audio ring at any rate, the winit key-code to HID usage map, pads from bound keys, pointer capture and the uncaptured absolute pointer, resets by kind, saving and loading a state, the host media and mute chords, the macOS system-key tap and the drives' activity; `screen`, the geometry a view shows a picture with (visible part, aspect, integer scaling) and its inverse for the absolute pointer | `unemu-core-api`, `winit`, `cpal`, `objc2*` (macOS) | `unemu-test-core`, `unemu-core-api/test-support` |
| `window` | The egui desktop application: the shell with its menus, toolbar and hotkeys (`shell`, `shell_model`, `app_menu`, `macos_menu`), the preferences and their sheet (`preferences`, `preferences_sheet`) and `main`; its machine window is a `machine_view::Session` over a `unemu_pc_core::PcMachine`, which the media dialogs' session half (`media_dialog`, until the session has a crate of its own) reaches through `PcMachine::assembled`. The machine library's frame, the PC dialogs and the command vocabulary come from `pc-ui`, the PC media and library models and the library's worker from `pc-media` and `pc-library`, the neutral library models and views from `library-model` and `library-ui`, and the PC media views (the media picker, Create from files…, the Catalogue submenu) from `media-ui`, re-exported under their old module paths. File › Open… and `--open FILE [--core ID]` prepare a file with `core_launch::prepare` on a worker thread and run it in a `player_ui::Player`, which the shell holds beside the PC's presenter, one machine at a time: the stop prompt asks before either replaces the other, and the commands, menus, toolbar, title, status line, central panel and raw input go to whichever runs; the pad keys live under `pads` in the preferences, and machines started from a file keep their records under `unemupc/saves` and `unemupc/states` in the data directory (`core_launch::HostDirs`). The tests that stay here name the application: its preferences, its hotkeys, its command line and setup from a machine file, the drive lights or the program's provider list; those of Open… run the test core, and the session's tests over a real PC are in `pc-session-tests` | `assembly`, `machine`, `winit`, `egui`, `eframe`, `image-store`, `image-registry`, `pc-image`, `platforms`, `core-launch`, `player-ui`, `ui-kit`, `library-model`, `library-ui`, `pc-media`, `pc-library`, `media-ui`, `pc-ui`, `unemu-core-api`, `machine-view`, `unemu-pc-core` | `cpu`, `assembly/test-support`, `image-store/test-support`, `media/test-support`, `pc-image/test-support`, `ui-kit/test-support`, `library-model/test-support`, `library-ui/test-support`, `pc-media/test-support`, `media-ui/test-support`, `pc-library/test-support`, `pc-ui/test-support`, `machine-view/test-support`, `unemu-pc-core/test-support`, `unemu-test-core` |
| `pc-session-tests` | Tests only, no library code ([core contract](design/core-contract.md) §8.1.10): the desktop's session of one PC, a `machine_view::Session` over a `unemu_pc_core::PcMachine` assembled from a machine config, driven as the desktop drives it: pacing against the host clock, host keys, the media and mute chords and the pointer at run boundaries, resets, the drives' journals at Stop, host audio and the host mix levels (the shipped open games machine file and template image); launch-factory / verified-file session equality and host media append (§14.3 S9). Neither the session (neutral) nor the desktop (application) holds them, so a change to the session or the PC core runs them and a change to the shell does not | none | `assembly/test-support`, `audio`, `cpu`, `device`, `machine`, `machine-file`, `media`, `pc-config`, `video`, `machine-view/test-support`, `unemu-core-api/test-support`, `unemu-pc-core/test-support`, `winit`, `image-store`, `pc-image`, `pc-library` |
| `pc-media` | The desktop's PC media models without a toolkit ([core contract](design/core-contract.md) §8.1): the Insert floppy dialog's catalogue drafts, the library insert, the blank-disk form and the floppies that wait for a Start (`media_dialog`), Create from files… and its pending record (`created_media`), the catalogue a library machine keeps at Stop (`kept_catalogue`), the status bar's drive views (`drive_chips`), and the sets, disks and short names of the picker and the Catalogue submenu that these name (`media_picker`, `catalogue_menu`) | `assembly`, `devices`, `image-store`, `machine-file`, `media`, `pc-image`, `unemu-core-api`, `serde`, `sha2`, `toml` | `assembly/test-support` |
| `media-ui` | The desktop's PC media views, drawn with egui ([core contract](design/core-contract.md) §8.1): the media picker, one Insert… per floppy drive, with its targets, sources, search and facets (`media_picker`), Create from files…, the picker's second view, which lays host files out on a diskette (`media_create`), and a floppy drive's Catalogue submenu with its rows and window past twelve disks (`catalogue_menu`), which answers with the disk chosen, or All N disks…, for the shell to turn into its command. The pane colours come from `ui_kit::pane`. Behind `test-support`: the source folder for the desktop's glyph test and the picker's fixtures (`media_picker::test_support`). The window re-exports them under their old paths | `assembly`, `egui`, `image-store`, `image-registry`, `media`, `pc-image`, `pc-media`, `ui-kit` | `assembly/test-support`, `ui-kit/test-support` |
| `pc-library` | The desktop's PC library without a toolkit ([core contract](design/core-contract.md) §8.1): the library's worker thread, which owns the store and runs every image, registry, firmware, media and machine job off the frame loop and wakes the frame through a plain function (`library_worker`), the PC's `DesktopPlatform` that hands the PC to the neutral registry models (`pc_desktop`), the image a workspace pins for the disk-layer dialog's save form (`layers_dialog`), the frame's query and the worker's snapshot of every list (`snapshot`), the Machines collection (`machines`), the Firmware collection with ROM-folder scans and imports (`firmware`) and a package's statement with Edit metadata… (`firmware_metadata`), the Installation media collection with its import (`install_media`), what the details panes say of an image's hardware (`hardware`, with the readiness check's `CheckedImage`), Settings… on an image (`image_settings`), the firmware choices with the newer-firmware hint (`firmware_choices`), File › New machine…'s wizard (`new_machine`), the property sheet of a machine file (`settings_model`, opened with the library's machine-file directory), and the machine-file list with each row's readiness check on a host thread and its diagnostics (`machine_files`). Behind `test-support`: the library's fixtures (`library_test_support`, with a synthetic firmware catalogue), machine rows read at once (`LibraryEntry::inspect_with_rom_dirs`) and the tests' hang guard (`test_wait`). The window re-exports them under their old paths; their views are in `pc-ui` | `assembly`, `image-store`, `image-registry`, `library-model`, `machine-file`, `media`, `pc-media`, `pc-image`, `serde`, `toml`, `unemu-core-api`, `unemu-pc-core` | `assembly/test-support`, `image-store/test-support`, `image-registry/test-support`, `library-model/test-support`, `pc-image/test-support`, `sha2` |
| `pc-ui` | The desktop's PC views, drawn with egui ([core contract](design/core-contract.md) §8.1): the machine library's frame (`library_view`: the Explorer tree, the image lists and details panes, the Machines, Firmware and Installation media collections with their dialogs and Edit metadata…, Remotes over `library_ui::remotes`, and the push, pull and Pull… dialogs the frame owns, with the PC's description of a stopped machine's floppy drive in `remote_pull`), Settings… of a machine file, Settings on an image and the review of its new revision (`settings_sheet`), File › New machine…'s wizard (`new_machine_view`), the disk-layer dialog with its save form (`layers_dialog`), the PC firmware pieces of the library's details panes beside the kit's layout (`pane`), and the desktop's command vocabulary that the views push and the shell carries out (`commands`: `Command`, `State`, `Enablement`). The frame wakes the worker's events with a repaint function, and lends Remotes what it reads of the library and applies what Remotes asks of it. Behind `test-support`: the source folder for the desktop's glyph test, the disk-layer dialog driven as a user drives it (`layers_dialog::test_support`), and the fixtures the application's tests drive the frame with (the frame on a synthetic worker, the collections' fixtures, the Remotes rig and the push lab beside a mock registry). The window re-exports them under their old paths | `assembly`, `egui`, `image-store`, `image-registry`, `library-model`, `library-ui`, `machine-view`, `media`, `pc-image`, `pc-library`, `pc-media`, `ui-kit` | `assembly/test-support`, `cpu`, `device`, `image-registry/test-support`, `image-store/test-support`, `library-model/test-support`, `library-ui/test-support`, `media/test-support`, `media-ui/test-support`, `pc-image/test-support`, `pc-library/test-support`, `ui-kit/test-support`, `toml`, `video` |
| `ui-kit` | The desktop's skin and widget helpers, free of any machine: the classic and modern skins (`skin`: visuals, fonts, title strips, bevels, the one-Escape rule, the text marks), the mock-up icons and their tested inventory (`shell_icons`), the library's details panes (`pane`), the brand logo, host file dialogs and clipboard shortcuts ([core contract](design/core-contract.md) §8); behind `test-support`, the helpers that drive egui frame by frame for the tests of the crates that draw with it (`test_support`: a modal's settled place, a scripted pointer, where a text is painted) | `egui`, `winit`, `image`, `rfd`, `arboard` | `toml` |
| `library-model` | The desktop library's platform-neutral models, without a toolkit ([core contract](design/core-contract.md) §8.1): the Preferences › Registries page (`registries_model`), Image › Push… and Pull… (`registry_transfer`), browsing the registries of Remotes (`remote_browse`) and Pull… from them (`remote_pull`), the rows of the image lists with their source, sizes, short digests and last-run time (`listing`) and the Save as new image… form and its tag rules (`save_image`). What they ask of a platform goes through the desktop's provider seam (`platform`: `DesktopPlatform`, `PlatformFacts`, §8.1.4), which the caller fills; the PC's implementation, `PcDesktop`, is `pc_library::pc_desktop`. The desktop re-exports the models under their old paths and draws them | `image-store`, `image-registry`, `unemu-core-api`, `toml` | `image-store/test-support`, `image-registry/test-support` |
| `library-ui` | The desktop library's platform-neutral views, drawn with egui ([core contract](design/core-contract.md) §8.1): the list tables of every collection and of Remotes, with the headers that sort, the columns that fit the list's room and the name cell with its icon and badges (`table`), the Save as new image… form and its questions (`save_image`), the Push…, Pull by reference… and Pull… dialogs (`registry_transfer_view`) and the Remotes group with its tree, list and details pane (`remotes`). Remotes draws without the library frame: the frame lends it the platform facts, the skin, the local tags and the stopped machines' drives (`RemotesContext`) and applies, in order, what it asks (`RemotesAction`: browsing requests, the node to show, Show in library, New machine from image…, Pull…, Log in…, the media picker). Behind `test-support`: the source folder for the desktop's glyph test and the layout figures and helpers the desktop's tests of these views compare with. The desktop re-exports them under their old paths | `egui`, `image-store`, `image-registry`, `library-model`, `ui-kit` | None |
| `pc-config` | The PC config codec below the contract ([core contract](design/core-contract.md) §14.2.1): the machine schema 1 (`schema`), its TOML decoder (`parse_toml`, `ParseError`), the source origins and their TOML half (`Origins`, `source`), the guest attachment names (`pc_names`) and the lowering to `assembly::MachineConfig` through assembly validation (`lower`, `lower_tracked`, `MachineFile::describe_with`), which asks its `ImageReader` for every image and everything of the host; and the stored PC image config (`image`: its model, decode and encode, its own `Digest` in the store's text form, the hardware-table encoding `image::encode` and frozen defaults `image::defaults`, and the codec half of validation `image::lower` over the store's `ContentRow`s and a contract `ContentSource`); and the launch document the PC core is to read (`launch`, `application/vnd.unemu.pc.launch.v1+toml`: the stored config plus a `content` table of each bound digest's flat SHA-256 and size and a `record` table of each journal's record and retention; `decode`, `encode`, `check` without content, and `lower`, which reads firmware and CMOS and names each disk base and journal by a key). Opens no file and reads no environment; `#![forbid(unsafe_code)]`. Behind `test-support`: the lowering's copy of the journal's retention-name rule and an empty diagnostics context, for the tests `machine-file` keeps | `assembly`, `unemu-core-api`, `serde`, `sha2`, `toml`, `thiserror` | None |
| `machine-file` | Machine description files on the host ([schema](design/machine-files.md#schema-field-by-field)): re-exports every name of `pc-config` at its old path and adds the YAML reader and the YAML half of the origins (`YamlOrigins`), `extends` and presets (`resolve`), journal path checks, firmware discovery, the seam of the firmware-package lookup (`FirmwarePackages`, which `pc-image` implements over the store; the prepared document keeps the packages' ids as text), the host's image reader (`FileImageReader`), `describe` over the host's file metadata (`Describe`) and the loaders (`load*`, `LoadedMachine`, `PreparedMachineFile`) | `assembly`, `pc-config`, `media`, `serde`, `sha2`, `toml`, `serde-saphyr`, `thiserror` | `pc-config/test-support`, `assembly/test-support`, `machine` |
| `testkit` | Single-device `MockBus`, scripted `MockCpu`, golden assertions | `device`, `cpu` | None |
| `platforms` | The one place that names the platforms compiled into both programs: the cores (`cores()`, the PC core, then the NES core `unemu_nes_core::NesCore` from unemunes, pinned to an exact revision, and the test core with the `test-core` feature, which no release sets; its tests check each core's contract and that core, platform and content-kind ids are unique, and `tests/nes_cartridge.rs` opens a cartridge image built from our own bytes through `core_launch` and pins its frame count and picture for the pinned unemunes revision; `tests/pc_launch.rs` starts the compiled-in PC from launch parts, boxed and typed), the image providers a transfer or listing uses (`with_image_providers`, the PC's from `pc_image::with_providers`), the push plan bound to the PC firmware catalogue (`push_plan`) and the program's licence notice | `assembly`, `image-store`, `image-registry`, `pc-image`, `unemu-core-api`, `unemu-pc-core`, `unemu-nes-core`, `unemu-test-core` (optional, `test-core`) | `core-launch`, `pc-config`, `unemu-core-api/test-support` |
| `pc-harness` | Machine runners for the PC/AT goldens and the command line: hand-encoded guest programs, key scripts, assembly/policy selection, output capture and stop diagnostics (`StopReason`). Its `tests/` hold the emulation goldens (BIOS POST, FreeDOS boots and sessions, floppy, IDE, VGA, EMS, sound, the scheduler A/B tests) and `tests/support/`, which other crates include with `#[path]` | `assembly`, `device`, `cpu`, `chipset`, `machine`, `devices`, `video`, `anyhow` | `bus`, `testkit`, `toml`, `machine-file`, `media/test-support`, `machine/reference-scheduler`, `assembly/reference-scheduler`, `assembly/test-support` |
| `cli-core` | The platform-neutral command line ([core contract](design/core-contract.md) §7.1, §7.2): the top-level parser, the usage text, the exit codes, the commands that need no emulator (image listing, sweep, compress, workspaces, media and firmware listing and show, push, pull, remotes, login, registry settings, `recipe check` and `show`, the journal tools) and the provider seam (`CliPlatform`, `PlatformCommand`) through which each platform adds its commands. Its `tests/` hold the journal tools' tests, run without a platform | `image-store`, `image-registry`, `media`, `recipe`, `anyhow`, `toml`, `libc` (Unix) | `image-registry/test-support` |
| `cli-pc` | The PC's `CliPlatform`: every PC command (`--boot`, `--bios`, `machine check`, `run`, image import, run and publish, the machine commands, media and firmware import, firmware edit, template build, `media create`) and the PC's answers to the neutral commands; `--boot`, `--bios` and `run` call the runners of `pc-harness`. `cli_pc::command` is the command line with this platform alone. Its `tests/` hold the PC command tests and the goldens that need the command line or the store | `cli-core`, `pc-harness`, `pc-image`, `image-store`, `image-registry`, `recipe`, `machine-file`, `assembly`, `device`, `machine`, `media`, `video`, `anyhow`, `toml` | `cpu`, `devices`, `assembly/test-support`, `media/test-support`, `image-store/test-support`, `image-registry/test-support` |
| `upcli` | The `upcli` binary: `main` with the host's paths and the platforms compiled in, the program's `--version` and `--about` (with a line per compiled-in core from `core_launch::Cores`), and `upcli::command`, the composed command line. Its `tests/` hold the tests that run the binary and the pins of the usage text, of the About text and of every documented command line | `cli-core`, `cli-pc`, `platforms`, `core-launch`, `image-store`, `assembly`, `devices` | `pc-harness`, `pc-image`, `cpu`, `machine`, `image-registry/test-support` |

Each crate names its layer in its manifest (`[package.metadata.unemu] layer`),
and `scripts/crate-boundaries.sh` keeps the layers apart
([core contract](design/core-contract.md) §7.3, §7.4):

| Layer | Crates | May depend on |
| --- | --- | --- |
| `contract` | none here: the contract, `unemu-core-api`, is in its own repository, whose CI keeps it free of dependencies | nothing at all |
| `neutral` | `media`, `image-store`, `image-registry`, `recipe`, `ui-kit`, `machine-view`, `library-model`, `library-ui`, `cli-core`, `unemu-test-core` (the contract alone), `core-launch`, `player-ui` | `contract`, `neutral` |
| `emulation` | `device`, `bus`, `cpu`, `video`, `audio`, `devices`, `chipset`, `machine`, `testkit`, `assembly`, `unemu-pc-core`, `pc-config` | `contract`, `emulation` |
| `pc` | `machine-file`, `pc-image`, `pc-harness`, `cli-pc`, `pc-media`, `pc-library`, `media-ui`, `pc-ui`, `pc-session-tests` (tests only) | `contract`, `neutral`, `emulation`, `pc` |
| `application` | `upcli`, `window`, `platforms` | anything |

The rules cover test dependencies too, and no window crate depends on a CLI
crate or the other way round. The desktop takes what it shares with the
command line from the libraries that own it: registry clients from
`image_registry::accounts`, the preferences path from
`image_store::location::preferences`, image creation from
`pc_image::media_create`, and from `platforms` the provider list (whose PC
part, `pc_image::with_providers`, the command line's PC platform holds too),
the licence notice and the push plan, which binds
`image_registry::push::plan_with_catalogue` to the PC firmware catalogue
(`pc_image::firmware::restricted`) so neither program can plan a push
without it. The one known crossing, `assembly` → `media` for the journals the
running machine writes, is listed in `scripts/crate-boundaries.allow`.
The same script holds the resolved build to one copy of the contract: exactly
one package named `unemu-core-api`, else it prints each copy's source and the
crates that use it.

Concrete hardware selection lives in `assembly` and the frontend. `machine`
does not depend on `chipset`, `devices`, `video`, or x86jit directly. `Bus` is a
concrete router over `dyn Device`, not a trait; the config-derived decode map is
its routing authority. Concrete devices do not depend on the runtime bus.

`image-store` takes a caller-selected local root and owns `store.toml`, the
OCI layout under `images/`, immutable blobs, staging files and stable advisory
locks. It has no emulator dependencies. `ConfigValidator` providers validate
platform-owned config schemas and enumerate their complete ordered payload
closure, including transitive disk references when supported. The store checks
routing-platform agreement, metadata/artwork references, descriptor limits,
exact closure order and every blob hash before publishing a tag. Providers
declare extra payload media types and size limits; shared code has no platform
payload allowlist. Resolution reports missing artwork without blocking required
payloads, while publication still requires the full artwork closure. Foreign OCI
index entries and optional fields survive local tag updates. Canonical
manifest bytes identify the image; config bytes independently identify the
machine. Reads never fall back to source paths. See
[machine images](design/machine-images.md), §§1–3 and §5.

The `metadata` module owns the deterministic schema-1 catalogue codec and strict
publication validation. Resolution degrades damaged metadata to read-only display
projections; newer schemas deserialize known fields without repeatedly copying
large title objects. Launch objects remain provider-defined, without compilation.
The `library` module stores source-image/title-id edits, personal fields and
local labels/favourites/last-run in `local/library.toml`. Edit callbacks run outside
the index lock, followed by content compare-and-swap under that lock. Conflicts
require a fresh edit; re-entrant store calls cannot deadlock the short index lock.
Corrupt/newer library files stay untouched and reject writes. Content hashes of
local sections invalidate summaries even after a backup restores old counters.
Publication verifies the source closure once, verifies new artwork, writes only
catalogue metadata and a new manifest/tag, then consumes edits and moves personal
fields, local labels and favourites to the new image by stable title id. The
config (machine) digest stays fixed; provenance points to the source image.
Empty/personal-only overlays reuse exact source bytes. Artwork descriptors are
sanitized; external/embedded data is rejected. Only catalogue/artwork overlays
root source images for GC; unchanged or empty local entries are not written.
The `summary` module opens using only index/library/cache files. A damaged local
library still permits read-only listing without local data and reports its error.
Bounded lazy batches attempt at most 64 images, with visible references first;
per-image error rows prevent damaged images starving later entries. Display-only
manifest decoding accepts unknown provider payload types without authorizing
execution. Unknown overlay title ids are skipped with warnings; publication rejects
them. Writers merge on-disk rows under the index lock. Capacity pressure drops
search text before replacing oversized facet rows with compact reported statuses.
Per-title facets and token-prefix search cover catalogue text, opaque launch
values, notes and combined labels, excluding artwork digests and numeric ratings.
Lowercase and a small Latin accent table normalize text; CJK character/bigram
tokens support incremental matching. Facets never combine different titles;
results group under repository:tag. Corrupt caches rebuild; warm queries use
summaries alone. Cache identities/sizes never replace full Start verification.
See [machine images](design/machine-images.md), §§2.1, 3.1 and 7.

Blob ingestion streams through a fixed 64 KiB buffer. Linux/macOS no-clobber
rename publishes synced blobs without replacing existing inodes; tag updates
reread the index under a short lock and atomically rewrite it after a digest
compare-and-swap. Stable kernel locks release on process exit, so leftover lock
files are reused. Locality checks and a lock probe reject unsupported roots;
Linux currently accepts ext-family, tmpfs, Btrfs, XFS, ZFS, F2FS and overlayfs;
opening also probes no-clobber publication. macOS uses
the filesystem's local flag, and other hosts are refused. This crate does not
implement PC machine-file import, disk reconstruction,
registry transfer, quarantine/damage tracking or GC. Future GC must take the
exclusive store lease before deleting any objects.

Installation media sets reuse the OCI envelope, closure validation and tag CAS,
with a neutral schema-1 config in `image-store::media_set`. Ordered stable media
ids, labels and provider-owned format tables define insertion order. Extras carry
original names, lengths, digests, kinds and optional validated DOS 8.3 names.
`MediaFormats` delegates exact payload format/size checks to the provider;
`pc-image::media_set` declares four floppy geometries and storage-only ISO-2048.
Summary cache version 3 distinguishes machines from media sets and retains the
same metadata/title facet projection for both. `image media` imports sources by
streaming full hashes, deduplicates descriptors and publishes only verified sets.

The `media_dialog` model (`crates/pc-media`) resolves a pinned set descriptor, checks every
format before modifying a catalogue, and submits entries through
`AssembledMachine::append_floppy`. It queues the initial selection through the
existing floppy event path. Journal paths belong to the receiving machine's
local sidecar or the owned image workspace's media directory; immutable blob
storage is never writable through a journal; bases remain SHA-256-pinned files accessed through
the existing sector overlay boundary. The shared location helper supplies platform data-directory defaults and workspace
media directories; Preferences can override the root. Background jobs load summaries
and verify each unique payload once, passing captured file receipts through assembly.
Damaged rows carry errors; try-locks report store contention. Batch appends roll back
controller entries, persistence handles, config and labels on any failure. Catalogue
provenance reuses existing disks and exposes their set/medium digests as GC roots. Extras are stored and inspectable but not mounted.

`image-store::codec` owns the `+zstd` payload codec: streaming, single-threaded
zstd level 3 through fixed 64 KiB buffers, frame checksum and pledged content
size. Providers declare which payload types have a `+zstd` variant
(`ConfigValidator::payload_types`; `codec::MEDIA_PAYLOAD_TYPES` for media sets
and `codec::FIRMWARE_PAYLOAD_TYPES` for firmware packages), so the shared store
never infers compressibility from a type name. `codec::describe` computes the
descriptor an import would store without writing, which firmware packages use
to know their identity before publication. OCI
digests and sizes describe stored bytes; `unemu.decoded.digest` and
`unemu.decoded.size` bind the raw identity that config references, layer
parents and journals use, so recompression changes blob and manifest digests
but not the machine digest. `Descriptor::names` and `decoded_*` give providers
one way to match a config reference to a raw or compressed descriptor.
`ResolvedImage` keeps one receipt per stored digest plus a decoded-content
index; receipt reuse must prove both identities. A receipt binds to the stored
blob: resolution (import, publication checks, `image compress`, Start, Insert)
hashes the stored bytes and streams the decoded bytes through SHA-256, writing
nothing. `content_reader` streams verified decoded content. Only a request for a
file of random-access media, `verified_file` or `Store::resolved_file_path` with
the content digest a config names, decodes a compressed payload into
`cache/materialized/<decoded-sha256>`, once per receipt, under the same
per-digest lock as flattened chain heads. An existing single-link file whose
SHA-256 matches is reused; anything else at that path is removed first, then
the payload is decoded to `<hex>.decoding` after a free-space check of its size
plus 256 MiB, verified and published without clobbering.
`Store::sweep_materialized`, under the exclusive store lease, removes decoded
files no tag or pinned image reaches, plus lock and partial-decode leftovers;
flattened heads stay. Decoding caps output at the declared size and the window
at 8 MiB. Failed ingest or decode publishes nothing.

`pc-image` mounts only what a machine opens as a file. Provider validation
(`disk::check`) writes nothing: a layered head is rebuilt in one streamed pass
and compared with its recorded result. Start (`disk::bases`) materializes a
compressed hard-disk base and flattens a layered head; the head streams its
base and sector packs through content readers, so neither is decoded to a
file. A compressed floppy decodes into memory: read-only and volatile drives
get its bytes, a journal drive a never-created `cache/in-memory/<hex>` key in
the verified-base map, and `SessionTransfer` carries those bytes to the
execution thread. Firmware is read with bounded in-memory decoding. Disk reads
and writes still go through the existing `SectorSource` overlay chain;
compression changes only the storage representation of an immutable base.
Store-first firmware resolution names a raw package ROM by its blob path and
decodes a compressed one into `PreparedMachineFile`, under a never-created
`cache/in-memory/<hex>` key that the launch reader, folder discovery and
machine-image import read from memory. `machine-file` knows no store: it asks a
`machine_file::FirmwarePackages` lookup, and `pc-image::firmware::StorePackages`
is that lookup over the store (`pc-image/src/firmware_package.rs`), with
`pc_image::firmware::prepare_for_assembly_with_store_root` and
`prepare_for_assembly_with_firmware_store` for the callers that hold a store
root or a store ([core contract](design/core-contract.md) §9 item 3).

`image-registry` moves stored blobs unchanged: push verifies the closure by
streaming and uploads the compressed frames, pull bounds descriptors with the
provider's payload types and imports by stored digest. The guard reads
`unemu.redistribution` on stored descriptors and keys a compressed descriptor
by both its stored and decoded digest.

`pc-image` is the PC-side provider: it depends on `machine-file`, `pc-config`
(the stored config's model and codec), `assembly` and the shared store, with no reverse dependency from `image-store`. Import first
uses the existing extends resolver and origin-aware loader, then projects schema-1
hardware into the bundle profile and replaces host paths with logical asset
bindings. Complete firmware containers/chips, every catalogue medium and CMOS
become verified blobs. The provider validates media descriptors and geometry without reading their
payloads into memory; bounded placeholders let the existing schema/lowering
validate the hardware, followed by file-backed bindings and final assembly
validation. Resolution retains full SHA-256 pins for assembly.
Imports materialize lowering defaults into immutable schema-1 hardware. Source directories are
never a runtime fallback. Resolution puts persistent outputs under a caller-owned
session directory; immutable bases stay in the store. Journal snapshots use an existing shared source lock when present and never
create one in the source folder. Private recovery reuses the scanner and streams
the resulting overlay directly into blob ingestion. Abandoned `pc-import-v1-*`
staging directories are swept under an exclusive importer lock. Local records
under `local/imports/<image-digest>.toml` report each medium policy and independent
recovery choice; they are outside image identity and do not claim a synchronized
machine snapshot. Firmware-root discovery and origin paths remain machine-file
APIs, with exhaustive projection views guarding schema evolution. Fixed-disk layers use immutable TOML descriptors plus sorted LBA/sector packs in
`media::layer`; its `Chain` merges an LBA index behind `SectorSource` and streams
logical SHA-256 verification. Providers enumerate head, pack and parent closure
in deterministic order. Floppies and CMOS always freeze whole. For existing v1
journals, a layered head materializes once in `cache/materialized/<result-sha256>`;
every Start hashes this shared raw file again. Controllers keep their existing
media adapters.

`image-store::workspace` holds a shared store lease and a stable exclusive
workspace lock across guest execution and journal finish, rejecting a second
owner in the same process too. Mutable sessions and private media journals live
under `workspaces/<name>/`, outside image blobs. `pc-image::workspace` pins the
image descriptor and config digest, resolves stopped journal heads, publishes a
complete new manifest, writes a durable intent, and performs a tag CAS. It then
retires old journals, updates the session pointer and removes the intent. Retry
is idempotent while the intent exists; Run refuses pending transactions. Fault
hooks cover each publication operation. Tag deletion changes only index metadata;
shared blobs and workspace roots survive. GC is not implemented.

`image-registry` moves tagged images between the store and OCI distribution
registries without emulator code; callers pass their `ConfigValidator`
providers, and the CLI passes the PC machine and media-set providers.
References follow Docker's rules against `registry.unemu.dev` (`name:tag` is
`unemu/name:tag`), with the default overridable by `--registry`,
`UNEMU_REGISTRY` and the preferences `default_registry`. The `registries` list
in the shared preferences file holds host, `private` flag, user name and a
loopback-only `plain_http` opt-in, never secrets. Secrets come from
`UNEMU_REGISTRY_USER`/`UNEMU_REGISTRY_PASSWORD`, which apply to one host only
(`UNEMU_REGISTRY_HOST`, else `registry.unemu.dev`), else from the macOS Keychain
through `/usr/bin/security` (service `unemu-registry:<host>`), never through the
Keychain API. `image login` and the desktop shell update the preferences file
under a shared sibling lock, each re-reading it and changing only its own keys.
TLS (rustls with ring, operating-system roots) is mandatory except for loopback
test registries; requests start anonymous and answer a `401` Basic challenge
once, then reuse the credentials. The client follows redirects itself and sends
`Authorization` only to the registry's own scheme, host and port; registry URLs
(`Location`, `Link`) resolve against the request URL and must be HTTPS, or plain
HTTP on loopback. Errors never show URL query strings. Bearer realms are parsed
and refused until implemented.

Push runs the guard first: every closure blob gets its effective class, the most
restrictive `unemu.redistribution` mark that any tagged local manifest records
for its digest (absent means user-supplied). User-supplied content requires a
registry entry marked private and a repository outside `unemu/`, pushes into
`unemu/` require all-open closures, and unlisted registries are refused. These
checks need no network. `PushPlan` is read-only and `push` runs the checks again
on the plan's target and closure. The store then verifies the closure,
restricted pushes probe anonymous `tags/list` and catalog reads (readable, or a
Bearer challenge that cannot show privacy, refuses), and the remote tag is
checked: version tags never move and other tags move only with `--move`. Each
blob is `HEAD`ed and only missing ones upload: single `PUT` up to one chunk,
otherwise chunked `PATCH` sessions (64 MiB default, 8 MiB floor) whose
`Location` is persisted under `local/registry-uploads/` after every accepted
chunk and which resume from the registry's reported offset, restarting when the
session is gone or the registry refuses the offset it reported. A blob already
present drops its persisted session. Cross-repository mounts exist in the
library API (`PushOptions::mount_from`); the CLI does not try them yet. Every
blob is `HEAD`ed again, the tag is rechecked, and the store's canonical manifest
bytes are `PUT` last. Pull fetches the manifest by tag or digest and checks its
digest, bounds descriptors with the provider's payload types, streams missing
blobs (four in parallel) through `Store::import` with the expected digest,
continuing an interrupted body with up to three `Range` requests into the same
staged file and hash (a whole-blob `200` restarts the blob), imports the
manifest and sets the local tag by compare-and-swap after full closure
verification, recording the source registry on it (`unemu.pull.registry`). The
local tag is short from `registry.unemu.dev` and carries the registry host from
any other registry (`Reference::local_tag`). A local tag naming another digest
is never rewritten. Listings
pass `n` and follow `Link` headers. An in-process mock registry (`test-support`)
covers these paths over loopback HTTP.

Chain traversal and the materialized cache currently live in `pc-image::disk`
and `media`. They would move into a shared module when a second platform provider
needs them. The store remains independent of PC hardware; workspace discard
uses generic sidecar locking across provider-selected directories.

Resolution retains verified open-file receipts and the captured provider result.
Raw disks bypass chain reconstruction. A layered head hashes its logical result
once during materialization; warm starts verify the cache once and reuse it.
A codec receipt binds reuse to the exact head, so a different chain claiming an
already cached result must still prove its reconstructed bytes. Large range reads and buffered sealing avoid per-sector filesystem calls.
Publication pre-verifies metadata/artwork, rolls back intent on CAS conflict,
and records local retention before the tag becomes visible. Explicit abandon
keeps journals. Discard keeps a durable tombstone until all media are removed;
the session document is removed last.

Firmware package envelopes and bindings live in `image-store::firmware`; attachments
reuse `media_set::Extra` and its shared validation. Packages have no machine digest
or machine-metadata layer. The PC provider exposes roles, fit and catalogue rules
through `pc-image::firmware`, which also resolves machine files' pins against the
store's packages with those rules through `machine-file`'s `FirmwarePackages`
seam, so the loader depends on no store. Host import
uses the persistent ROM hash cache through `assembly::firmware_discovery` and
streams selected complete sets into content-addressed blobs before CAS tagging.

Frontends explicitly pass their store root (desktop preferences or its injected
default; CLI `run --store` or the shared location, with `--no-store` available).
Plain machine-file library loaders never discover host store/index paths.
Preselection store errors warn and fall back to ordinary file resolution.
Machine-file preparation queries rebuildable firmware summaries by complete pins,
then verifies the selected complete package and retains its digest as provenance.
Store hits preserve declaration shape, chip order, lanes and offsets without listing ROM folders;
misses retain declared-path and indexed-folder fallback. Desktop readiness uses
this same preparation path. Selected-package corruption is an error. PC machine
import embeds the ROM closure independently and writes `unemu.firmware.sources`;
execution never needs the source package tag. Firmware summary rows index roles,
boards/classes and catalogue ids, and warm listing reads no ROM payloads.

## From configuration to a run

`machine_file::load` reads a complete TOML or YAML machine description, expands the
assembly-owned board data and card defaults, resolves images against their
defining files, checks optional SHA-256 pins, and retains the bytes inline.
It derives the RAM mosaic and calls `MachineConfig::validate()` before returning.
The reference file is `machines/at-freedos.toml`; frontends still use Rust presets.

`assembly::descriptors` owns the implemented card names, video decodes, RAM
and resource domains, effective defaults, named floppy geometries and firmware
slots. EMS domains reference the device model's public jumper/population
constants, so the runtime constructor and configuration cannot drift.
Machine-file parsing/lowering consumes these definitions. The
transport-neutral `assembly::capabilities` API describes fields, pages large
integer domains, offers choices and validates a lowered `Draft` with atomic
pending edits. Paths use card identities (`card[sound].irq`) and drive units,
with optional source-level card names and image-field paths supplied by the
caller. Image paths exclude the system BIOS when numbering option ROMs.
Port descriptors expose IRQ dependencies; edits follow a changed port's derived
IRQ default when the old default is still selected, preserving explicit IRQ edits.
Choices retain unrelated existing errors as diagnostics without disabling other
fields, so staged resource swaps remain editable. Responses echo
the caller's revision so a frontend can discard stale choices.

`validate_draft` shares the ordered checks behind `MachineConfig::validate`,
collecting one error per independent check. Diagnostics include stable codes,
field paths, conflict partners, decoded ports/routed IRQs/DMA ownership and
severity. Candidate edits replace existing resources and rebuild derived
port/MMIO/RAM maps; several fields are applied before checking conflicts.
Selected firmware uses catalogue requirements and sizes for placement checks,
while absent bytes remain pending. The known June 1985 IBM BIOS drive-type
restriction is exposed to drafts; legacy lowering's first-error behavior and
source-origin mapping remain unchanged. No host image reads, UI types or
execution clocks enter the API. Launch must still load/verify inputs and run
assembly validation, including persistent-path checks. `Validation::ready`
means no reported error or pending check, not permission to bypass assembly.

Host firmware discovery lives in `assembly::firmware_discovery`. Its `frontend`
module reuses one listing for the union of BIOS and option-ROM pins in a check, enforces the same
size bounds on declared paths, and hashes only eligible files. The `persistent`
module supplies a scoped host-worker session to existing machine-file resolver
calls: an explicit cache path, metadata-validated SHA-256 index, cancellation flag
and bounded progress/result snapshots. No guest configuration or execution state
contains cache settings. Tests explicitly disable persistence or use temporary
cache paths. The window readiness worker owns the session; the UI polls snapshots
without doing filesystem I/O or waiting for the worker to join. Non-racy cache hits avoid
candidate file reads, while the ordinary loader still verifies the selected ROM
bytes before launch. See [machine descriptions](../../machines/README.md#selecting-a-machine)
for local cache locations, privacy and Rescan/Cancel behavior.

The normal real-mode path is:

```text
MachineConfig (preset or caller data)
  -> assemble(&config)
  -> AssembledMachine { Machine<X86JitCpu>, device_index, observation }
  -> RunPolicy + assembly::run(&mut machine, &observation, &policy)
  -> Machine::run_bounded(...) -> output capture / stop classification
```

[`MachineConfig`](../../crates/assembly/src/machine.rs) describes CPU/reset state,
clock rate, RAM/MMIO layout, devices, drives and loadable images.
[`assemble()`](../../crates/assembly/src/assemble.rs) validates
it before construction, maps CPU memory, loads images, creates devices in config
order, derives CMOS contents from memory/drives, and resolves device roles into
bus indices. It builds the decode map, IRQ lines and DMA couplings. One shared
master/slave PIC pair backs both bus faces and the interrupt-controller seam;
a configuration without a PIC receives a null controller.

[`RunPolicy`](../../crates/assembly/src/run.rs) describes how to drive that machine:
virtual-time cap, halted idle limit, natural stop or console-prompt polling, and
keyboard stimuli after the prompt. `run()` applies the idle limit and drives
`run_bounded`; prompt polling captures a bound `DeviceId` display route through
`ConsoleProbe` and the observation facade. Policy cadence affects guest execution and golden identity.
The assembled machine starts without an idle wait. Some simple runners call
`Machine::run()` directly. The assembler constructs Real16 machines and rejects a CPU without step-bounded
execution. The Long64 machine demo has been intentionally removed.

### Current execution order

The authoritative loop is
[`Machine::run_bounded`](../../crates/machine/src/lib.rs). For a running CPU:

1. Settle instruction effects, check the absolute virtual-time cap, then
   acknowledge a serviceable PIC request only if `Cpu::irq_ready()` permits it.
2. Bound `Cpu::run_steps` by the next device deadline, remaining cap and host
   cooperation limit. A pending INTR requests a stop at the first eligible
   boundary, respecting IF and STI/SS-load inhibition.
3. Charge completed prefix steps, dispatch the stopping port/MMIO access at
   instruction-start time, and finish pending read/write handshakes. Each REP
   iteration is a step, so long string instructions advance device time.
4. Charge the completed stopping step once, settle bus/DMA effects and sample
   wired IRQ transitions before the next acceptance check. Errors retain the
   completed prefix's device time. Exact active caps win over a new INTA.

Production uses deadline dispatch. Devices without a deadline implementation
retain per-clock visits, while scheduled devices synchronize lazily on access.
The non-default `reference-scheduler` feature exposes per-clock dispatch to
integration tests as an oracle with the same CPU/IRQ rules. No production
`MachineConfig` selects it, and no legacy outer loop remains.

[`Bus`](../../crates/bus/src/lib.rs) dispatches port cycles according to each
window's `PortWidth`: `Byte` by default, or `Word` for IOCS16# devices. An even
port in a word window receives one size-2 call for a word access. Width is
selected by the cycle's starting port, so a single-port word window consumes
both data bytes even if the next port decodes to a byte device. Odd word accesses
use two byte cycles. An aligned 32-bit access to word windows uses two size-2 cycles at
port and port+2; each new cycle independently selects its window and width.
Mixed windows therefore use byte or word cycles as selected, low bytes first,
with port arithmetic wrapping at 16 bits. A fault preserves completed cycles.
Device traces record those actual cycle sizes. This models the IBM Personal
Computer AT Technical Reference, Section 1, System Board, I/O Channel,
I/O Channel Signal Description, -I/O CS16 (IOCS16#); wait states and physical
byte-lane signalling remain outside the model. Existing presets retain byte
windows. Assembly checks word windows against a kind-based register support
table, currently empty; ATA card wiring will declare its word data register.
Port dispatch resolves the device and cycle width in one first-match lookup.
See [Port window width](design/machine-config.md#port-window-width).
Undecoded ports default to
all-ones reads/dropped writes; optional bounded tracing and strict faulting are
selectable. Assembly coalesces touching MMIO windows into one CPU trap region
while retaining per-device bus decoding. MMIO accesses contained in one decode
window reach the device once with their original size. Accesses crossing windows
are split into contiguous sub-accesses in ascending address order, with
little-endian read composition and write slicing. An undecoded MMIO address
faults immediately: earlier sub-accesses remain completed and later ones are not
attempted. Read results use `complete_port_in` or
`complete_mmio_read`; trapped writes require `complete_mmio_write` so retrying
the instruction does not repeat the device side effect.

While halted, the loop stops immediately with IF clear (NMI is not modelled).
With IF set it first tries IRQ service; otherwise, subject to idle/cap bounds,
it jumps to the next device event, idle expiry or cap, settles DMA and samples
edges before retrying IRQ acceptance. A queued IRQ permits CPU re-entry. Halt state and accumulated idle time
survive bounded runner calls.

## Contracts and ownership

- **[`Cpu`](../../crates/cpu/src/lib.rs)** exposes `run_steps`, completed-step
  and whole-instruction counters, general-purpose registers/RIP, port/MMIO
  completions, IF and IRQ readiness, IRQ injection/pending status and DMA byte
  access. It has no segment-register API,
  bus-binding API or `irq_ack()` method. Segment seeds and `code_segment()` are
  concrete `X86JitCpu` setup/inspection APIs.
- **[`Device`](../../crates/device/src/lib.rs)** supplies port/MMIO access,
  `tick(VClock)`, IRQ level queries, reset, per-device save/restore, trace
  drains and optional DMA request/service/completion hooks. Host-side
  `as_any_mut` is an optional downcast for input/output glue, not execution-loop
  hardware selection. `NextEvent::{EveryPitClock, At(VClock), Dormant}` is an
  scheduling contract consulted by the production deadline loop; `next_event`
  defaults to `EveryPitClock` for unmigrated devices. Timestamped access wrappers
  synchronize the target device at instruction start; deadline dispatch settles
  due transitions without ticking dormant devices at each CPU boundary.
- **[`SectorMedia`](../../crates/device/src/media.rs)** is owned by each uPD765
  drive and covers its configured geometry. It serves 512-byte sectors, accepts
  one ordered batch of replacements per nonempty completed WRITE DATA, and
  reports write protection that remains constant while mounted. Partial sectors
  are zero-filled by the controller; abnormal completions include sectors already
  committed. The contract exposes no clock or host error result. Default drives
  use `InMemoryMedia`; reset retains the medium and device snapshots exclude it.
  The host-side `media::JournalMedia` owns reconstructed bytes, synchronous
  per-command output, recovery and checkpoint/finish under a stable sidecar lock.
  Its shared `SinkStatus` latches the first failure outside guest-visible state;
  reads and later memory writes continue after output stops. Assembly adapts
  journals to `SectorMedia` and exposes per-drive persistence handles. CLI and
  window frontends poll status at run boundaries and explicitly finish normal
  sessions; execution errors leave the tip unsealed.
  See [persistent media](design/persistent-media.md#integration-layers-flush-points-determinism-argument).
- **`InterruptController`**, defined in `device`, supplies `set_irq_line`,
  non-mutating `poll`, `acknowledge`, and `end_of_interrupt`. Machine wiring
  forwards rising and falling device pin levels to PIC inputs; the PIC detects
  edges and withdraws unacknowledged requests when their inputs fall.
  Acknowledgement requires CPU IRQ readiness and no already queued vector,
  after the stopping instruction's bus/DMA effects have settled. Guest EOI port
  writes reach the same shared PIC state. See the adopted ordering in
  [scheduler design](design/scheduler.md#adoption).
  Known limitation: cascaded-board validation does not reserve master IRQ2 for
  slave INT, so a device route can still conflict with that internal connection.
- **[`FrameSink`](../../crates/video/src/lib.rs)** consumes `&Frame` via
  `present`; the current frame is `Frame::Text(TextGrid)`. Rendering turns VRAM
  and display-start/cursor state into cells with glyph bytes and attributes.
  `CaptureFrameSink` retains the last frame. Presentation is driven by the assembly observation facade at capture
  boundaries, outside the machine loop.

The bus owns devices; `X86JitCpu` owns guest memory. Devices do not retain RAM
references. DMA requesters supply a channel and payload/length, the configured
controller supplies addresses/counts, and the machine lends a temporary
`DmaMemory` adapter over the CPU seam. `DmaOutcome` returns a transfer count or
the bytes read, truncated to the controller's actual transfer length.

### Snapshot clock and interrupt state

Device restore keeps the current trace history, drop counts and recording policy.
A restored bus clock must be at least as late as each device snapshot's absolute
clock; devices ignore earlier sync/tick times to avoid rewinding or replaying an
interval. Restoring an earlier epoch requires restoring its global clock too.

A future machine-level snapshot must also save `Machine::last_levels`, the IRQ
IRQ sampler's last observed levels. No device snapshot contains these samples;
losing them can suppress or invent an interrupt edge after restore. CPU state,
the global clock and its fractional remainder, halted state and idle counters
also belong to that machine-level snapshot.

### Observation and presentation

The optional `window` crate remains a leaf frontend. Its `unemupc`
binary uses egui/eframe 0.37 with the glow renderer and the existing winit 0.30
event loop. One client area switches between the machine library and a single
nearest-filtered guest texture fitted at 4:3. Classic grey controls and a blue
gradient title strip are the default; macOS uses its native title bar without
an additional custom strip. Toolbar icons are painted geometry, independent of
font coverage. View selects modern controls and the system, light or dark theme.
Menus stay inside the window on every platform.

`src/shell_model.rs` owns hotkey mapping, the status-bar widths and pixel
conversion; discovery is `pc_library::machine_files` and command enablement
`pc_ui::commands`, which it re-exports. Discovery includes TOML/YAML files directly in `machines/`
and `machines/local/`, with stable ordering and blank last-run metadata. The
existing machine-file loader and read-only checker supply hardware details and
readiness; missing firmware or invalid media disables Start and exposes the
validator diagnostic. Open accepts a machine-file path. The Settings sheet edits
machine files; Locate firmware and Preferences share a ROM-folder list. Save
state, new-machine creation and CD-ROM remain disabled or placeholders.
The Insert dialog reads media sets through the image-store summary and PC
format provider.

`crates/pc-ui/src/library_view.rs` owns the Explorer tree, summary queries, repository/tag
groups, fixed-column list and selected-image details; its lists draw with the
tables of `library_ui::table`, and Remotes is `library_ui::remotes`. It builds
widgets only for visible rows. `crates/pc-ui/src/library_view/firmware.rs` owns the Firmware collection: package
rows, facets and the chip-layout pane from summaries, the Import ROM folder…
review dialog, and the worker-side scan and import functions. The library worker
(`crates/pc-library/src/library_worker.rs`, re-exported as the window's
`library_worker`) owns host jobs, session kinds and durable recovery records. One host worker owns
the store and summary cache, sends bounded snapshot messages, rebuilds at most
eight pending summaries per batch, and performs image readiness checks, imports,
overlay edits, tag deletion, and the ROM-folder scan and firmware-package import
jobs (which report failures only to the import dialog).
Warm startup opens only index/library/cache files; query and sort do not read
manifests. Full descriptor verification and disk hashing remain mandatory at
Start. Platform data-directory selection and the optional `store_root` preference
use the shared `image_store::location` policy and the unemu family data root.

Image launches use `pc_image::workspace::prepare_pinned`, so a moved tag cannot
change the selected image. The worker holds workspace ownership through guest
execution and persistence finish. `SessionTransfer` carries verified open files
across threads; execution-local sector readers are reconstructed without another
payload read. Start reuses regular workspaces. Gaming mode creates isolated
disposable workspaces, including catalogue media and CMOS. Durable desktop records
precede preparation; stop/exit offers Keep as new image or Discard. Successful
publication removes a disposable workspace. Failures and deferred/forced exits
retain it for the startup recovery list. Worker shutdown drains notifications and
waits a bounded interval for queued finish operations. Declared read-only/volatile
policies remain intact.
Workspace discovery follows the pinned identity after publication rebases a
workspace. Only local last-run metadata uses a host timestamp.

`crates/pc-ui/src/layers_dialog.rs` owns tag entry and affected-media confirmation.
`pc_image::desktop` supplies cancellable descriptor-only readiness and base-to-head
chains without hashing or materializing disks. Pending checks coalesce and yield
to queries, edits and launches. The summary cache supplies distinct/shared storage
without reading other manifests. The dialog shows stored/exclusive/shared
bytes, firmware, catalogues and working-sector counts. Save/New/Discard first
stop and finish the presenter, then queue the existing offline publication or
discard operation. New images optionally flatten disk chains. Deleting a tag
uses digest compare-and-swap and never removes blobs. Machine-file rows explain
that layers require an imported image.

`assembly::firmware_discovery` is a host-only API taking explicit search roots.
It locates single ROMs or interleaved chips by full-file SHA-256 and optional
catalogue length, preferring valid declared paths. Its process cache includes
canonical path, size, modification time, Unix change time and file identity;
fresh metadata is checked on hits, and Windows always rehashes. Catalogue pins
supply exact image/chip lengths. Unknown pins scan only 512-byte ROM blocks from
512 bytes through 1 MiB (or an exact declared length up to 1 MiB). Frontend scans
stay inside each canonical root, skip unusable roots/entries with origin-labelled
warnings and keep searching; the required-ROM test lane retains strict failures. The existing user-ROM test entry points are thin wrappers using
the vendor directory and `UNEMUPC_USER_ROMS_DIR`; they find the source tree at
run time from the `CARGO_MANIFEST_DIR` that `cargo test` sets, so no build path
is compiled into a binary; the lane exists only with `test-support`. The
desktop's default roots are `UNEMUPC_USER_ROMS_DIR` and, when cargo started
it, the tree's `vendor/user-roms`. No discovery runs while the guest executes.

`machine_file::load_for_assembly_with_rom_dirs` resolves missing or mismatched
firmware paths in the in-memory resolved document before ordinary lowering.
Part pins and the assembled-image pin are validated with the existing checked
interleaver; option-ROM container prefixes retain their full-file pins. Machine
files and ROM files are never rewritten or copied. The launch loader still
retains verified disk-base descriptors through assembly. CLI run/check accept
repeatable `--rom-dir`; without explicit directories they perform no discovery,
preserving ordinary path/hash diagnostics regardless of the environment.

The shell stores host ROM folders in the platform config directory's
`unemupc/preferences.toml`. On macOS this is under Library/Application Support;
on Linux it follows XDG_CONFIG_HOME, falling back to ~/.config. The desktop and
the command line both take that path from `image_store::location::preferences`, which first
copies the file from the directory named after the project's former name
when the new file does not exist yet; the old file stays. Preferences and
Locate firmware share native folder browsing (rfd) and text entry. Saves write and
sync a temporary file in the same directory, rename it, then sync the directory
on Unix. Unknown TOML fields round-trip. Save invalidates welcome readiness;
only the selected machine is checked on a single host worker, with a checking
message meanwhile. Start requests another worker check, then consumes the prepared
in-memory document without scanning on the UI thread. Stale results cannot replace
newer preferences or selections. Launch uses the same roots, also searching the environment
when set. The deterministic configuration and guest core do not store preferences.

`ui_kit::logo` (`crates/ui-kit/src/logo.rs`) embeds the CC0 brand assets, decodes the icon and the logo once
at startup, and uploads the static About texture. The file-only
`reduced_motion` preference defaults to false and stills the firmware-scan
progress animation when enabled.

`src/shell.rs` draws the shell and wraps eframe's public winit application.
The wrapper retains physical keyboard positions and ordered raw mouse motion
instead of reconstructing scan codes from text events. The machine
session is `machine_view::Session` with its pacing policy, `HostAudio` and
pointer capture, over the PC core's `PcMachine`, whose `HostKeyboard` and
`PcMouse` encode the guest's input; the window's `Presenter` adds the PC
catalogue model the media dialogs edit. Menus, dialogs and focus loss release held guest keys and
pointer capture; a screen click restores guest focus. The wrapper withholds
keyboard/IME events from egui during guest focus and pointer events during
capture, including when the host can only confine the pointer. The screen is
not an egui keyboard-focus target, and keyboard UI zoom is disabled. Modifier
changes always reach egui so a held Ctrl cannot survive into a dialog.
Queued keys retain their arrival focus, session, repeat and synthetic flags.
Start/Stop and keyboard focus transitions reset shell key state; only keys
whose makes reached the guest can produce forwarded breaks. Modifier makes
wait for a guest chord, keeping host shortcut prefixes out of the guest.
Held keys from before Start/focus gain cannot enter through OS repeats or
synthetic focus events. Only Ctrl+Alt chords remain host shortcuts during guest
focus; F1 and Ctrl+N/O/comma reach the guest. Pointer release and Pause preserve
keyboard focus. On macOS an opt-in preference (`send_system_shortcuts`) adds
`machine-view`'s `macos_keys`, an active Quartz event tap that exists only while a running,
unpaused guest has the keyboard and takes the Ctrl and function-key chords
Mission Control and other system shortcuts would claim; `machine_view::system_keys`
decides which keys and when, and the taken keys enter the same key queue as
window events, so focus checks and held-key release are unchanged. Reset queues explicit guest Ctrl/Alt/Delete makes rather than
resetting the board directly. Stop and normal window close finish only a
successful, still-running session. Execution errors remain unsealed; a halt
has already finished. A failed finish drops the session, shows its diagnostic
and retains a nonzero exit status while allowing the window to close. A later
successful Start or an actual clean Stop clears the stale error.

The session advances with `Machine::run` of the contract toward bounded
virtual-clock targets, one chunk of at most a millisecond of guest time at a
time; `PcMachine` turns each deadline into a retired-instruction cap for
`run_bounded`, delivers queued keys and pointer segments at the boundary,
performs guest-requested resets inside the run, captures the observed display
through the observation facade and rasterises it, and the session uploads the
picture it returns. No sink ticks devices. Only the binary
reads `Instant`: the existing rational pacing policy limits catch-up to 50 ms,
paints on a 60 Hz grid, and discards hidden, suspended, paused and modal time.
One pause predicate combines user pause, modal/menu state, occlusion and
minimization. Hidden logic passes retain focus-loss and key-release bookkeeping;
restoring a minimized window needs no separate occlusion event.
Audio retains its bounded cpal ring, mute and recoverable-error policy.
Neither playback nor UI repaint frequency drives virtual guest time.

```text
MDA/VGA scanout -> observation capture -> RasterSink -> CP437/graphics rasteriser
  (unemu-pc-core) -> Machine::frame 0x00RRGGBB pixels -> Session picture
  (machine-view) -> egui texture (nearest) -> eframe/glow window
```

The original VileR fonts and attribution stay embedded. About and informational
CLI output also carry egui's embedded font credits and original licence texts;
AccessKit is enabled for the shell. Integer scaling first removes the sink's
pixel-doubling factor. Rasterisation and all
input/pacing/session tests remain headless. The status bar shows power, one
chip per drive, configured instruction/PIT ratio, EMS, host audio and speed.
Drive lamps have an activity signal: after every run chunk `PcMachine` reads
`Upd765::drive_active` (the DOR-selected, motor-on drive or a pending DMA
transfer) and `Ata::activity` (BSY or DRQ), both read-only, the session takes what was busy
through `Machine::slot_states` once per paint, and the shell holds
each lamp for 200 ms of host UI time, which never reaches the machine. Headless
CLI runs do not depend on the shell and retain their existing behavior. See
`design/machine-images.md` §7.

`crates/pc-media/src/drive_chips.rs` is the pure model behind the status-bar drive chips and
the Media menu's drive labels. It builds one view per A:–D: drive from the live
media configuration, the Insert dialog's floppy slots and the FDC's inserted
medium, naming the image, format, write mode, source (machine file or image,
library media set, or a file chosen with Browse…) and short SHA-256, and owns
the lamp hold and the chip widths. The status-bar cell widths depend only on the
window width, so chips appearing or changing never move the other cells; a chip
shortens only its image name, keeping format and write mode. `src/shell.rs`
draws the chips, their tooltip and the drive menu, which reuses the Media
menu's commands.

`settings_model` resolves the selected machine's inheritance and consumes
`MachineFile::describe`, the metadata-only path through normal machine-file
lowering. It shares RTC, card, drive and ROM defaults without reading image bytes
or opening journals. Unknown extents remain pending. Atomic pending edits live in
`capabilities::Draft`; assembly owns field groups, display labels, domains and
diagnostics. Each ROM retains its document index rather than a slot-wide identity.
`settings_sheet` renders the classic tab strip, grouped controls, reasons and
local-child preview. Saves retain inherited media bindings and hash pins, require
a stopped machine and explicit overwrite consent, and select the child in the
library. Saved local children are edited in place with current text and a diff;
unique locked staging files are hidden from library scans and stale files are
cleaned before publication. Image paths are rebased lexically, retaining symlink
spellings. Firmware selections write a catalogue pin and plain filename without
host discovery on edits. Metadata descriptions cannot authorize launch: readiness
still uses ROM folders and the normal image-verifying loader. All state and save
checks are headless-testable.

`assemble_with_observation` binds requested routes by `DeviceId` and validates
capabilities before execution. `AssembledMachine::capture_text` captures display
state at a stamped machine boundary; semantic byte sinks collect UART TX and
POST independently of access tracing. `ObservationPolicy.trace` selects Off
(default) or `Ring { capacity }` for every device and the open bus. All recorders
share overwrite-oldest behavior and cumulative drop counters; draining retains
the count. Diagnostics and their policy are excluded from hardware snapshots.
Capture preserves saved device state and
does not consume traces. BIOS runners return the colour and MDA card grids,
COM1/POST streams, final floppy media and classified stop. The BIOS preset routes
`0xB0000..=0xB7FFF` and display ports `0x3B0..=0x3BB` / `0x3BF` to MDA;
printer ports `0x3BC..=0x3BE` remain undecoded. VGA retains only its colour ports,
so the cards have independent CRTC state. Prompt detection selects the MDA route
by `DeviceId` and uses the same facade capture as final capture. The facade
renders once and fans out that frame to subscribers.
`ObservationPolicy.output.frame_sink` selects Headless (the default, no subscriber)
or Capture (one last-frame sink per requested display). Presentation selection
belongs to observation setup, outside the hardware-only `MachineConfig`.
`Observation::subscribe` accepts a boxed `FrameSink` for a bound route, providing
the window presenter seam without a window dependency. `run_vga_text` selects
Capture; presentation stays outside the machine loop and adds no execution steps.

See [`design/observation.md`](design/observation.md) for the route contracts and
historical design inventory.

## Virtual clock

[Decision-5](../decisions/decision-5%20-%20Virtual%20clock%20in%20retired%20instructions.md)
sets retired instructions as the deterministic time source; block counts and
wall time do not define device time. `VClock(u64)` counts virtual ticks;
`device::clock::TICKS_PER_PIT_CLOCK` remains **1**. `VClock::from_pit` converts
PIT input clocks at unit boundaries. The `device` crate owns clock domains
because it already owns the common device-time seam; no additional crate or
runtime dependency is needed.

`device::clock::OSC` is exactly **315/22 MHz**. Named `ClockDomain` constants
cover PIT (OSC/12), CGA horizontal dots (OSC), VGA dots (25.175/28.322 MHz),
UART (1.8432 MHz), RTC (32.768 kHz), OPL3 samples (OSC/288), 48 kHz output,
and AT 12/16 MHz CPU crystals (6/8 MHz 286 after division by two). The CPU
crystals describe hardware frequencies, not the calibrated instruction rate.
A domain stores an exact ratio of virtual ticks per edge. `edges_between(a,b)`
counts edges in `(a,b]`; `edge_at(n)` rounds up once; `next_edge_after(t)` uses
the absolute epoch to retain phase, including when several fast edges share a
tick. Checked edge methods report exhaustion; duration conversion saturates.
Devices keep local reset/programming origins where the existing model anchors them.
Audio integration remains in virtual-tick areas. PIT counters advance by input
edges, while deadlines, UART/keyboard durations and the VGA clock conversion use
domains. The public `VClock` field and existing stateless VGA raster remain
unchanged by this seam (decision-22 §1a–b).
[`ClockConfig`](../../crates/assembly/src/clock.rs) supplies the positive rational
CPU rate A/B (`retired_per_pit_clock` / `retired_per_pit_clock_denominator`), in
completed steps per PIT clock. Assembly passes
the rate into `Machine::with_cpu_rate`; the default is 1/1. Running time emits
`floor((remainder + completed_steps_delta * B * ticks_per_PIT) / A)` virtual ticks and carries the remainder
across CPU exits. This is an instruction-timed approximation, not cycle timing.

[Decision-14](../decisions/decision-14%20-%20Virtual%20time%20advances%20while%20the%20CPU%20is%20halted.md)
extends time through HLT: halted clocks tick devices without retiring CPU
instructions or consuming the running clock remainder. `Machine::virtual_time`
is completed steps plus `floor(total_halted_ticks * A / (B * ticks_per_PIT))`, saturating
at `u64::MAX`. Caps and policy targets use these instruction equivalents;
device clocks use `VClock`. Golden runs depend on the CPU revision, config and
policy cadence. Host cooperation size cannot change timing by contract;
device-free CPU regressions and the per-clock-versus-deadline comparisons hold
the CPU core to it (see [scheduler adoption](design/scheduler.md#adoption)).

## x86jit integration

[`X86JitCpu`](../../crates/cpu/src/x86jit_backend.rs) adapts a pinned external
`x86jit-core`; machines use its Real16 interpreter. Long64 remains an
adapter unit-test concern only. CPU changes are developed upstream and consumed by re-pinning,
not copied into this workspace
([decision-13](../decisions/decision-13%20-%20Consume%20x86jit%20main%20at%20an%20exact%20revision.md)).

- `Exit::PortIo`, `MmioRead`, `MmioWrite`, `Hlt`, `Exception` and
  `BudgetExhausted` map to `CpuStop`. `UnmappedMemory` and `UnknownInstruction`
  become typed CPU errors; `Syscall` is unsupported. Host interruption, code
  space exhaustion and unrecognized exits become `UnexpectedExit` errors.
- The memory mosaic uses `RegionKind::Ram` (RWX), `Trap` (MMIO), and `Rom`
  (RX, executable/readable with guest stores dropped). Host image loading uses
  `Vm::write_bytes`, which can populate ROM. Real-mode `INT n` already vectors
  through the guest IVT; it is not an outstanding embedder implementation.
- The Lo-tech EMS board owns its four latches and SRAM behind `Device` and a
  64 KiB MMIO frame, and shares them with assembly through a `LotechEmsFrame`
  handle. Port writes update the latches synchronously before the next CPU
  instruction. Every 400h I/O alias is represented in the decode map and
  conflict validation. The IBM 5170's E0000h–FFFFFh system-board ROM decode
  rejects an E000h frame; the shipped profiles use D000h. Only Rust bare test
  rigs accept E000h. The normal high ROM alias is unchanged. CPU reset
  preserves latches and SRAM.
  See the coverage matrix in `crates/devices/src/lotech_ems.rs` and
  [machine fields](../../machines/README.md#lo-tech-ems-card-reference).
- The CPU cannot fetch instructions from a `Trap` region, so assembly maps
  each EMS frame window in one of three forms
  (`crates/assembly/src/ems_frame.rs`). A window whose latch selects a
  populated page that no other latch selects is `Ram`, and its CPU copy is
  authoritative. An unbacked window is `Rom` filled with FFh: reads and
  fetches see the floating bus, guest and DMA stores are dropped. Only a page
  selected by two latches stays `Trap` and reaches the board, data-correct but
  not executable. `Machine::with_memory_map` calls the map when every run
  starts and after every guest port write, before that instruction's DMA and
  the next instruction. It writes held copies back into SRAM (all of them
  before a retile), retiles the frame with `X86JitCpu::unmap` and
  `map_ram`/`map_rom_in_place`/`map_mmio` (neighbouring windows of one form
  share a region; the DMA region table follows; the core's flat backing keeps
  bytes across unmap and map), then loads newly held pages, or the FFh fill of
  a newly unbacked window, with `Vm::write_bytes`, whose `note_write` drops
  any translation of the previous contents. Its change counters advance last,
  so a failed apply is retried whole. When a run returns, held pages are
  written back, so device snapshots read current SRAM; a restore makes the
  next run drop the held copies unwritten and reload them. The core joins RAM
  and ROM regions for one operand, so a word straddling a held and an unbacked
  window reads FFh for the unbacked byte; an operand crossing a `Trap` region
  (a shared window, or another device's window beside the frame) and a held
  or unbacked window stops with `CpuError::Unmapped`. A latch
  write costs one or two 16 KiB copies; a retile, whose `Trap` map invalidates
  the whole translation cache, happens only when a window changes form.
  The write-back on every run return copies at most 64 KiB. MEASURED once in
  release on the development host: 1 ms run chunks (1,193 steps) of a spinning
  guest took 26.2–26.6 µs per return with no held window and 26.9–27.3 µs with
  four, under 1 µs and about 3% of the chunk, so held windows carry no
  per-window dirty tracking.
- `inject_irq` forwards a queued vector; the core gates delivery on IF, pending
  completions and the STI/SS-load shadows, then enters the guest IVT handler.
  `completed_steps()` drives machine time; `retired()` remains available for
  whole-instruction diagnostics.
- DMA byte access uses `Vm::read_bytes` / `write_bytes`; writes run `note_write`
  for self-modifying-code invalidation
  ([decision-6](../decisions/decision-6%20-%20Guest%20memory%20writes%20via%20note_write%20for%20SMC%20safety.md)).
  The backend remembers the regions it mapped: a DMA write into ROM is
  dropped and a DMA read from ROM returns ROM bytes, while a byte in a trapped
  MMIO window is handed back to the machine (`DmaAccess::Mmio`), which runs it
  as a bus MMIO cycle on the owning device. Read errors return `0xFF`; write
  errors are discarded by the current byte seam.

## In flight and further reading

- **Board validation:** [`design/compatibility-diagnostics.md`](design/compatibility-diagnostics.md)
  records the diagnostic-ROM spike, user-supplied firmware policy, expected
  Landmark AT coverage and the golden capture rule; execution is not qualified.
- **Scheduler:** [decision-15](../decisions/decision-15%20-%20Deterministic%20device%20scheduler%20with%20instruction-exact%20interrupt%20boundaries.md)
  amends [`design/scheduler.md`](design/scheduler.md). Instruction-boundary
  timing and deadline dispatch are adopted. Per-clock dispatch survives only as
  a feature-gated test oracle; the harness compares ordered accesses, INTA/EOI,
  RAM, registers, saved device state and stop endpoints without normalization
  between the two engines. The post-adoption profile is in
  [`design/machine-time-profile.md`](design/machine-time-profile.md).
- **Observation:** [`design/observation.md`](design/observation.md) describes
  typed device capabilities and assembly-owned observation routes. Access traces
  are opt-in, bounded and independent of semantic output; the document retains
  the historical inventory. Prompt polling, final capture and presentation now share one route-based facade.

[`design/machine-config.md`](design/machine-config.md) explains composition;
[`design/spec.md`](design/spec.md) specifies hardware coverage and seams;
[`design/machine-time-profile.md`](design/machine-time-profile.md) records timing
measurements. Historical decisions are under [`../decisions/`](../decisions/);
accepted designs may precede implementation. Deferred hardware and fidelity
work is listed in [`deferred.md`](deferred.md).
