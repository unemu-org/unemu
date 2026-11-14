# Status

Snapshot of what works, what's in flight, what's broken. **Not the roadmap** — open work lives in
beads (`bd ready --exclude-type epic`, `bd list --type epic`).

Update this when a feature lands, breaks, or gets pulled. Stale status is worse than no status.

- **2026-11-14 — No silent stop, session write modes, toolbar speed:** with a
  machine running, Machine library, Open machine file… (once a path is
  confirmed), Exit, ⌘Q/⌘W and the window's close button pause it and ask
  "Stop it and continue?" (Stop machine / Cancel); Cancel resumes without
  catch-up and restores guest focus and pointer capture. Quitting works through
  open menus and dialogs, closing them first. Gaming mode keeps its
  Keep/Discard question. Library-disk journals of image sessions live in the
  image workspace and Gaming Discard removes them. The macOS build installs no platform application menu so ⌘Q reaches
  the shell; ⌘H/⌘⌥H still hide, while Dock Quit and logout still quit at once.
  The Insert dialog fixes the write policy only for machine-file entries;
  Browse… and From library… disks switch between read-only and a journal on
  re-insert, and browsing the same file and SHA-256 reuses its row. A toolbar
  Speed combobox after Layers mirrors View › Speed, shows Unlimited during the
  fast-forward hold and re-anchors pacing like the menu. Headless tests cover
  each prompt path, the policy switch and the toolbar selection; native visual
  and Dock/menu-bar behaviour need manual checking.
- **2026-11-14 — Open VGA BIOS on the open AT profile:** `vendor/vgabios/fetch.sh` builds the unemu fork of the LGPL VGABIOS (release `v0.8a-unemu.1`, revision `8e154f04`) and its INT 10h probe floppy with Nix from the sibling `vgabios` repository, deriving the forge from this repository's origin remote, and checks both SHA-256 pins; neither file is committed. Catalogue entry `vgabios-0.8a-unemu.1-std` is an open (LGPL-2.1-or-later), **Tested** video BIOS: raw 32 KiB at C0000h, requiring the register-level `vga`. `machines/at-games-open.toml` replaces its MDA/VGA-text console with that card and ROM; XTIDE stays at C8000h. **MEASURED at CPU `fe240a56`:** Kiselev POST runs the ROM, shows its sign-on and "EGA/VGA (Video BIOS Present)", and FreeDOS reaches `A:\>` (`retired=15241264`); the probe reports all 32 checks `ok`, `fails:0000` (13 screen lines, `Halted` at `0000:880A`, `retired=10239957`); a boot sector drives mode 0Dh panning, page 1 and a CRTC byte start through INT 10h and ports, then mode 13h with a BIOS pixel and DAC entry; all four 0Dh frames match planar memory and the 13h frame matches the chain-4 byte stream and the BIOS default palette, pixel for pixel (`retired=10730615`). The CI `fixtures` gate now also builds XTIDE, the FAT16 disk and these fixtures; from CI's credential-less HTTPS origin the VGA BIOS builds only while `vgabios` is anonymously readable (MEASURED readable from the LAN), otherwise the gate fails naming `UNEMU_VGABIOS_FLAKE`. Registry delivery from `registry.unemu.dev/unemu/` is later work. No game is qualified. See [the fixture notes](../../vendor/vgabios/README.md) and [Video ROM placement](design/firmware-and-machine-classes.md#video-rom-placement).
- **2026-11-14 — Registry push and pull:** the platform-neutral `image-registry` crate pushes and pulls tagged machine images, media sets and firmware packages to OCI distribution registries, with `image push/pull/remote ls/login` CLI commands. References resolve Docker-style against `registry.unemu.dev`; preferences list registries (host, private flag, user, no secrets); secrets come from the environment (bound to one host) or the macOS Keychain via the `security` tool. TLS is mandatory except for loopback test registries; authentication is anonymous first, then Basic, and credentials never follow a redirect to another origin. Push uploads only missing blobs, chunked and resumable above one chunk, publishes the manifest last, keeps version tags immutable and moves other tags only with `--move`. A push guard classifies each blob by the most restrictive mark any local manifest records for it and refuses user-supplied content outside private registries, anything but open content in `unemu/`, unlisted registries, and targets that are anonymously readable or answer with a Bearer challenge; `--dry-run` prints each blob's effective class and where it came from. Pull resumes interrupted blob downloads by range, verifies every digest through the store import and tags only a complete closure. Mock-registry tests cover resume, dedup, ordering, auth, digest mismatch, tag rules, paging and every guard refusal; the live round trip against registry.unemu.dev is opt-in. Firmware packages (2026-11-15) travel through the same CLI commands and guard: CLI mock-registry tests push and pull the open Xi8088 package with identical manifest and blob digests, list it in `image firmware ls` with its facets and as `firmware` (no Start) in `image ls`, show a repeat pull fetching nothing, refuse a user-supplied package to a non-private registry and to `unemu/`, and show each ROM layer's effective class in `--dry-run`. An open package's ROM layers are `open`, but its config is user-supplied like every config, so it still needs a private registry outside `unemu/`. Bearer-token authentication, the GUI login and Browse registry remain separate.
- **2026-11-14 — Desktop image library and disk layers:** local-store summaries join machine files in an Explorer-style tree, virtualized details list and selected-image pane. Search, metadata facets, repository/tag groups, sorting, favorites and local labels use cached summaries. Store loading, bounded cache rebuilds, verification, imports and publication run in a background worker. Image launches pin the selected descriptor and reuse a regular workspace; Gaming mode isolates all writable media and CMOS, with explicit Keep as new image or Discard and startup recovery. Selection uses cancellable descriptor checks; full disk verification happens at Start. Disk layers exposes chains, sharing, working-sector counts, whole-machine publication/flatten and confirmed discard. Tag deletion preserves blobs. The store root follows platform data-directory conventions and can be changed under Tools. GUI acceptance remains manual; media-set browsing stays separate.
- **2026-11-14 — macOS host shortcuts:** Command chords stay in the shell,
  with no guest modifier or letter scan codes. Ctrl/Alt and Ctrl+Alt+Del remain
  guest input. Mute uses ⌘⇧M and Stop uses ⌘K, preserving ⌘M minimize and
  ⌘Q quit; ⌘W closes, ⌘H hides and ⌘Tab switches applications. Hold ⌘T for
  Unlimited; releasing either key or losing focus restores the selected speed.
  Menus and Help show macOS bindings; other platforms retain their shortcuts.
  Headless tests inject either platform and cover actions, guest bytes, repeats
  and focus loss. Native visual acceptance remains manual.
- **2026-11-14 — Disk publication review fixes:** failed CAS releases publication intent; strict artwork/metadata checks precede intent, and abandon preserves journals. Starts enforce pinned tags and fresh-session requests, retain one raw verification plus one logical-head/cache verification for layered disks, and reuse captured preparation. Discard uses a durable tombstone and generic sidecar locks, lists affected media and requires explicit confirmation. Flatten streams new raw heads at the layer depth limit; published local records preserve retention. Regression coverage includes process exits with layered parents and both CAS modes.
- **2026-11-14 — Immutable disk layers and workspaces:** fixed disks seal deterministic sparse sector packs into parent-bound descriptors; chain reads use the existing sector interface and verify complete logical SHA-256. Named isolated workspaces retain journal policies, with a shared materialized cache for layered heads. Offline whole-machine publication includes all catalogue media and CMOS, verifies the closure, publishes by tag CAS and rebases the workspace through a durable retryable intent. Opt-in CLI commands run, list, discard and publish workspaces. Fault injection covers publication steps; tag deletion leaves shared blobs intact. GUI integration, registry transport and GC remain separate.
- **2026-11-14 — Installation media sets:** Neutral deterministic OCI media-set codec, extras with provider-validated canonical DOS names, PC floppy/ISO storage formats, `image media import/ls/show`, and summary facets are implemented. Media > Insert offers From library, ordered catalogue loading, an initial disk selection and read-only or machine-local journals. Optical, incompatible and damaged sets remain visible with reasons. Imports preview their order and offer dry runs; preparation runs in background jobs, and transactional catalogue insertion reuses disks and verified receipts. The default store uses the shared unemu family data root directly, with store.toml and images/ inside; Preferences can override it. The library collection, Import dialog and transfer-medium generator remain deferred.
- **2026-11-14 — Machine settings:** Machine > Settings… (Ctrl+, outside guest
  focus) opens the classic seven-tab property sheet from effective inherited
  values, including user-ROM machines without their image bytes. Assembly owns
  the fields, domains, disabled-choice reasons and all resource-conflict pairs.
  Declared catalogue firmware also constrains services before ROM bytes are read.
  Running/paused sessions are read-only. OK/Apply previews and writes a sparse
  `machines/local/<parent>-custom.toml` child with explicit replacement consent,
  then selects it in the library. Shipped files and inherited media pins/policies
  stay unchanged; pending image checks still gate Start. Headless model tests
  cover discovery, round-trips, preview bytes, invalid edits and save guards;
  visual acceptance remains manual. Metadata uses the normal machine-file
  lowering rules; indexed ROM edits preserve other bindings and symlink spellings.
  Saved local children edit in place with a diff and overwrite consent; firmware
  edits defer discovery to readiness checks.
- **2026-11-14 — Firmware packages:** The neutral strict OCI codec, shared extras, PC roles/chip layouts/fit validation, ROM-folder import and `image firmware ls/show` are implemented. Import matches full hashes and lengths, rejects incomplete or ambiguous sets, preserves sources and publishes by CAS. Frontends explicitly select the store: desktop readiness honors Preferences, CLI run accepts --store/--no-store, and plain library loaders disable store lookup. Fully pinned declarations keep their image/chip shape; identical package re-import is idempotent and preselection store failures warn and fall back to declared paths and ROM folders; damaged selected packages block use. Machine-image import embeds ROMs and records package provenance. Firmware summaries expose role, board/class and catalogue facets without reading ROMs. The Firmware collection UI remains pending; see [design §5.3](design/machine-images.md#53-firmware-packages).
- **2026-11-14 — Installation media sets:** the design section and [mock-up](design/mockups/install-media.html) were approved by the maintainer. The codec, CLI and From library dialog are implemented; the library collection, Import dialog and transfer-medium generator remain deferred.
- **2026-11-14 — PC machine image import:** `pc-image` imports resolved TOML/YAML machines into self-contained OCI blobs and local tags, validates PC bundle bindings and resolves them back to assembly input. Firmware, full media catalogues and CMOS are copied; journal generations freeze to whole raw bases through private recovery copies. `image import` requires an explicit store and tag, with `--create` for initialization; interrupted journals require named per-medium recovery choices. Validation and resolution keep large disks file-backed, and local import records retain policies and generation choices. Source files and journals remain unchanged. Layers, workspaces and registry transport remain separate.
- **2026-11-14 — PC machine image import:** `pc-image` imports resolved TOML/YAML machines into self-contained OCI blobs and local tags, validates PC bundle bindings and resolves them back to assembly input. Firmware, full media catalogues and CMOS are copied; journal generations freeze to whole raw bases through private recovery copies. `image import` requires an explicit store and tag; interrupted journals require a recovery choice. Source files and journals remain unchanged. Layers, workspaces and registry transport remain separate.
- **2026-11-14 — Firmware folder index and progress:** desktop readiness uses a
  local, bounded, atomically replaced SHA-256 cache with fresh metadata checks;
  invalid indexes rebuild and completed scans drop deleted files from scanned
  folders without discarding hashes for other folders or pin sizes. All pins in a
  chip layout share one folder walk, and declared paths obey the size filter.
  The status bar and Locate firmware dialog show listing/hashing progress,
  all found lines plus bounded skip results, Cancel scan, cached Check again and
  an uncached Rescan action. Progress stays in the fixed-height message pane;
  listing and index are shared across machine checks for the desktop session.
  Start reuses the checked document, verifies images while showing “Loading machine…”,
  and retries discovery once after a located firmware file fails verification.
  Folder changes and Rescan refresh listings; selection alone does not.
  Pending machine rows show “checking firmware…” with Start disabled. Synthetic
  headless regressions cover cache reuse, edits, deletion, invalid indexes,
  cancellation and rescans; native visual acceptance remains manual.

- **2026-11-14 — Machine image catalogue:** `image-store` now provides the strict,
  deterministic schema-1 publication codec, read-only degraded display projections,
  atomic local catalogue overlays and personal ratings/notes, and explicit overlay
  publication preserving the machine digest. Personal fields never enter blobs.
  Local per-title facet/search summaries rebuild in bounded lazy batches; warm
  startup reads the index, library and cache without opening payload blobs.
  Per-image failure rows prevent rebuild starvation; content hashes handle restored
  local files. Corrupt libraries permit read-only listing, cache writers merge rows,
  and capacity pressure is reported with reduced search data. Publication consumes
  overlays and moves local personal data, sanitizes artwork and reuses unchanged
  metadata bytes. Overlay artwork is exposed as GC roots. GUI wiring, provider
  launch compilation, import and GC remain separate.
- **2026-11-14 — Host speed controls:** View > Speed offers 100%, 200%, 400%
  and Unlimited; every session starts at 100%. Ctrl+Alt+T temporarily holds
  Unlimited and restores the selected mode on release or focus loss. The status
  cell measures virtual-time progress against host time over the last second.
  Paced modes may use the full observed frame period; Unlimited leaves 3 ms
  for presentation. Periods track repaint cadence within 16.7–50 ms, with at
  most one guest frame on each 60 Hz deadline. Mode changes and resume re-anchor
  without catch-up. Audio is muted outside 100%, without time-stretch; the UI
  distinguishes speed mute from the user preference toggled by Ctrl+Alt+M.
  Returning to 100% uses fresh buffering and preserves that preference.
  Guest clocks and headless execution are unchanged. Headless pacing, keyboard,
  audio and synthetic-machine determinism tests cover these paths; no GUI
  acceptance was performed. See [window commands](commands.md).
- **2026-11-14 — Machine image store:** platform-neutral OCI layout, streamed immutable SHA-256 blobs, deterministic manifests, provider-validated complete closures, paged local tags, provider-declared payload codecs, foreign OCI index preservation, missing-artwork reporting and atomic compare-and-swap updates are implemented; application wiring, import, layers, workspaces and registry remain separate.
- **2026-11-14 — Application logo:** the logo now shows "PC"; the project name remains unemupc98. Embedded window/Dock icon, GIF-timed welcome animation (file-only `reduced_motion` preference), static About logo and README branding. The welcome logo is centred above the heading
  at up to 128 px; welcome/About frames key only the exact uniform border colour
  connected to the perimeter by four-neighbour paths. Enclosed regions of that
  colour remain opaque, preserving the logo interior. The window/Dock icon is unchanged; visual
  acceptance remains manual.

- **2026-11-14 — Desktop media controls:** the Media menu and Ctrl+Alt+I
  expose a per-drive Insert dialog, catalogue next/eject, disabled absent drives
  and CD-ROM, and the fixed disk's write policy. Browse checks geometry and full
  SHA-256, then appends through assembly's validated media boundary. Existing
  catalogue policies stay fixed; browsed media permit read-only or a separately
  named journal with the drive's retention policy (ten generations by default).
  Cancel preserves the medium; paused insertion advances no guest time.
  Browse validation errors
  remain editable in Insert without ending the session. Mount/persistence failures
  open an error dialog and stop execution. Tools creates blank CHS/size fixed disks
  or formatted standard FAT12 floppies using the existing media-create code. Relative
  journal paths use the base directory; relative creation paths use the machine
  directory. Creation prefills the failing drive's path and geometry. Journals
  default beside their base with a Save picker. Occupied creation outputs ask for
  a new name. Status cells clip independently; library warning markers exclude
  pending checks
  and skipped firmware roots. Details wrap left, shorten machine-folder paths,
  offer cause-specific recovery hints and report EMS unknown after failed loading.
  Reset chords account for remaining typed-buffer capacity, discarding stale host
  backlogs and waiting when the complete chord cannot yet fit;
  replay retains typed-chord ownership. Headless tests cover these paths; native
  picker and visual acceptance remain manual. Live journal-sector counts are not
  exposed by the frontend and are omitted.

- **2026-11-14 — Buffered guest warm-reset input:** the shell's menu,
  toolbar and Ctrl+Alt+End transfer the whole Ctrl/Alt/Del make-and-break chord
  to the keyboard in one call. Its 16-byte typed buffer accepts keys while the
  controller interface is disabled; FFh/F5h/F4h/F6h discard all unsent typed bytes,
  preserving the controller output and ACK/BAT order (IBM PC AT Technical
  Reference, Keyboard, Keyboard Buffer and Commands from the System). The
  bounded overflow policy replaces the last buffered byte with 00h. Whole-script
  golden injection retains its existing order. Snapshot version 9 saves and
  validates the typed buffer. Host shortcuts now pass Ctrl/Alt taps to the guest
  because modifier deferral was removed; Ctrl or Alt held alone reach games.
  Consuming a host chord releases every already-forwarded modifier, and plain
  Ctrl+C retains its make/break sequence. Mock-bus and headless tests cover
  buffering under ADh, reset flushing, delivery, overflow and snapshot restore.
  Both `user_rom_at_games_ctrl_alt_del_reboot` and its frame-chunk variant use
  one-call typed injection; their existing pins await supervisor measurement.

- **2026-11-14 — ATA read-block timing:** PIO reads now enter BSY with DRQ
  cleared between blocks and schedule the next block from a nominal 3600 rpm,
  17-sector service clock (about 1 ms). A Status acknowledgement after consuming
  a block can lower IRQ14 before the next block raises it, preserving separate
  edges at the cascaded PIC. Writes and the initial read block remain immediate.
  Snapshot formats 3 (ATA) and 4 (AT adapter) include the pending deadline and
  reject older formats. Mock-bus timing, reset/replacement, malformed-snapshot
  and continuation checks accompany an open guest IRQ14 regression; both IRQ
  regressions fail against the previous implementation.

- **2026-11-13 — Machine settings capabilities:** assembly owns shared card,
  RAM, SB16/EMS/UART/IDE resource domains and defaults, video decodes, named
  floppy geometries and firmware slots. `capabilities::describe`, `choices`
  (with paging for large integer domains) and `validate_draft` expose options
  and structured diagnostics without UI dependencies. Drafts replace resources
  atomically, retain card identity paths and report unavailable firmware as
  pending. Known firmware metadata also restricts floppy types and reserves
  ROM ranges before bytes are available. Settings presentation remains separate.
  Repository-file synthetic parity snapshots and the existing lowering and
  validation corpus cover unchanged legacy behavior and source diagnostics.

- **2026-11-13 — egui desktop shell:** `unemupc98-window` opens a classic
  in-window File/Machine/Media/View/Tools/Help menu, toolbar, status bar and
  machine library. Start changes the same client area to a nearest-filtered
  4:3 guest texture; Pause/Resume, guest Ctrl+Alt+Del, media next/eject and Stop
  reuse the existing session boundaries. Stop finishes persistence. Missing
  firmware/media disables Start using the existing validator. View offers
  modern skin and system/light/dark themes. File Open accepts a machine-file
  path; Tools › Locate firmware/Preferences save ROM folders and refresh
  readiness using the shared explicit-roots SHA-256 discovery. The CLI accepts
  repeatable --rom-dir. The macOS shell uses only its native title bar and the
  toolbar paints its icons without font symbols. Future editors/import/layers are placeholder dialogs, save
  state/CD-ROM remain disabled. Drive activity observation is unavailable, so
  only the power light changes. Last-run metadata stays blank. Existing window
  tests remain headless; new command/discovery/texture/input tests also open no
  windows. Visual appearance and physical capture/audio need manual checking.

- **2026-11-13 — AT controller self-test and warm start:** 8042 command AAh
  reinitializes the output port to its power-on value and drives A20 enabled,
  while retaining the SYS flag/55h reply and issuing no CPU-reset request.
  This is an AT compatibility policy: IBM PC AT Technical Reference, Keyboard
  Controller, Commands, documents the self-test reply but not that output-port
  initialization. The IBM POST depends on it before protected-mode memory
  sizing after DOS has disabled A20. Mock-bus and synthetic IBM 5170 ROM tests
  cover D1h/DDh, AAh and the subsequent high reset fetch. The existing test
  without AAh still pins the empty-bus/#UD result with A20 disabled.
  `Machine::reset_cpu` remains a board CPU-line reset, preserving RAM, devices,
  the 8042 output port and A20. Host reboots use guest Ctrl+Alt+Del or relaunch.
  `user_rom_at_games_ctrl_alt_del_reboot` follows the guest path back to the
  FreeDOS A: prompt. **MEASURED (supervisor user-ROM release run):** reboot
  returns to `A:\>` at `CS:IP=05E3:15C4`, with 81,853,092 retired instructions
  and VGA text SHA-256
  `8810a375732aec052068b73542b7a9a9185e8be8d5224b75a7834aa5a02dc8b4`.
  The test pins those values. Its polling and the open install golden reject
  terminal run outcomes and a non-advancing virtual clock, so a failed warm
  start cannot spin forever below a virtual-time cap. Transient non-text VGA
  frames during POST are allowed. The open installation golden injects only
  Ctrl/Alt/Del makes for both warm starts:
  queued breaks would precede the keyboard reset ACK/BAT during POST.
  **MEASURED (before the installed-empty-drive correction, unchanged CPU pin):**
  guest warm starts move the C: prompt from
  screen FNV-1a `1429bef60948e1d5`, 102,208,270 retired instructions and virtual
  time 106,800,000 to `5d0c917f3ab08d17`, 98,075,818 and 100,800,000.
  Guest warm POST replaces the former host CPU resets, changing the executed
  path and the time displayed by DOS. This measurement predates the installed
  empty-drive correction; the combined endpoint is recorded under Open FreeDOS
  installation media below.
  **MEASURED mutation:** reverting only AAh output-port initialization fails
  both synthetic regression tests, but the open install golden still passes
  with the same endpoint. That golden qualifies the guest reboot path, not
  the IBM-specific dependency on AAh. With the user-supplied IBM BIOS, the
  same mutation never returns to the prompt after Ctrl+Alt+Del (measured
  before the polling guards existed; the guards now turn that into a prompt
  test failure).

- **2026-11-13 — Exact clock domains:** `device::clock` centralizes OSC =
  315/22 MHz and named PIT, CGA/VGA, UART, RTC, OPL3, 48 kHz output and
  12/16 MHz AT CPU crystal domains. Device durations, PIT input-edge counting,
  legacy scheduler ordering and visits, audio grids and host pacing use exact
  ratios. Legacy devices run only when a PIT input edge has elapsed, including
  when a scheduled transition occurs between PIT edges.
  One virtual tick remains one PIT clock. Rounded RTC/UART/VGA/pacing crystal
  spellings now derive from OSC; the existing RTC UIP window remains 1193 PIT
  clocks. CPU instruction-rate calibration and policy units are unchanged.
  See [conventions](conventions.md#device-time-and-clock-domains) for the rules
  for new code. This implements decision-22 §1a–b; the private time field,
  permanent multi-tick test build and stateful VGA beam remain separate seams.

- **2026-11-13 — Lo-tech hardware EMS:** the r02 board model owns 512 KiB
  to 2 MiB SRAM behind a 64 KiB MMIO frame at C000h/D000h, or E000h on Bare. Four
  write-only page latches decode 260h/264h/268h/26Ch and all 400h aliases;
  values 80h–FFh and unpopulated chips float high and drop writes. Power-on
  latches are deterministically FFh; CPU and device reset retain latches and
  SRAM. Ordinary RAM keeps its existing CPU path. On IBM 5170, E0000h–FFFFFh
  is system-board ROM space, so EMS uses C000h or D000h; Bare also permits E000h.
  A code fetch from the frame raises a CPU fetch fault and stops the run with a
  CPU error; this limitation is tracked as open work (upc-6le.53). Coverage and
  model choices are in `crates/devices/src/lotech_ems.rs`.
  Machine files require `frame` for `kind = "lotech-ems"`; `port` and `size_kib`
  have defaults. Both shipped game profiles enable 2 MiB at D000h, port 260h.
  The CPU-region snapshot includes the card. MEASURED: the open-profile
  behaviour golden is unchanged. The supervisor's gate provides the user-ROM
  at-games golden measurement before merge. LTEMM r01 is a fetched,
  double-hash-pinned, gitignored fixture, never redistributed or placed in a published disk image; the project
  emulates the board only and LTEMM is the user's own driver (decision-23, accepted). The ignored open-firmware
  FreeDOS MEM golden uses a disposable in-memory tools disk and a synthetic DOS
  program that checks all 128 pages through all four windows before MEM /E,
  including an aliased-page negative control. MEASURED: MEM reports 2,048 KiB
  total and free EMS (2,097,152 bytes). The D000h endpoint retires 18,681,813
  instructions with text FNV-1a `0eda968da7052433`; the page-alias mutant fails
  the DOS check at 16,355,288 instructions with text `df1f6dc3129c84ce`. See
  [the machine reference](../../machines/README.md#lo-tech-ems-card-reference)
  and [fixture terms](../../vendor/ltemm/README.md).

- **2026-11-13 — Game-ready AT profiles:** `machines/at-games.toml` combines
  the user-supplied June 1985 IBM 5170 BIOS and IBM VGA EPROM, register-level
  VGA, 640 KiB + 1 MiB RAM for a shorter POST memory test, SB16 with OPL3
  (220h/IRQ5/DMA1/HDMA5), a COM1
  Microsoft serial mouse, a type-2 IDE disk with a separate journal, and a
  swappable 1.2M FreeDOS/transfer catalogue. B: is omitted because this BIOS
  has no 1.44M drive type. `at-games-open.toml` uses Kiselev AT-reset and XTIDE;
  since 2026-11-14 it also has the register-level VGA with the open VGA BIOS
  (entry above) instead of the MDA/VGA-text console. Both keep the empty 80287
  socket and fixed virtual RTC.
  The [game guide](../../machines/README.md#playing-dos-games) covers supplying
  ROMs/tools, disk preparation, copying files, BLASTER, mouse drivers and
  journal export/rollback. The FreeDOS fixture supplies only kernel and shell.
  Synthetic tests check resolved firmware pins/device selection, lowering,
  resource wiring, full-base hashing and non-writing `machine check`.
  Synthetic C0000h validation correctly refuses an unidentified system BIOS;
  the rest of that smoke uses a relocated C8000h stand-in. The opt-in
  `user_rom_at_games_freedos_prompt` keeps the actual C0000h mapping and uses
  a disposable blank HDD. **MEASURED (supervisor user-ROM run, earlier 4 MiB extended RAM):** this game-ready
  AT boots FreeDOS with the user-supplied IBM 1985-06-10 system BIOS and IBM VGA
  BIOS: `RetiredCapReached`, `CS:IP=0070:02CB`, `retired=68093462`,
  `dos_prompt=true`, VGA text SHA-256
  `a2b73fae1c7ee569829051dea41b7c997aaeace94f3d28a8d9ee7ab89e364452`.
  The test pins these values and the complete POST checkpoint sequence against
  CPU revision `fe240a56ac2040e3deaebb2537e642137ee02c71`.
  The IBM profile's measured boots are POST and guest Ctrl+Alt+Del with a blank
  type-2 disk, ending at `A:\>`. FDISK, FORMAT, copying and running from C:
  through the IBM fixed-disk BIOS on the AT adapter remain unmeasured, as does
  C: boot. No game or real
  mouse-driver session is qualified. The I286 supports LMSW entry, 16-bit
  protected execution and task switches; the pinned POST tests all 640 KiB +
  1 MiB. No memory manager is supplied; XMS, INT 15h block move and DOS-extender
  use remain unqualified. There is no 386 instruction set.
  Tyrian 2000 and Duke Nukem 3D require a 386 or later and remain **not yet**.
  **MEASURED (open fixtures):** `at-games-open.toml` passes non-writing
  `machine check` with the real pinned Kiselev AT-reset, open VGA BIOS, XTIDE
  and FreeDOS 1.2M fixtures and `media create` local disks. Its ignored FreeDOS
  prompt golden samples the POST screen, then records `RetiredCapReached`,
  `retired=15241264`, `dos_prompt=true`, VGA text SHA-256
  `41ff081e1227af8f1a053df1b627b734de4032963cc0357c488cbbe67e01469e`
  (previously, on MDA: `retired=13772948`, text
  `f016e6af8c82493b1a489b8753d1ad9c34f31da6af744e177984405ca1f4ead6`).

- **2026-11-13 — Sparse persistent media:** journal drives read their immutable
  base on demand and retain only written sectors, including removable floppies.
  CLI/window journal-disk launches retain the verified read-only descriptor from
  machine-file lowering through assembly: one streamed full-base SHA-256 and
  no whole-disk buffer. Independent assembly/offline opens verify once each.
  Metadata checks reject changes after verification through the host
  failure channel. Retention, readable finish, verify/rollback and atomic export
  use sparse states and bounded buffers; the version-1 format is unchanged.
  Counting-allocator tests cover a sparse 512 MiB media lifecycle and 504 MiB
  frontend launch. Seeded keep=2 workloads restore base sectors and compare every
  retained generation export with dense snapshots.
  `SectorSource` is the parent seam for future hash-linked layers; actual layer
  files, deduplication and FAT optimizations remain deferred. See
  [persistent media](design/persistent-media.md#memory-and-hashing).

- **2026-11-13 — Microsoft serial mouse:** opt-in `mouse = "microsoft"` on a
  UART card supplies a two-button 1200 7N1 endpoint. DTR/RTS power-up produces
  `M` after an assumed 10 ms startup plus a byte frame; inactive lines cancel
  transmission and drop motion. Signed motion splits across three-byte packets,
  with positive Y down. The generic UART now receives timed bytes, models RX
  FIFO/overrun, receive/line-status IRQs and FIFO timeout, and gates its board IRQ
  through OUT2. Loopback forces external modem outputs inactive and isolates
  external RX; a driver probe therefore powers the mouse down and restarting it
  identifies again. 7N2 receives the 7N1 stream; other divisor/data/parity
  mismatches receive 00h with framing error (an explicit approximation, including
  8N1). Line settings are sampled at the start bit, after startup, so raise-then-
  program works.
  Snapshots include endpoint identity, partial transmission and accumulated input.
  Machine files reject simultaneous PS/2 and serial host targets; the window
  routes its existing captured input to either target. Target lookup is resolved
  from configuration without synchronizing unrelated devices. Restore rejects
  endpoint/MCR line mismatches without mutation. Presets are unchanged.
  Logitech extensions, bit sampling, TX timing/interrupts, modem inputs and
  internal loopback feedback remain unmodelled. Synthetic IRQ4 ROM and mock-bus tests qualify the
  protocol; CuteMouse, MOUSE.COM, games and Windows driver sessions have not
  been exercised. See [serial mouse configuration](design/machine-files.md#serial-mouse).

- **2026-11-12 — CPU A20 wiring:** Assembly applies the shared 8042/92h OR
  line at construction; the machine forwards effective transitions after port
  writes and before runs using the change counter. CPU-only resets, including
  shutdown, preserve the line. IBM 5170's I80286 Machine model honours the gate.
  Bare rigs with an A20 source use that model too, retaining a low reset entry;
  source-free rigs keep flat Application(Real16). The initial transition count
  remains 1 after 8042 attachment, 0 for port 92h alone. CPU physical bit 20 is
  gated; host memory access and ISA DMA bypass it. Source policy remains OR
  (ASSUMED), with SeaBIOS qualification still owned by the generic ISA board.
  Synthetic tests cover bidirectional 1 MiB aliasing through both controls,
  OR combinations, CPU-only reset, gated fault metadata and redundant-call
  suppression. Bare A20 rigs vector divide error through INT 0 in the guest.
  The IBM 5170 reset test pins the assumed external-gate/ROM interaction:
  with A20 off, FFFFF0h becomes empty-bus EFFFF0h and FFh bytes enter #UD.
  See [the board contract](design/machine-config.md#a20-line-and-system-control-port-a).

  **MEASURED CPU-detection re-baseline, with the same CPU pin:** the bare POST
  rig has an 8042 and therefore runs the I80286 Machine model. The single-step
  trap after POP DS changes Kiselev's 8088-identification result from
  `Harris-designed 80C88` to `Intel 8088 '78` and adds its segment-load warning.
  The maintainer accepted this model change. Only this CPU-detection report
  changes; the floppy, FreeDOS and IDE screen expectations stay unchanged.

  | Bare POST expectation | Previous | Current |
  | --- | --- | --- |
  | MDA FNV64 (`bios_boot_golden`) | `bfa9b5ab3f8508a4` | `b59246d76d78f3fc` |
  | Logical FNV64 (`bios_boot_golden`, volatile media POST, scheduler POST) | `e36ef0102a7cbc14` | `945fcce568e5d520` |
  | MDA cursor row | 18 | 19 |
  | Retired instructions | 19,015,022 | 19,015,022 |
  | Virtual time | 20,000,000 | 20,000,000 |

  **Proof:** in the captured 25-row MDA console, restoring only the CPU
  identification text, removing only the added warning row and appending the
  resulting empty final row reproduces the previous raw MDA and logical FNV64
  values exactly. Removing that row also restores cursor `(0,18,true)` from
  `(0,19,true)`. RTC, other text, stop kind and POST bytes require no changes.
  No A20-wrapped guest access explains this drift; it is the model's TF behavior
  (Intel 80286 Programmer's Reference Manual, §9.7.1).

- **2026-11-12 — Derived AT CMOS century:** every derived layout seeds 32h
  from the RTC start year in BCD, including the standard AT layout. The IBM
  PC AT Technical Reference, System Board, CMOS RAM address map, defines this
  date century byte outside the checksum range. Images retain their supplied
  byte; journals retain their persisted byte. The live two-digit year still
  rolls from 99 to 00 without updating the century seed. The open Kiselev BIOS
  displays `2000-01-01` instead of `0000-01-01`; FreeDOS keeps
  `Sat 01-01-2000` instead of falling back to `Tue 01-01-1980`, and new DOS
  files receive the 2000 date. Screen goldens retain these date differences. See
  [machine-config.md §RTC start and NVRAM source](design/machine-config.md#rtc-start-and-nvram-source).

  **MEASURED against `20cd1da`, with the same CPU pin:** seeding CMOS 32h
  with 20h gives the Kiselev BIOS the century for its RTC line and date service.
  FreeDOS accepts that date, and FreeCOM displays `Sat 01-01-2000`;
  the DOS write golden's new `ZZ` entry is dated `01-01-00` rather than
  `01-01-80`. Reversing only the RTC date, FreeCOM date line and `ZZ` year in
  the captured consoles exactly recovers every previous raw and logical hash.
  The raw MDA hash re-baseline is:

  | Golden | Previous hash | Current hash |
  | --- | --- | --- |
  | BIOS POST | `0b55c39cf21513e6` | `bfa9b5ab3f8508a4` |
  | BIOS floppy boot | `9a13654e51c09b99` | `bfa3c76f8f761e6b` |
  | DOS DIR | `20d57080f986a38c` | `806c1c1c475339a2` |
  | DOS write | `e4c4f5de9a7f0d49` | `71f290c7a24fb363` |
  | FreeDOS prompt | `c95b91cec87e1379` | `83ffbc2844f28889` |
  | IDE prompt | `b93bd1a24305e490` | `b7d8e6cd59c50bf2` |
  | IDE final | `ada1634e25758753` | `a045a62f7781ffa1` |

  Logical hashes normalize RTC/time rows but retain the DOS date and directory
  entries. BIOS POST and floppy logical hashes remain unchanged; the moved
  constants are:

  | Golden | Previous logical hash | Current logical hash |
  | --- | --- | --- |
  | FreeDOS prompt; media fixture; scheduler prompt | `af201c7e8acb071d` | `c46f0c147c881837` |
  | DOS DIR; scheduler DIR | `d330b78c4533697e` | `65259fa4c26a891c` |
  | DOS write; media fixture; scheduler write | `a6601fff99236a61` | `6b0404d06d7532c3` |
  | IDE prompt | `26cb04d76a1ee3c7` | `fbd3caf6e77b879d` |
  | IDE final | `bf790c95b531f8e8` | `48d2b9699fbe5cc2` |

  With the valid 2000 date, the measured retired endpoints move as follows;
  virtual-time caps, cursors, POST/COM1 and stop kinds remain unchanged.
  Both scheduler engines match complete access/state streams with these
  date expectations.

  | Golden / endpoint | Previous retired count | Current retired count |
  | --- | --- | --- |
  | FreeDOS prompt | 8,800,477 | 8,800,473 |
  | DOS DIR | 11,772,930 | 11,772,926 |
  | DOS write | 11,766,599 | 11,766,596 |
  | IDE prompt | 15,775,579 | 15,775,575 |
  | IDE final | 19,746,109 | 19,746,105 |

- **2026-11-12 — Open AT BIOS:** legacy Bochs rejected; keep Kiselev with an explicit display switch for open goldens and use the user-firmware lane for AT fidelity; an open 286 AT BIOS is [deferred](deferred.md#open-at-class-80286-system-bios).

- **2026-11-12 — Audio seam:** the `audio` crate provides exact PIT-to-48-kHz
  mapping, interval box filtering of edge histories, timestamped native-rate integration,
  saturating stereo mixing and bounded headless PCM capture with streaming hashes. Run-scoped
  `AudioSinkKind` defaults to no output; source-free machines create no mixer.
  Sources release history even with output disabled and retire after their tails.
  Assembly settles devices before audio observation, including halted time.
  The AT speaker records OUT2 AND port 61h bit 1 at exact PIT clocks, with
  independent GATE2 control, direct bit-banged/PWM output and 0/+16384 stereo
  drive levels with exactly zero silence. SysControl/PIT pairs attach it
  automatically; default None output bounds device history to an incomplete
  frame even without capture calls. Synthetic AT tone and toggle goldens pin PCM,
  while late device-history errors surface at the next observation and restore
  the current level in a new segment without waiting for another pin change.
  The window plays through cpal (i16 or f32, stereo plus silent extra host channels),
  preferring 48 kHz, then a compatible device default, then the supported rate
  nearest 48 kHz. Producer-side linear interpolation carries exact rational phase
  across blocks; the deterministic stream stays at 48 kHz. Downsampling has no
  anti-alias filter and can alias high frequencies. Device-rate ring frames keep
  a 200 ms capacity, 50 ms prefill/latency target and oldest-frame drops above
  70 ms. Underruns play silence. Ctrl+Alt+M toggles host mute; unavailable or
  fatally failed output reports once and leaves
  emulation running silently. Transient cpal notifications keep playback active;
  recoverable source errors never stop the window. Tests stay device-free; physical playback is
  unmeasured. Optional SB16 DSP 4.05 now supports 8-bit and 16-bit mono/stereo DMA output
  (single-cycle and auto-init), exact programmable rates, direct DAC, pause/resume,
  independent byte/word IRQ acknowledgement and backward-compatible hardware snapshots. The `sb16` machine-file card
  defaults to base 220h, IRQ5, DMA1 and word DMA5; no preset adds it. CT1745
  registers and SB Pro aliases read back with documented reset values; master
  and voice volumes attenuate PCM. Other mixer controls are stored only.
  The onboard YMF262 now supplies native integer FM (Nuked-OPL3 1.8 port),
  virtual timers and OPL aliases, with CT1745 MIDI/master attenuation. Recording
  remains unimplemented. See
  [audio design](design/audio.md).

- **2026-11-12 — 8042 keyboard command transmission:** writing a keyboard
  byte to port 60h releases the keyboard clock and clears command-byte bit 4;
  controller parameters and D3/D4 writes retain their routing, and the aux inhibit
  bit is unchanged. ACK readiness waits two 11-bit serial frames (**DERIVED**:
  2626 PIT clocks at the model's **ASSUMED** 10 kHz clock). Reset BAT adds
  300 ms (**READ**: IBM PC AT Technical Reference, March 1986, 101/102-Key
  Keyboard, Basic Assurance Test, p. 4-39; lower end of 300–500 ms) plus a reply
  frame. Unread output remains first; draining it cannot shorten reply deadlines.
  Snapshots preserve queued receive deadlines and use device version 7.
  E0h read-test-inputs remains unmodelled because serial pin levels are not tracked.
  Keyboard replies now take the host and reply transfer time and the keyboard's
  basic assurance test delay (IBM PC AT Technical Reference, 101/102-Key Keyboard,
  Basic Assurance Test), so the BIOS keyboard reset finishes later in virtual
  time; screen content is unchanged apart from the clock rows. The six BIOS/DOS
  goldens retain their normalized logical hashes and cursors. Only BIOS floppy
  changes retired instructions: 19,008,614 → 18,991,006 (−17,608) within the same
  20,000,000-PIT-clock cap; all other retired counts and virtual-time caps stay fixed.
  The measured raw MDA hash re-baseline is:

  | Golden | Previous hash | Current hash |
  | --- | --- | --- |
  | BIOS POST | `809829e50bc06fbd` | `0b55c39cf21513e6` |
  | BIOS floppy boot | `938e07668f3b0b34` | `9a13654e51c09b99` |
  | DOS DIR | `bb926924f4d7efa1` | `20d57080f986a38c` |
  | DOS write | `153da3e002975834` | `e4c4f5de9a7f0d49` |
  | FreeDOS prompt | `526086b917baa58e` | `c95b91cec87e1379` |
  | IDE prompt | `49bbec2d36c0bbf0` | `b93bd1a24305e490` |
  | IDE final | `fb811e1f40836da7` | `ada1634e25758753` |

- **2026-11-11 — Absent physical memory on firmware boards:** assembly covers
  the AT's fixed 24-bit extent (16 MiB) around configured RAM, ROM,
  MMIO and existing zero-filled adapter gaps. Remaining holes read FFh
  (**ASSUMED** floating value) and discard guest writes, so memory sizing can
  probe above installed RAM without an unmapped CPU error. Configured extended
  RAM remains writable up to the shared 15,232 KiB board-map limit (**READ**,
  IBM PC AT Technical Reference, Section 1, System Memory Map); Bare
  rigs retain their maps. FE0000h–FFFFFFh mirrors the resolved low 128 KiB
  read-only as a board decode. Fill/copy scratch is bounded to 8 KiB; VM extent
  is fixed by AT decoding; CPU width and mapping bounds are validated. With
  A20 enabled, accesses above 1 MiB read FFh or extended RAM instead of Unmapped;
  the CPU wiring follow-up now makes a disabled gate alias physical bit 20. See
  [memory-map policy](design/machine-config.md#memoryconfig--a-semantic-memory-map).

- **2026-11-11 — Register-level VGA graphics:** opt-in `vga` cards capture
  planar 16-colour 0Dh/0Eh/10h/12h and chain-4 13h through owned indexed
  pixel frames with a captured DAC palette and presentation repeat factors.
  CGA-compatible mode 06h uses row-scan bits to select its memory banks. The existing display route follows
  AC text/graphics selection; the window presents pixels with integer scaling.
  CRTC logical geometry, start/stride, double scanning and Fig. 2-65 addressing,
  plane enable, AC colours and PEL mask are covered. CGA-interleaved shift,
  mid-frame changes other than the split and smooth-scrolling timing finer than
  a frame remain unmodelled. Frame-level panning and splits are covered below.
  Existing text presets and golden expectations are unchanged. See
  [graphics scanout](design/observation.md#register-level-vga-graphics-scanout).

- **2026-11-11 — XT devices (device level):** Mode 0 8255, XT SW1/PPI and
  83-key set-1 receiver, NMI enable latch at A0h, and single 8237 with XT page
  decode are available as isolated devices, with snapshots and trace policy
  tests. Review against the March 1986 XT Technical Reference corrected SW1
  POST/FPU bits (default 7Dh), removed the PC-only PA/SW1 path, added the reset
  hold threshold, bounded the keyboard queue to 16, and covered DMA/NMI aliases.
  The queue emits FFh on overrun. The reset hold uses a documented 1,193-clock
  compatibility threshold and preserves an unread board byte until PB7 clears
  it; tests cover the BIOS's 10,582-iteration hold at rate 1/1 and PPI/controller
  register aliases. The PIC test uses BIOS ICWs 13h/08h/09h. Module matrices
  distinguish READ facts from remaining wiring/timing assumptions; keyboard
  transmission is byte-level.
  No board is wired: `ibm-5160` and `8088` remain reserved and rejected.

- **2026-11-11 — Debug console:** emulator-only QEMU-style byte port (default
  402h, E9h readback) and bounded `DebugConsole` observation are available to
  bare Rust test rigs. IBM 5170 validation rejects the device; future
  `generic-386-isa` wiring and machine-file exposure remain pending.

- **2026-11-11 — PS/2 auxiliary channel and mouse:** the 8042
  can attach an `AuxDevice` at construction, routes D4h to it, tags aux output
  with status bits 0/5 and exposes IRQ12 as interrupt output 1 (command-byte
  bit 1). D3h injects an aux byte even without a peripheral. Clock inhibit
  retains peripheral output; ready keyboard/aux bytes alternate after controller
  replies. `Ps2Mouse` implements standard ID-00 commands, stream/remote/wrap,
  signed motion, three buttons, overflow and stream-only 2:1 scaling. Snapshots
  include partial exchanges and all peripheral state. Packets are emitted at
  input injection boundaries and coalesced while busy; sample-rate pacing in
  virtual time is not modelled. Machine configuration now opts in through `I8042Cfg.mouse` or
  `board.mouse` in TOML/YAML; slave IR4 / IRQ12 uses the same level propagation as
  IRQ1. `assembly::inject_mouse` deterministically accepts motion/buttons at
  run boundaries, including accumulation with reporting disabled. A synthetic
  AT ROM checks reset/enable replies and exact packets through a guest IRQ12
  handler with both PIC EOIs. Existing profiles attach no mouse. The window now
  captures a configured mouse on a client-area click, swallows that click, and
  releases with Ctrl+Alt+G, focus loss or suspension. It tries Locked then
  Confined, hides the cursor and shows the release chord in the title; macOS
  supports Locked only. Raw relative input uses a fixed 1:1 scale with Y-up
  conversion and carried fractions. Motion coalesces between button changes
  and enters only at run boundaries. Pure and headless presenter tests cover
  conversion, capture, button ordering and guest-visible packets; host GUI grab
  behavior remains unmeasured. See [window input](design/observation.md#captured-ps2-mouse).

## VGA frame scrolling and page flipping (2026-11-13)

Register-level VGA now applies AC horizontal pixel panning in planar, chained
and unchained 256-colour graphics and in text; CRTC byte panning and preset row
scan; and the 10-bit line-compare split with the AC panning-compatibility flag.
Start address and CRTC 08h latch at vertical retrace start; pixel panning
latches at retrace end using virtual time. This pairs a start written during
display with panning written during retrace for the next frame.
Scrolled text captures use indexed pixels for partial cells; ordinary text
captures retain their cell grid. Snapshots use version 4 and restore version 3
with latches initialized from the live registers. Mock-bus pixel tests and a
synthetic planar ROM cover scrolling and page flips without external firmware.

IBM establishes start-address latch timing. Abrash documents programming
panning after sync starts; sampling it at retrace end and CRTC 08h at retrace
start are explicit frame-level assumptions. Undefined panning values and split
boundary phase remain ASSUMED. CGA-interleaved shift, mid-frame changes other than the split, and
smooth-scrolling timing finer than a frame remain unmodelled. See
[frame scrolling](design/observation.md#frame-scrolling-split-screens-and-retrace-latches).

## IBM ISA VGA video ROM integration (2026-11-12)

- Firmware supports a separate C0000h–C7FFFh video scan for the IBM 5170 BIOS
  catalogue and both Kiselev builds. Extension-ROM checks retain their separate
  windows and checksums. The `ibm-vga-bios-1987-32k` entry is **Tested** for
  the qualified IBM 5170 BIOS 1985-06-10 case below. Its supplied full EPROM
  identity maps offset 2000h onward as 24 KiB at C0000h and requires register-level `vga`. A plain 24 KiB dump is unlisted.
- Machine files use `firmware.option_roms` plus `kind='vga'`; the catalogue owns
  the container offset, full-file SHA-256 pins include the unmapped prefix,
  and TOML/YAML also support explicit offsets for synthetic/unlisted ROMs.
- VGA adds subsystem and ISA setup/enable decode (3C3h/46E8h/102h), a
  deterministic colour-monitor sense approximation and virtual-time retrace
  polling. Existing mono/colour port select, plane-2 font upload and DAC index
  protocols are covered across enable changes. Version-3 snapshots include
  enable state. See [observation coverage and assumptions](design/observation.md#vga-cpu-decode-monitor-sense-and-polling).
- A synthetic 24 KiB ROM inside a 32 KiB container tests the open Kiselev POST
  path. **SUPERVISOR-MEASURED at CPU `fe240a56`:** the ignored
  `user_rom_ibm_vga_bios_mode13_pattern` qualifies the user-supplied IBM VGA
  BIOS with IBM 5170 BIOS 1985-06-10 on `ibm-5170`: POST initializes the video
  ROM, INT 10h mode 03h produces 80×25 text with 16-line characters, and mode
  13h produces the complete 320×200 byte-ramp pattern. The test pins Halted at
  `0000:7C50`, **23,463,327** retired instructions, and both capture SHA-256
  values in the [qualification table](design/firmware-and-machine-classes.md#video-rom-placement).
  Sampling requires the boot program's magic/stage marker and CS/IP inside
  its sector. Missing ROMs skip cleanly. No ROM or firmware screen text is
  stored or fetched; other BIOS revisions and VGA modes are not qualified.

## Works (as of 2026-11-10, main @ 479f8ed — the 286 foundation is finished)

- **An unmodified open BIOS boots FreeDOS from a floppy, headless and deterministic.** The Kiselev
  Xi8088 BIOS completes POST and its INT 19h loads a FreeDOS 8086 floppy over the µPD765 floppy
  controller + 8237 DMA; FreeCOM reaches its prompt, and a scancode script drives it to `A:\>`,
  runs `DIR`, and creates a file (`echo x>zz`) that the controller commits to the in-memory image.
- **Firmware catalogue:** IBM 5170 BIOS bytes are checked against the pinned
  Kiselev Xi 8088 1.0.2 SHA-256 before mapping. Unknown images list known builds
  and fetch instructions; listed firmware checks board, CPU and required parts.
  Declared or sole mis-sized ROMs receive a precise BIOS-placement diagnostic;
  ambiguous undeclared ROM sets report a missing BIOS. System BIOS presence,
  placement, power-on reset and isolation from writable
  RAM/other images are checked independently of allowlist membership. Option
  ROMs are also SHA-256 checked, aligned, bounded by the BIOS scan window,
  header/length/checksum validated and mapped read-only. Adapter space has no
  writable system RAM; MMIO is carved out of the zero-filled gaps. Assembly
  reports effective ROM scan order. Machine-file loader APIs accept the run-only
  unlisted escape while preserving source diagnostics. A synthetic 2 KiB
  extension has a fixture-gated BIOS POST acceptance test. CLI and window
  accept the run-only `--allow-unlisted-firmware` flag and report the unknown hash
  to stderr. Bare assembly remains unrestricted; the historical POST runner
  explicitly checks the real BIOS against the intended board catalogue.
- **Devices:** 16550 UART, 8259 PIC pair (master + slave cascade), 8253 PIT, 8042 keyboard
  controller, MC146818 CMOS/RTC (seeded NVRAM), 8237 DMA, µPD765 floppy (read + write), port 0x61,
  POST port 0x80, independent VGA/CGA and MDA text cards, all behind the `Device` trait with mock-bus
  tests; text output reaches a `FrameSink`.
- **2026-11-11 — 8254 read-back:** PIT count/status latches support single and
  multiple counters, status-first reads in all RW formats, programmed mode bits
  (including aliases), OUT and NULL COUNT. NULL COUNT follows the initial load
  clock and running Mode 2/3 reload boundaries; BCD counting remains unsupported.
- **Device fidelity:** PIT Modes 2/3 adopt complete reloads at period/half-period
  boundaries and isolate incomplete LSB writes. RTC periodic PF/PIE uses a
  rational 32,768 Hz divider and rounded-up PIT deadlines, including IRQ8 wake
  from HLT; periodic and calendar phases remain an explicitly documented
  approximation. CRTC indices decode five bits; cursor comparison uses the full
  16-bit address while scanout wraps at the CPU aperture by assumption. FDC
  IRQ6 and DMA requests obey DOR bit 3. With RDY active, SEEK succeeds with or
  without a disk or installed drive. RECALIBRATE reports equipment check with
  PCN=0 only for absent drives; installed empty drives complete normally and
  report T0 at PCN=0 while disk change remains asserted. Without a disk,
  ID/data searches wait for reset without an interrupt. SENSE DRIVE STATUS
  (04h) accepts the drive/head parameter and returns ST3 without raising or
  acknowledging an interrupt or changing the present cylinders. ST3 echoes
  unit/head, reports two-sided geometry, installed-drive track 0 and media
  write protection, and keeps READY active for every select. ASSUMED: no disk
  clears TS/WP; absent drives also clear T0. FT is always clear because drive
  faults are not modelled.
  ASSUMED for silicon: SEEK/RECALIBRATE completion supersedes unread reset
  polls (Bochs-style, limited to seek and recalibrate completion). MAME instead
  retains pending ST0 per drive and senses the lowest pending drive; NEC µPD765
  and Intel 8272A SENSE INTERRUPT STATUS descriptions do not define the interaction.
  SIS reports the pending seek/recalibrate completion and then idle 80h.
  All four C0h–C3h/00h reset polls remain available until a seek/recalibrate
  completion supersedes them. Overlapped seek completions are not modelled:
  one latch holds the latest completion, even across drives.
  CCR writes at 3F7h select 500/300/250 kbps (00/01/10); reserved 11 matches
  no medium. READ ID and READ/WRITE DATA
  report abnormal termination with ST1 missing-address-mark when the selected
  rate differs from the medium: 500 kbps for 1.44/1.2 MB, 250 kbps for
  720/360 KB. Fault result/IRQ occurs on the final command byte, without DMA
  or rotational delay; DIR reads and seek/sense commands are unchanged.
  IBM PC AT Technical Reference, Fixed Disk and Diskette Drive Adapter,
  Diskette Control Register (3F7h), does not specify its reset value.
  ASSUMED: device reset selects 500 kbps; DOR reset preserves the adapter
  rate. Snapshot v4 stores it and restores v3 using 500 kbps. Separate drive
  and medium types (360 KB media in a 1.2 MB drive at 300 kbps) and
  rate-dependent transfer timing are not modelled.
  WRITE DATA reports not-writable before rate or CHS errors on protected media.
  Per-device module matrices record the remaining unsupported cases and
  assumptions; changed expectations are listed in
  [scheduler adoption](design/scheduler.md#adoption).
- **AT fixed-disk adapter registers:** `ide` cards accept
  `controller_model = "at-adapter"` on every board, keeping task latches
  accessible without a disk (including cylinder-low 55h/AAh echo). Port 1F1h
  reads Error and writes Precompensation. IBM's adapter reference, **Command
  Register** (p. 7), inhibits drive commands without readiness and clears IRQ14
  on every command write; **Diagnose** (p. 10) runs controller self-tests and
  interrupts even without a disk, returning error 01h. Status acknowledges the
  shared controller IRQ (pp. 5–6); write-only 3F6h reads FFh (p. 11 I/O table).
  ASSUMED exact empty-drive idle status: 00h. Snapshots carry an explicit model
  tag and preserve pending diagnostic IRQs without media. Default `"ata"` and
  historical presets retain their behaviour. The adapter is opt-in alongside
  the FDC, which owns all 3F7h bits; MFM and full WD1003 commands/timing remain
  unmodelled. See
  [machine-config.md](design/machine-config.md#the-fixed-disk-idehdd-attachment-seam)
  for reference sections and coverage. MEASURED externally: an empty adapter
  removes POST error 601 with an original IBM 5170 BIOS; this worker checks the
  register probe using synthetic firmware rather than that BIOS.
- **Open FreeDOS installation media:** `vendor/freedos/fetch.sh` builds the
  pinned `freedos-tools-1200.img` (1.2 MB FAT12) with FDISK, FORMAT, SYS,
  XCOPY, EDIT, MEM and CTMOUSE. FDISK needs 186/286 or later, not 8086;
  CTMOUSE needs a 286 or later, so the complete tools set requires a 286.
  Package versions, licences and compatibility evidence are in `vendor/freedos/README.md`; a local-package
  mode rebuilds it without network access from the verified boot fixture.
  `freedos_install_golden` creates a blank 32 MiB disk through `media create`,
  partitions it as active FAT16, warm-starts through guest Ctrl+Alt+Del, runs
  FORMAT with system transfer, ejects A: and warm-starts again to boot C:.
  Before combining the paths, CPU resets with installed-empty-drive behavior
  ended at screen FNV-1a `8f1e57cd86975e25`, 104,210,281 retired instructions
  and virtual time 108,800,000. Guest warm starts before that drive correction
  ended at `5d0c917f3ab08d17`, 98,075,818 instructions and virtual time
  100,800,000. **MEASURED with both changes:** the final C: prompt has screen
  FNV-1a `040490b8724fbaba`, 99,077,816 retired instructions and virtual time
  101,800,000, with the same open `at-ide.toml` firmware and CPU pin `fe240a56`.
  The new pin combines guest Ctrl+Alt+Del POST with successful recalibration
  and T0 on the installed empty A: drive after eject, which reaches READ ID
  instead of failing recalibration. Both POST work and the displayed DOS time
  differ from the earlier baselines.
  MEASURED on the CPU-reset path: normal empty-drive recalibration reaches READ ID;
  without index pulses the BIOS exhausts one IRQ wait (726,351 virtual steps)
  and three command-ready waits (786,864 total). NEC uPD765A datasheet,
  READ ID command and Status Register 1 (MA): an ID search ends only on an ID
  field or after two index pulses. Intel 82077AA datasheet, §8.1 Command and
  Result Phase Handshaking, corroborates the required software timeout. The
  motor start is unchanged; the first C: boot read moves 1,507,019 virtual
  steps later, rounded to two prompt-polling chunks at the endpoint. XTIDE now
  sees timeout 80h and omits the old error 40h line; the retained top banner
  and later RTC time change the screen hash. With guest warm starts, the
  correction instead moves the endpoint by one polling chunk: 1,000,000
  virtual steps and 1,001,998 retired instructions beyond the warm-start
  baseline. The polling cadence and POST path determine the combined endpoint.
  Omitting FORMAT makes the same C: boot fail at the partition boot signature
  check. Writes stay in a hash-bound journal; the base remains unchanged.
  See `machines/README.md` §Playing DOS games for the same installation steps.
- **IDE PIO:** one primary (1F0h/3F6h, IRQ14) or secondary (170h/376h,
  IRQ15) channel assembles from Rust configs and TOML `ide` cards. Master/slave
  fixed disks use explicit CHS geometry and volatile/readonly/journal `SectorMedia`.
  Primary IDE splits the FDC decode at 3F6h. An assembled Bare rig transfers all
  256 IDENTIFY words with `rep insw`; firmware boards require a ROM declaring
  fixed-disk services, even for an empty IDE card. The fixture recipe retains
  the AT build and adds configured XT+ (`xub-r638-xtp-ata1f0`), using 16-bit
  ATA polling. `machines/at-ide.toml` loads XT+ at C8000h with a readonly
  615/4/17 FAT16 master. **FreeDOS now boots from A: and lists C:** the separate
  Kiselev `bios-xi8088-atreset.bin` accepts XUB's DL=80h reset request using
  IBM AT semantics. The upstream BIOS remains unchanged for existing machines.
  The ignored `ide_freedos_golden` types `DIR C:` through the keyboard and
  checks label `UNEMUHDD`, `MARKER.TXT` (29 bytes), the completed prompt,
  screen hashes and POST. Three runs before pinning matched: 14,777,577 retired
  at the first shell prompt, 17,748,107 after DIR, final screen FNV-1a
  `ef5a10891b9ef196` (rechecked unchanged at CPU pin `a92b676`, including the
  corrected FDC INT acknowledgements). XT+ detects `UNEMU ATA-2 PIO DISK`.
  No emulator device or CPU fix is included. The golden rejects missing XUB,
  a byte data window, a renamed marker
  in a copied HDD, and the original BIOS; disconnecting IRQ14 preserves its
  pins because this build polls. CMOS
  fixed-disk types follow the provider: option ROMs (including XUB)
  leave 12h/19h/1Ah zero; a hash-matched system BIOS that provides fixed-disk
  and declares `cmos-drive-types` derives IBM AT types 1–14/16–22 and the
  checksum. Unsupported geometries are refused rather than inventing a type-47
  convention. Kiselev declares no such capability; existing seeds are unchanged.
  Catalogued BIOS service
  declarations must match the catalogue, and omitted declarations inherit it. ATA write batches preserve complete sectors on interruption and
  reject readonly writes with ERR/ABRT. ATA submits completed commands independently of host pacing. Assembly flushes
  in-flight prefixes only for finish, CPU reset, run errors and explicit
  snapshots/checkpoints; partial sectors stay in controller state. Media capacity is checked at
  construction. Primary/secondary IDENTIFY interrupts reach the cascaded PIC.
  Existing ATA behavior tests are unchanged.
- **Floppy and IDE write modes:** volatile is the default (in memory only), readonly
  rejects writes, and explicit journal mode persists command-complete writes
  beside an immutable file-backed base and replays them on the next mount.
  Journal selection is available in Rust `MachineConfig` data and TOML/YAML
  floppy and IDE drive tables, with explicit per-layer journal paths and positive
  retention counts (default 10). Changing a drive image drops inherited journal
  settings while preserving readonly/volatile modes; a child can turn journaling
  off without repeating the image. Journal outputs cannot alias loaded images or
  machine-file layers, with source-positioned refusals before assembly.
  Fixture machines keep their existing modes; persistence goldens use temporary journals.
  NVRAM journals persist changed CMOS RAM (0Eh–3Fh except 0Fh) only before CPU
  reset and at finish. Reset records keep one session generation open; finish
  closes it and runs retention. Run stops and host frame slices perform no NVRAM
  I/O. The journal codec, recovery and offline tools are shared with drives.
  TOML/YAML accept explicit journal paths and retention with drive-style image
  override units. Readonly NVRAM is rejected with a volatile-mode hint because
  the MC146818 has no write-protect input. IDE shares the drive journal format and
  offline tools (512-byte sectors; capacity from the base image length). Offline CLI
  `media generations` and `verify` inspect under shared locks, with an explicit
  no-lock report when read-only storage has no lock entry;
  `verify --repair-tail` and `rollback --to N` require synchronized full backups and
  exclusive locks. `media commit`/`compact` exports a verified immutable image;
  configured retention keeps a verified anchor and the requested closed history.
  Acceptance tests cover full volatile/journal guest results and replay,
  failure isolation and rollback through fresh guest DMA. The ignored fixture
  guard checks pinned hashes and vendor directory entries across volatile
  BIOS/FreeDOS/DOS-write runs; golden expectations remain unchanged.
  macOS/APFS sync/lock/publication paths were measured; Linux storage behavior
  and physical power-loss durability remain assumptions under the documented
  storage contract. See [persistence evidence](design/persistent-media.md#acceptance-evidence-appendix)
  and the [runnable walkthrough](commands.md#journal-session-walkthrough).
  Closed/recovered markers do not imply a healthy filesystem.
- **Machine files in frontends:** TOML/YAML, inheritance and embedded presets
  feed the window and CLI through `machine-file`, with source-positioned
  assembly catalogue diagnostics. `machine check FILE` validates and assembles
  without guest execution or journal writes (exit 0/1/2). Missing, recoverable
  and busy journals produce notes/warnings; corruption or a wrong base is invalid.
  `run --machine FILE` accepts retired/idle caps and a display/UART card id for
  final output. Normal halt/budget exhaustion finishes persistence; failed runs
  leave unsealed commands for the next mount.
  The window defaults to the build workspace's `machines/at-freedos.toml`,
  chooses the first display or `--display CARD`, and no longer accepts positional
  BIOS/floppy paths. FreeDOS goldens load that same file; BIOS goldens and
  synthetic fast tests keep their presets.
- **Machines are declarative.** A `MachineConfig` (the `assembly` crate) is built by one
  `assemble()` and run under a `RunPolicy`; presets replace the old hand-written runners.
- **CPU pin:** `fe240a56ac2040e3deaebb2537e642137ee02c71` (x86jit `main`).
  READ: real-mode I286/I386 machine execution now accepts the Application Real16
  instruction set, including AAD/AAM, BCD/SALC, x87 with MSW.EM/TS checks and
  resumable REP INS/OUTS. I286 rejects 386-only 66h/67h forms with #UD.
  IDE and Landmark pin assertions name this revision. Open-firmware goldens
  use the measured I286/IVT baseline below; proprietary runs remain unmeasured.
  Historical validation by the supervisor at `44403dd` on 2026-11-11: the fast
  suite, slow set and user-ROM lane passed with unchanged golden values on
  Application Real16. At `a92b676`, 1,666 fast tests, 611 release device tests
  and all existing slow golden expectations passed.
  The `44403dd` update added `xj-srh.11 (x86jit)` A20 gating for LegacySystem.
  Board A20 integration now forwards the retained shared line into this CPU
  profile; the accepted no-media bare POST re-baseline is recorded above.
- **CPU:** `ibm-5170` with model `80286` selects the I286 machine profile, with
  real-mode entry, LMSW protected entry and same-CPL 16-bit protected execution.
  `set_cpu_model(I80286)` fixes the VM model and defaults to InGuest delivery:
  real exceptions use the IVT; CPL-0 protected interrupts, exceptions, IRQs,
  TF and IRET use the IDT. LDT selectors and descriptor-query instructions are
  available. READ: `xj-srh.18 (x86jit)` adds CPL-0 hardware task switches
  (TSS/task-gate JMP/CALL, IRET with NT, IDT task gates), busy bits and MSW.TS
  under `decision-24 (x86jit)`; privilege transitions and V86 remain unsupported. Explicit
  outward faults retain typed vector/error-code/CS:IP diagnostics. A fault
  during #DF delivery requests the same CPU-only reset as the 8042, retaining
  RAM, CMOS, the A20 latch, devices and accounting. The standalone Real16 constructor
  and source-free bare rigs retain Application Real16; bare rigs with an A20
  source select I80286 with low reset fetch and in-guest processor exceptions.
  Both profiles have exact step bounds,
  per-iteration REP time, STI/SS-load inhibition and interrupt-readiness stops.
  Real16 INS/OUTS support byte/word elements, REP, OUTS segment overrides,
  and IRQ delivery between elements; answered INS stores retain their value on retry.
- **Scheduler:** deadline dispatch is the sole production engine. INTA follows
  completed instruction bus/DMA effects and only occurs when the CPU can accept
  it. Active caps are exact; HLT jumps to the next event or bound. The per-clock
  reference is test-only (`reference-scheduler`); config has no timing or block
  quantum selection. The Long64 machine demo/blob path is intentionally retired.
- **Observation:** screens come from the text card's scanout state, COM1 and POST
  bytes through byte sinks; access traces are off in production and opt-in as a
  bounded ring with drop counts. Snapshots carry each device's time base and
  reject malformed input.
- **Optional window presenter:** the `window` leaf crate embeds the original
  VileR v2.2 CP437 MDA/VGA fonts (CC BY-SA 4.0), parses NE/FNT resources and
  rasterises decoded text styles to softbuffer pixels. `unemupc98-window` boots
  `machines/at-freedos.toml` with paced runs and observation subscribers; it retains
  the last delivered frame while continuing past date/time prompts. Physical
  host keys map to set-1 make/break bytes, including E0 navigation and right
  modifiers, queued only at run boundaries with backpressure. Presenter,
  keyboard and FreeDOS input tests run headlessly. The maintainer confirmed the
  FreeDOS prompt, font and colours in a window. MDA underline uses row 12; cursors use BIOS default shapes rather
  than programmed CRTC start/end registers. HLT waits for IRQs within run caps.
  The event loop alone reads a host clock: integer-rational PIT targets use the
  configured CPU rate, 60 Hz paints and a 50 ms guest-work bound per paint.
  Excess backlog is discarded; `WaitUntil` idles between paints. Occlusion uses
  `Wait` and pauses the guest; resuming discards the hidden interval. Fake-clock
  synthetic and fixture-gated FreeDOS tests replay accepted input at recorded
  virtual times with different headless chunking and compare MDA/time/device
  state, CPU registers/retirement and conventional RAM hashes. Blink remains steady; its phase can later derive from virtual time.
  Pause/Break and PrintScreen are unmapped. Typematic uses host repeated presses.
  Manual desktop acceptance passed for date/time input, `c:` and `dir`.
  E0 navigation/Divide use IBM artificial Shift wrappers on first make/final
  break; repeat makes are plain. Encoding at boundaries follows guest LED NumLock
  state, preserving navigation through Ctrl+NumLock and reboots. The 8042
  snapshot v4 stores LEDs and pending LED parameters; reset clears both.
  Focus loss releases held keys. macOS winit 0.30 does not deliver CapsLock.
- **Tests:** MEASURED 2026-11-10 at `479f8ed`: `cargo test --workspace` 610
  passed, 0 failed; `cargo test --release -p devices` 358 passed; the 13 slow
  goldens and engine comparisons pass (BIOS POST, floppy boot, FreeDOS prompt,
  `DIR`, file write, per-clock versus deadline on every workload with final
  state); clippy for the full and the production feature sets, fmt, publication
  guard and cargo deny are clean; `cargo doc` has no warnings. Forgejo CI runs
  the same gate. See `commands.md`.
- **Dependency gate (2026-11-11):** the all-features `deny` lane also checks
  RustSec advisories. Vulnerabilities and yanked crates fail; unmaintained crates
  require a dated, justified ignore with a review date. RUSTSEC-2026-0192
  (`ttf-parser` 0.25.1, unmaintained, not a known vulnerability) is accepted to
  retain winit's Wayland client-side decorations; review on 2027-05-11.
- **Speed (MEASURED 2026-11-11):** the release BIOS POST, FreeDOS boot and
  `DIR` test has a median wall time of 0.436899 s at `a92b676`, versus
  0.449007 s at `8979fc9` (three alternating pairs, 2.7% less time under shared
  host load). Current-pin sample shares await the supervisor; see
  [`design/machine-time-profile.md`](design/machine-time-profile.md).

- **CPU warm restart:** 8042 reset requests restart the CPU at F000:FFF0 after
  the persistence boundary check. CLI natural, prompt, post-prompt and
  machine-file runs, and the window, resume with 80286 reset registers and
  cleared execution latches. A reported reset remains pending until performed;
  direct machine resumption resets before execution, including after a host
  persistence failure. Window reset diagnostics count only performed resets.
  RAM, CMOS shutdown byte 0Fh, devices, the 8042
  output-port/A20 latch, retirement counters and virtual time survive. Reset
  does not end a journal session. I286 reset restores MSW/IDTR and real caches,
  including hidden CS base FF0000h; the system ROM has a 24-bit reset mirror.
  A20 address masking follows the shared board line; see
  [machine-config.md, Board model](design/machine-config.md#board-model).

### User-supplied system ROM catalogue

- **2026-11-11 — User-ROM test lane:** shared test-only SHA-256 discovery accepts
  arbitrary filenames in `vendor/user-roms/` and optional `UNEMUPC98_USER_ROMS_DIR`,
  including nested chip layouts or assembled images. `ci-gate.sh user-roms` is a
  local opt-in lane; absent identities skip visibly unless
  `UNEMUPC98_REQUIRE_USER_ROMS=1`. CI never calls it. Synthetic tests cover
  discovery and policy, including symlinks and loops, external-root configuration
  failures, partial-layout counts and downstream path redaction. External roots
  must be absolute folders of unpacked dumps. Proprietary firmware execution
  remains limited to the measurements below.

IBM 5170 BIOS versions 1984-01-10, 1985-06-10 and 1985-11-15 are Listed by
assembled SHA-256. Single interleaved images and even/odd parts (including
both 1984 chip layouts) support identity checks, TOML/YAML inheritance,
per-part pins and chip diagnostics (including partial pair swaps and exchanged
halves). Child source overrides drop stale service declarations; known images
use catalogue services. No proprietary bytes are distributed.
No default firmware changes. **MEASURED at CPU `fd67927` with locally supplied
firmware:** the 1985-06-10 BIOS passes POST cleanly and boots FreeDOS from a
1.2 MB floppy to the DOS prompt.
This does not qualify other BIOS revisions or broader guest compatibility.
`user_rom_ibm_5170_1985_post` uses I286 with MDA, `display_switch='auto'`, an
`at-adapter` fixed-disk controller, read-only `freedos-1200.img`, derived
volatile CMOS and a pinned RTC. It waits for FreeCOM's date prompt, sends Enter
for date and time, then polls for `A:\>` or `A:>`. Reports contain a safe stop,
port-80h checkpoints, retirement count, DOS-prompt verdict and final MDA-plane
SHA-256, without screen text. Regression expectations pin that measurement;
identity tests remain available.
See `vendor/user-roms/README.md` and `vendor/freedos/README.md`.

## Port 61h timing re-baseline (2026-11-11)

Port 61h bits follow the PIT; delay loops in the Kiselev BIOS now take real
refresh time. Bit 4 toggles on PIT1 OUT rising edges, bit 5 samples OUT2, and
bit 0 drives GATE2. The shared PIT snapshot owns the refresh flip-flop and gate.

**MEASURED by the supervisor:** the IBM 5170 BIOS speed/refresh check passes
at rates `4/7` through `2/3`; `11/20` stops at checkpoint `04`, and `27/40`
stops at checkpoint `11`. The 6 MHz `ibm-5170` default is **8/13 steps per PIT
clock**. This applies to Rust presets, machine-file board defaults and the
shipped FreeDOS TOML/YAML files; IDE inherits the FreeDOS rate. Explicit rates
remain authoritative. Bare rigs retain their own rates, including 1/1 for the
historical no-controller POST rig. See
[machine configuration](design/machine-config.md#cpuconfig-and-clockconfig--minimal-but-the-right-home).

**MEASURED comparison against main:** the old baseline is main `889601b`,
with CPU `44403dd85077c0db29ad09780003ebd3b193ecdd`, per-read refresh and rate 1/1.
The new side uses the same CPU pin, PIT-driven status and calibrated AT rate.
No intermediate 1/1 refresh baseline is used in the table below. Complete MDA
and colour dumps, COM1, POST, cursors and final data were compared. Every
logical screen matches main after replacing only displayed RTC/time fields.
There are no unexplained category (d) changes: (a) guest stages occur at different
steps, (b) guest clock text advances differently, and (c) safety/command windows
must reserve enough time for completed commands rather than stop early.

Goldens again use the shipped CLI BIOS/FreeDOS runners. Their production boot
and POST caps are 20,000,000 steps, with a 3,000,000-step command window.
At 8/13 the date prompt is first observed at the 10,000,000-step poll; DIR and
write/DIR finish their windows at 13,000,000, leaving seven million steps before
the safety cap (one more prompt poll plus six million of margin). Poll cadence
remains one million steps. IDE keeps its 100,000,000 safety cap, three-million
command windows and 40,000,000 DIR completion bound. The trace-overflow test
also asserts completed command output, final prompts and successful stop kinds.
The shared test helper contains passive screen assertions only, not execution
policies, captures or a duplicate floppy reader.

**MEASURED final endpoints:** R is CPU retired instructions; T is virtual
instruction-equivalent time, including halted time. Hashes are FNV-1a over all
25 canonical MDA rows. Exact raw hashes and counters are pinned separately from
whole-console logical hashes, which replace only `RTC (Real Time Clock):` and
`Current time is` fields. Both scheduler engines match complete access/state
streams at the production-policy endpoints.

| Golden / endpoint | Category | Main R -> new R | Main T -> new T | Main MDA hash -> new MDA hash |
| --- | --- | --- | --- | --- |
| BIOS POST; scheduler BIOS POST | a/b/c | 9,015,022 -> 19,015,022 | 10,000,000 -> 20,000,000 | `cf7334eb70c4c237` -> `809829e50bc06fbd` |
| BIOS floppy boot | a/b | 10,996,320 -> 18,993,723 | 12,000,000 -> 20,000,000 | `ba49c0fe9e88293e` -> `0744aaf03bcc6424` |
| Scheduler BIOS floppy | a/b | 8,996,320 -> 18,993,723 | 10,000,000 -> 20,000,000 | `ba49c0fe9e88293e` -> `0744aaf03bcc6424` |
| FreeDOS date prompt; scheduler prompt | a/b | 7,802,475 -> 8,802,475 | 9,000,000 -> 10,000,000 | `1da6d49437180ba0` -> `526086b917baa58e` |
| DOS DIR; both scheduler engines | a/b/c | 9,774,929 -> 11,774,928 | 11,000,000 -> 13,000,000 | `56d8215daf926acd` -> `bb926924f4d7efa1` |
| DOS write; both scheduler engines | a/b/c | 9,768,598 -> 11,768,597 | 11,000,000 -> 13,000,000 | `616104befb7e4588` -> `153da3e002975834` |
| IDE initial A: prompt | a/b | 14,777,577 -> 15,777,577 | 16,000,000 -> 17,000,000 | `922f327d9dd4db6b` -> `49bbec2d36c0bbf0` |
| IDE DIR C: and final prompt | a/b | 17,748,107 -> 19,748,107 | 19,000,000 -> 21,000,000 | `ef5a10891b9ef196` -> `fb811e1f40836da7` |
| Media fixture BIOS | a/b/c | not pinned | 10,000,000 -> 20,000,000 | `cf7334eb70c4c237` -> `809829e50bc06fbd` |
| Media fixture FreeDOS prompt | a/b | not pinned | 9,000,000 -> 10,000,000 | `1da6d49437180ba0` -> `526086b917baa58e` |
| Media fixture write | a/b/c | not pinned | 11,000,000 -> 13,000,000 | `616104befb7e4588` -> `153da3e002975834` |

Final cursors match main: BIOS failure `(0,18)`, boot-sector console `(0,17)`,
date prompt `(31,24)`, completed commands `(4,24)`. Colour text remains blank.
BIOS clock text changes from `0000-01-01 00:00:01` to `0000-01-01 00:00:03`;
floppy DOS time from `12:00:07.41 am` to `12:00:12.90 am`; IDE time from
`12:00:11.36 am` to `12:00:18.12 am` (b). Dates, banners, directory and volume
identities, file/free-space counts and final prompts match. DIR lists KERNEL
and COMMAND; the write session adds ZZ with `x\r\n` (three bytes) verified
from volatile floppy media; IDE DIR C: lists the 29-byte MARKER.TXT.

POST remains `01 02 03 04 05 06 07 08 09 10 12 20 21 22 24 25 30 31 40 43 00`.
IDE retains `41 42` between `40` and `43`. COM1 remains empty except for the
boot sector's `B`. Fixture SHA-256 hashes and vendor namespaces remain intact.

**DERIVED / MEASURED calibration:** including `DEC BL / JNZ`, iterations per
complete refresh-bit cycle are `n = (36R - 2)/3`. Centring 256 cycles at 1710
iterations gives `R = (3 × 1710/256 + 2)/36 ≈ 0.6122`. The synthetic guest
counts 2903 iterations at 1/1 (CX=`F4A9h`) and 1721 at 8/13 (CX=`F947h`, inside
the BIOS acceptance range). Actual firmware window measurements above belong
to the supervisor; complete proprietary POST/boot compatibility remains
unqualified. See [scheduler calibration](design/scheduler.md#device-migration-inventory).

## I286 CPU-probe and exception-delivery re-baseline (2026-11-12)

**MEASURED against main `5931d32`, CPU `44403dd`:** the new CPU pin `8bad85d`
and `set_cpu_model(I80286)` take the BIOS single-step CPU probe traps per the
Intel 80286 Programmer's Reference Manual §9.7.1 and Appendix B, and deliver
real-mode exceptions through the IVT. This is the approved reason for the
following golden changes. The synthetic boot sector reaches the normal
20,000,000-step cap through the BIOS INT 13 handler rather than stopping at
the segment-limit exception. Exception delivery itself retires no instruction.

Full raw MDA and colour dumps, POST and COM1 were compared without normalizing
away text differences. The sole visible change is:

- `Main Processor:             Harris-designed 80C88 @ 4.77 MHz` ->
  `Main Processor:             Intel 8088 '78 @ 4.77 MHz`;
- the added line `WARNING: This CPU does not disable interrupts after loading segment registers!`;
- following rows move down one; the final blank row leaves the 25-row display.

This remains visible only in the floppy-boot endpoint. DOS and IDE scroll it
off the screen, leaving their final screens byte-identical. The historical
no-controller BIOS POST rig is Bare/Application Real16 and stays unchanged.
All guest-rendered clock lines, boot messages, directory entries, prompts and
file contents match main. T is virtual instruction-equivalent time, R is
retired instructions, and hashes are FNV-1a over 25 canonical MDA rows.

| Endpoint | Main R -> I286 R | Main T -> I286 T | Main MDA hash -> I286 hash | Main cursor -> I286 cursor |
| --- | --- | --- | --- | --- |
| Bare BIOS POST | 19,015,022 -> 19,015,022 | 20,000,000 -> 20,000,000 | `809829e50bc06fbd` -> same | (0,18) -> same |
| BIOS floppy boot | 18,993,723 -> 19,008,614 | 20,000,000 -> 20,000,000 | `0744aaf03bcc6424` -> `938e07668f3b0b34` | (0,17) -> (0,18) |
| FreeDOS date prompt | 8,802,475 -> 8,800,477 | 10,000,000 -> 10,000,000 | `526086b917baa58e` -> same | (31,24) -> same |
| DOS DIR | 11,774,928 -> 11,772,930 | 13,000,000 -> 13,000,000 | `bb926924f4d7efa1` -> same | (4,24) -> same |
| DOS write/DIR | 11,768,597 -> 11,766,599 | 13,000,000 -> 13,000,000 | `153da3e002975834` -> same | (4,24) -> same |
| IDE initial A: prompt | 15,777,577 -> 15,775,579 | 17,000,000 -> 17,000,000 | `49bbec2d36c0bbf0` -> same | (4,24) -> same |
| IDE DIR C: and final prompt | 19,748,107 -> 19,746,109 | 21,000,000 -> 21,000,000 | `fb811e1f40836da7` -> same | (4,24) -> same |

The floppy logical hash changes `69d62210a69ffe15` -> `92c581d95567fb07`;
all other logical hashes are unchanged. Both scheduler engines use the same
new floppy screen baseline. Colour text stays blank with visible cursor (0,0).
MDA cursors remain visible. POST remains
`01 02 03 04 05 06 07 08 09 10 12 20 21 22 24 25 30 31 40 43 00`;
IDE retains `41 42` between `40` and `43`. COM1 remains empty except for
floppy boot's `42` (`B`). No firmware or media fixture is changed.

IBM POST and the Landmark full-pass golden now pin their measured regression
values. The Landmark values below come from the supervisor's user-ROM run,
not these open-firmware runs.

## IRQ input level golden baseline (2026-11-12)

**MEASURED against main `3f529bf`:** the BIOS floppy endpoint changes from
R **18,991,006** to **18,990,963** at unchanged T **20,000,000**. The old model
acknowledged one stale keyboard vector **09h at PIT clock 4,896,203**, after POST
read FAh from port 60h at **4,896,116**. The new model withdraws that request when
the line falls, as Intel 8259A datasheet, **Edge and Level Triggered Modes**
requires. Main has 427 acknowledges; corrected inputs have 426, with identical
remaining vector order. Subsequent acceptance times can shift because the stale
handler no longer executes. Restoring only stale IRQ1 latching reproduces all
427 vectors and timestamps and the old retired endpoint, isolating the cause.
The trace's IMR is still its initialization value at the polled read; the first
later OCW1 is B8h at 4,896,164. This is withdrawal before CPU acceptance, not a
change in keyboard transfer timing or PIT arithmetic.

Both MDA raw hash `9a13654e51c09b99` and logical hash `92c581d95567fb07` are
unchanged. MDA cursor stays visible at (0,18); the blank colour screen keeps raw
and logical hash `c1fdc54ce4885c65` and visible cursor (0,0). Only the retired
count expectation in the floppy golden is updated. The other endpoints retain
their main baselines.

PIC snapshots now use version 3 and reject pending IRR bits on low inputs
atomically; version 2 is rejected because it allowed requests to outlive their
input levels. A low input may still have an ISR bit from an earlier INTA.

## Empty 80287 socket integration (2026-11-12)

ibm-5170 has an empty 80287 socket by default (maintainer decision 2026-11-12);
the core reports no coprocessor (x86jit decision-25).
The pin is `fe240a56ac2040e3deaebb2537e642137ee02c71`. The choice is carried by
`cpu.fpu = "none"` (default) or `"80287"`, and derived CMOS 14h combines the
selected display bits with coprocessor bit 1 and the drive inventory. Explicit
NVRAM images retain their contents.

**MEASURED:** the synthetic machine-file guest reads FFFFh from FNSTCW and
FNSTSW m16/AX with no FPU, completes WAIT and reaches HLT. Selecting `"80287"`
keeps the generic present engine, with control word 037Fh and CMOS bit 1 set.
The isolated RTC test checks MDA equipment bytes 31h/33h, colour bytes 21h/23h
and the AT checksum.

**MEASURED:** a paired Kiselev BIOS floppy run with Present and None reaches
the normal 20,000,000-step boundary with byte-identical raw screens. The exact
coprocessor report is unchanged:

- `Mathematics Co-processor:   Absent` -> `Mathematics Co-processor:   Absent`.

**READ:** this BIOS first compares the stored control word with 03FFh;
the generic Present engine returns 037Fh, so it already takes the absent branch.
The empty socket's FFFFh takes that same branch. This observation does not
qualify generation-specific 80287 behaviour or the board's FERR#/IRQ13 wiring.
`decision-24 (x86jit)` / `xj-srh.18 (x86jit)` additionally provide CPL-0 task
switches; that core capability is source-inspected, not a new guest-OS claim.

**MEASURED final validation:** 1,911 fast tests, 694 release device tests and
22 slow tests passed. Build, Clippy, formatting, publication guard and rustdoc
with warnings denied passed. All existing golden text/logical hashes,
retired/virtual counts, POST streams and cursor expectations remain unchanged;
no golden re-baseline was needed.

## In flight

- **2026-11-12 — Device IRQ levels and PIC withdrawal:** the machine forwards
  both device pin transitions at existing scheduler sampling points. Falling
  inputs withdraw unacknowledged edge-triggered IRR bits, including masked
  requests and slave INT on master IR2. Explicit empty INTA supplies default
  IR7 without setting ISR7; normal poll/INTA remains atomic. PIT already exposes
  sustained OUT0 in Modes 0/2/3; no timer pulse synthesis remains in the wiring.
  8042, FDC, ATA and RTC retain their held-output acknowledgement rules; UART
  IRQs and ICW1 LTIM remain unmodelled. See
  [scheduler interrupt contract](design/scheduler.md#pic-state-must-follow-cpu-acceptance).

- **2026-11-12 — AT primary-display configuration:** board display selection now
  derives the 8042 C0h input bit 6 and CMOS 14h video bits from the installed
  cards. Auto prefers MDA, then colour; `board.display_switch` accepts
  `auto`, `mono` or `colour`, with unavailable adapter classes rejected.
  ROM-less colour derives 80-column colour; a video ROM at C0000h selects
  own-BIOS configuration. No adapter retains CMOS 00 and the colour switch
  as an explicit compatibility policy. Explicit NVRAM is not rewritten.
  Kiselev profiles explicitly select colour: that BIOS interprets the AT switch
  with inverted polarity and uses MDA there, while derived CMOS video bits stay
  colour (10b). Other firmware retains automatic selection.
  Synthetic isolation, assembly and TOML/YAML tests cover these paths. The
  goldens did not change, and the user-firmware lane passed (supervisor-measured).
  See [primary display](design/machine-config.md#primary-display).

- **2026-11-11 — A20 / System Control Port A device side:** Bare rigs can
  opt into port 92h with versioned snapshots, latch readback and CPU reset on
  bit-0 0→1; readback remains set after CPU reset until cleared by a write or
  system reset (IBM ThinkPad 560 Technical Reference, "System Control Port A
  (Hex 0092)"; Intel386 EX User's Manual, PORT92, CPURST). Bit 1 is zero after
  system reset. Pulse timing and ordinary security/light latches remain ASSUMED;
  real bit-3 writes are permitted only while zero, an unmodelled restriction.
  Bit 4 is read-only watchdog-timeout status, modelled as zero (Bochs PORTS.LST,
  port 0092). The 8042 and 92h share one reset line and one observable A20 line;
  effective A20 is their logical OR (ASSUMED board policy). The enabled 8042
  prevents SeaBIOS from disabling A20 through 92h alone; `upc-o0j.6` must choose
  arbitration and test INT 15h AX=2400h/2402h. Attaching the 8042 starts the
  change counter at 1; no source reports disabled despite flat CPU addressing.
  The CPU wiring follow-up below retains those source defaults and counter baseline.
  IBM 5170 rejects port 92h. See
  [machine-config.md §A20 line and System Control Port A](design/machine-config.md#a20-line-and-system-control-port-a).

- **2026-11-11 — Firmware-owned QEMU CMOS layout:** opt-in catalogue capability
  and byte-exact synthetic tests derive RAM, century, boot order, type-47 IDE
  geometry, translation and single-CPU fields. No production entry enables it;
  the SeaBIOS entry will do so with `generic-386-isa`. The complete 128-byte
  derivation is available and Bare Rust rigs can expose it through a
  DS12887-compatible 128-byte RTC. The IBM AT retains its 64-byte MC146818 and version-4 snapshots;
  the extended part uses version 5. Restore rejects snapshots whose capacity
  differs from the configured part. NVRAM images/journals
  stay 64-byte only; machine files have no size selector. Board selection
  remains pending.

- **2026-11-13, diagnostic-ROM validation and full-pass golden:** user-supplied
  `landmark-rom-post-at-1.2.0` remains Listed for `ibm-5170`, with pinned
  assembled and U27/U47 hashes. **MEASURED by the supervisor at main `9bd422d`,
  x86jit `fe240a5`:** full diagnostic passes complete; every test passes except
  floppy read on a rig without a drive or medium. The earlier twelve-test
  Real16 prefix and unsupported-SIDT stop remain historical evidence.
  Catalogue-only, per-service direct-hardware-test permissions now allow this
  diagnostic's FDC with media and fixed-disk adapter without claiming boot
  services. Ordinary BIOS entries and unlisted images keep the provider rule.
  The new ignored `user_rom_landmark_at_full_pass` uses MDA, automatic display
  switch, a read-only 1.2 MB FreeDOS floppy, an `at-adapter` IDE card, fixed RTC
  and derived volatile CMOS. After the floppy-controller fix, the supervisor
  measured a passing floppy read and one complete pass before the 150,000,000
  cap. The ROM counts two reset-based returns from protected mode as errors per
  pass, matching its real IBM 5170 behaviour; the golden pins this state.
  **MEASURED by the supervisor and pinned:** stop `Ok(RetiredCapReached)` at
  `0004:0015`, retired instructions `138,208,231`, MDA text-plane SHA-256
  `fe45632b8a9c1b762af117b162f56adc523b8aa2e5bda990bad5adb7079e807a`.
  The existing diskless prefix configuration and constants remain unchanged.
  The capture records only stop metadata, retired count and the text-plane
  hash, with no screen text. Adapter gaps remain board-owned independently of
  the absent option-ROM scan window. Ruud XT v5.7 is a future `ibm-5160`
  fixture (`upc-fro.6`). See
  [compatibility diagnostics](design/compatibility-diagnostics.md#3-golden-capture-rule).

- **2026-11-11, fw_cfg device:** traditional read-only selector/data interface,
  inline file directory and explicit COM1 serial-console file are available
  through Rust assembly configuration on Bare rigs. Full-word selector decode
  at 510h preserves architecture keys; 511h is byte-wide. No DMA, machine-file
  syntax or firmware-board wiring; `generic-386-isa` remains pending. See
  [machine-config.md §Emulator-only firmware configuration](design/machine-config.md#emulator-only-firmware-configuration).

- **2026-11-11 — Register-level VGA:** isolated registers, DAC, planar memory,
  CPU read/write modes and version-4 snapshots (including ISA enable state and
  display latches, with version-3 migration). Register protocols, mode-3 logic,
  attribute widths and retrace status are checked against the IBM VGA/XGA
  Technical Reference Manual (May 1992), limited to the module coverage matrix:
  Figs. 2-19–2-26, 2-29–2-37, 2-58, 2-69–2-80, 2-81–2-89 and 2-94.
  Chain-4 offsets are DERIVED from Figs. 2-37 and 2-65. ASSUMED: floating ports
  and unselected memory read FF, 128 KiB mirroring, MSR[5] page polarity, DAC
  edge cases, odd/even gating of A0 replacement and the chain-4 pipeline.
  Alphanumeric rendering and opt-in `kind = "vga"` wiring now use guest fonts,
  CRTC geometry/start/stride/cursor and AC/DAC colours through `Frame::Text`.
  The existing text-only defaults and goldens are unchanged. No option ROM is
  installed by default; the user-supplied IBM video ROM is opt-in as described
  above. Graphics scanout is available, while
  SeaVGABIOS delivery via fw_cfg remains pending.

- **2026-11-11, SeaBIOS fixture:** pinned rel-1.17.0 ISA build recipe and
  configuration added under `vendor/seabios/`, with a separate firmware
  devShell. Two independent builds matched on macOS arm64 with the flake-pinned
  i686-elf GCC 15.2.0 and binutils 2.46 (MEASURED by the supervisor); the
  output SHA-256 is pinned. No board or catalogue entry is added;
  the entry waits for `generic-386-isa` and the x86jit 386 CPU model.
- **2026-11-11, phase B:** firmware decided (decision-19): unmodified SeaBIOS
  with emulator-only fw_cfg; requirements in
  [firmware-and-machine-classes.md §3.6](design/firmware-and-machine-classes.md#36-phase-b-firmware-requirements-seabios-on-generic-386-isa).

The 286 foundation milestone is closed. Next is phase A of decision-16 (the
display through a real MDA card to a window, host keyboard input): epic
`skeleton` in beads. The DOS-session profile records the current paired-pin
wall-time comparison and historical CPU sample shares.

## Broken / known defects

- **Coprocessor error/control wiring remains unimplemented.**
  `ibm-5170` now defaults to no coprocessor; machine files can select
  `cpu.fpu = "none"` or `"80287"`. Derived CMOS 14h bit 1 follows the choice,
  with its checksum updated; explicit NVRAM images are preserved. Bare rigs
  retain their present FPU. Under `decision-25 (x86jit)`, the I286 `None`
  policy accepts FNINIT and WAIT, returns FFFFh for FNSTCW m16 and FNSTSW
  m16/AX, and preserves other output destinations, subject to EM/TS/MP #NM
  controls. The `"80287"`
  option uses `Present`, with no generation-specific fidelity claim, and
  FNINIT/FNSTCW stores 037Fh. Socket ports still read FFh and ignore writes;
  FERR#/IRQ13 and F0h/F1h control wiring remain unimplemented.

- Multi-byte scalar accesses crossing a RAM/MMIO boundary in either direction
  ended the run with `Exit::UnmappedMemory` in historical x86jit
  `a92b676e081ce862dcb48c82645b3d3a8aa2364f` (READ). Its
  [`Memory::region_at`](https://github.com/unemu-org/x86jit/blob/a92b676e081ce862dcb48c82645b3d3a8aa2364f/x86jit-core/src/memory.rs#L2512-L2534)
  requires the whole transfer inside one region before read/write paths check
  for MMIO (`memory.rs:2730`, `memory.rs:2864`). Neither part of the scalar
  transfer is performed. Assembly coalesces adjacent MMIO device windows so
  those crossings reach the bus, but mixed RAM/MMIO accesses need the upstream
  CPU fix tracked by `xj-ir6.12 (x86jit)`.
- Journal selection is available in machine-file floppy tables. The CLI and
  window frontends do not load machine files yet; that integration is in progress.
  Immutable-image export (`media commit`/`compact`) and atomic retention pruning
  are implemented. Retention runs only after a durable closed generation, keeps
  a verified anchor bound to the original base, and reports maintenance failures
  separately from command persistence. Physical power-cut durability is assumed
  under the documented storage contract, not measured.

## Recently landed

- **Presentation outside machine identity:** `ObservationPolicy.output` now owns
  Headless/Capture selection, defaulting to Headless. `MachineConfig`, presets
  and machine-file lowering describe hardware only. Inline hosts retain
  `Observation::configure_output`; capture frames and boundaries are unchanged.

- **BIOS/FreeDOS console through MDA:** the profiles route `0xB0000..=0xB7FFF`
  and display ports `0x3B0..=0x3BB` / `0x3BF` to the MDA card, excluding printer
  ports. Console capture and prompt detection use its scanout. VGA has independent
  CRTC state; golden cursor expectations now reflect the two cards.
  Prompt polling and final capture share the observation facade by `DeviceId`.
  `ObservationPolicy.output` selects Headless or per-display Capture sinks; boxed
  `FrameSink` subscribers receive the same frames at the existing capture boundaries.

- **Interactive FreeDOS — keyboard to a running command** (`task-26`): a scancode
  script fed through the 8042 drives FreeCOM past its date/time prompts to the
  `A:\` prompt and runs `DIR`, listing `KERNEL.SYS` + `COMMAND.COM` — the full
  input path (scancode → 8042 → IRQ1 → BIOS INT 09h → BDA ring → INT 16h → DOS)
  end to end. Frontend-only driver (`run_freedos_interactive`); the models are
  untouched. Two genuine interrupt-model limitations were found and worked around
  in the driver (IRQ0 starving IRQ1 under a coarse quantum; the 8042 refilling its
  output buffer so IRQ1's level never re-edges) — faithful fixes tracked in
  `task-27`. Interactive golden reaches the prompt in ~25 s (`#[ignore]`).
- **🏆 FreeDOS boots to a shell prompt — the north star** (`task-20`–`task-25`):
  the unmodified Kiselev Xi8088 BIOS completes its entire POST, renders its
  hardware-detection screen via INT 10h, and its INT 19h loads a real **FreeDOS**
  8086 floppy over the modelled **µPD765 floppy controller + 8237 DMA**;
  `KERNEL.SYS` loads and inits and hands off to `COMMAND.COM` (FreeCOM), which
  reaches its prompt:
  ```text
  FreeDOS kernel 2044 (build 2044 OEM:0xfd) [compiled Jan 25 2026]
  FreeCom version 0.86 - WATCOMC - XMS_Swap
  Current date is Sat 01-01-2000
  Enter new date (mm-dd-[cc]yy):
  ```
  It then idles headless in the INT 16h keyboard wait. The whole stack runs under
  real firmware + a real OS. Key steps: x87 `FNINIT` lifted (`task-24`, cleared
  BIOS FPU detect); the µPD765 + a machine-orchestrated DMA-bus-master seam
  (`Cpu::guest_dma_write` → `note_write`, SMC-safe, `task-21`); BIOS ROM at
  `0xF8000` (`task-20`); and the final fix (`task-25`) — the BIOS runs its console
  in **MDA** mode, so its INT 10h read-char busy-waits on the mono status port
  `0x3BA`, which the machine only decoded for CGA (`0x3DA`); aliasing the mono
  ports (`0x3B4/0x3B5/0x3BA`) onto the text card cleared the spin. Console output
  lands in the MDA window at `0xB0000` (`BiosRun.mda`). Workspace: **226 tests**
  (+3 slow boot goldens `#[ignore]`; the FreeDOS one reaches the prompt in ~9 min).
- **The real Kiselev Xi8088 BIOS boots and runs 14 POST checkpoints** (`task-20`,
  folds in ROM mapping `task-12` + POST-port `task-15`): the unmodified BIOS ROM,
  mapped read-only at `0xF8000`, far-jumps from the reset vector and drives our
  hardware through CPU test → chipset → DMAC → low-RAM test → IVT → PIT → PIC → KBC
  → enable-interrupts → video-BIOS scan → MDA/CGA fallback → **RTC init** (passes —
  CMOS is seeded) → CPU-type → **FPU detect**, where it stops at x87 `FNINIT`
  (`DB E3`, linear `0xFA85B`) — the next real-mode lift gap (`task-24`). A 1 MiB
  memory mosaic (RAM / trapped VGA MMIO / zeroed option-ROM area / masked BIOS ROM)
  composes every M5a device with IRQ0/1/8 wired; `run_bios` captures the port-0x80
  POST stream + VGA screen on every path. Emulator-only; x86jit untouched.
  Workspace at that point: **208 tests**. The colour screen was blank because
  the BIOS fallback console then lived in plain RAM at `0xB0000`; it is now
  the independent MDA card, captured in `BiosRun.mda`.
- **DOS-boot groundwork — M4.5 + M5a machine hardening** (tasks 13–19, 23): the
  push toward booting FreeDOS on the AT via the Kiselev Xi8088 open BIOS
  (decision-12). **Real-mode CPU driven to 99.63% decided on the SST/80286 corpus**
  (was 96.20%), lifting LES/LDS, LAHF/SAHF, PUSHA/POPA, ENTER, real-mode LEAVE, and
  fixing real-mode string ops to use `ds<<4`/`es<<4` bases instead of a flat base 0
  (the biggest fix — ~45k corpus fails; string ops are everywhere in a BIOS/DOS).
  No regression in the engine's other modes; x86jit pin bumped to 68e0ee1.
  Machine hardened for a real BIOS: open-bus default for undecoded ports
  (0xFF read, no-op write), CGA mode/colour/status ports with a per-read retrace
  bit, the full 8042 AT command set (self-test/interface-test/input-port/aux/BAT),
  port 0x61 with PIT-driven refresh detect and timer-2 OUT, and seeded CMOS NVRAM with a valid AT
  checksum. All device-level;
  the BIOS machine assembly (task-20) composes them. BIOS fact-check (task-19)
  confirmed the Xi8088 config: master+slave cascade (no 0xA0 collision), 32 KiB ROM
  at 0xF8000, built-in µPD765 floppy INT 13h, 6845 CGA. Workspace: **205 tests**.
- **test386.asm boots as a smoke test** (`task-7`, spike): the third-party 386
  CPU-test ROM boots from RAM at `0xF0000` (reset `F000:FFF0`), reaches **POST
  `[0x00, 0x01]`**, and stops at `SAHF` (`0x9E`) in the conditional-jump test — the
  next unlifted real-mode op. This surfaced (and fixed) a **stale x86jit pin**: the
  emulator was 13 commits behind `pc-emulator`, before the far-jmp/seg-load lift;
  bumped to `3ff42a5` (also brings in-guest `#UD`/`#GP` + `RegionKind::Rom`), no
  regression. New `PostPort` (0x190 capture), `Machine::run_bounded` + a retired
  cap, and `run_test386` (captures POST on every exit path). `vendor/test386`
  builds the image from a pinned commit (GPL-3.0, gitignored). To push further:
  lift `SAHF`/`LAHF`, then POST 3 needs FS/GS, POST 8 needs protected mode.
- **M4 integration — the full AT interrupt/graphics wiring** (`task-5`): the M4
  devices are now wired into the machine. Keyboard scancodes raise **IRQ1** through
  the master PIC and a guest handler reads them; guest stores into the `0xB8000`
  text window are trapped as MMIO and rendered into a `TextGrid` through the
  `FrameSink` seam; and the **8259 slave/cascade** now carries **IRQ8–IRQ15**, with
  the MC146818 RTC's update-ended interrupt reaching the CPU across the
  slave→master-IRQ2 cascade. `AtChipset` device set is
  `[UART, master PIC, PIT, 8042, VGA, slave PIC, RTC]`. Added `Cpu::complete_mmio_write`
  (a trapped store is now acknowledged so the guest advances past it). Golden tests
  for keyboard, VGA text, and the RTC cascade path. Workspace: **163 tests pass**.
- **`RegionKind::Rom` on x86jit `main`** @ 3ff42a5: read-only executable memory for
  a masked BIOS/option ROM — reads and fetches take the inlined RAM path at full
  speed, stores are silently dropped on the interpreter path. Purely additive to the
  region-kind match arms; Long64/Compat32 codegen byte-identical (jit 203/0,
  smc 15/0), `jit_abi` offsets unchanged. Not yet consumed by the emulator.
- **Real-mode fault delivery on x86jit `main`** (`task-11`) @ cbba7d9: in-guest
  `#UD` (invalid-on-286 opcodes), `BOUND`/`#BR`, and the 80286 segment-limit /
  instruction-length `#GP` — all Real16-gated. 80286 corpus jumped to **95.98%
  decided / 92.91% exception delivery**; no Long64 regression.
- **M4 devices + first graphics** (`task-5`, device level): 8042 keyboard, MC146818
  CMOS/RTC, 8237 DMA (via a `note_write`-safe `DmaMemory` handle), and VGA text
  mode as a `Device` behind the new `FrameSink` seam (`video` crate) with a
  rendered-frame golden. All isolated + unit/golden tested. (Now wired into the
  machine — see the M4 integration entry above.)
- **Real mode graduated to x86jit `main`** (`task-3`) @ 40acdbc: the full
  real-mode (16-bit) seam — addressing, INT/IRET/IVT/IF, IRQ injection, the
  complete instruction set — now lives on x86jit main, entirely gated so the
  engine's other modes (Long64/Compat32) are byte-identical. `pc-emulator` is
  fast-forwarded to main; no drift. 80286 corpus 89.7%/95.2%, cf16 34/34,
  JIT suite for the engine's other modes intact.
- SingleStepTests/80286 authoritative silicon oracle (`task-10`) — our exact
  target CPU, now the **sole** real-mode oracle on x86jit `main`: 1,478,000 tests,
  **89.7% overall / 95.2% decided** after the lift-gap work. The redundant 8088
  oracle (`task-6`) was retired — it produced false failures on 8088-vs-286
  differences and the 286 corpus supersedes it.

- Timer-interrupt boot (`task-4`): the real-mode machine takes an IRQ0 timer
  interrupt via the in-guest IVT and prints to serial. `AtChipset` = 16550 UART +
  8259 PIC + 8253 PIT; PIT OUT level → PIC edge detection → `inject_irq`; deterministic
  virtual clock from the retired-instruction counter. Boot golden passes.

## Not started

See `bd ready --exclude-type epic` and `bd list --type epic`. Don't duplicate the bead list here.
