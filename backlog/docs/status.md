# Status

Snapshot of what works, what's in flight, what's broken. **Not the roadmap** — open work lives in
beads (`bd ready --exclude-type epic`, `bd list --type epic`).

Update this when a feature lands, breaks, or gets pulled. Stale status is worse than no status.

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
- **Device fidelity:** PIT Modes 2/3 adopt complete reloads at period/half-period
  boundaries and isolate incomplete LSB writes. RTC periodic PF/PIE uses a
  rational 32,768 Hz divider and rounded-up PIT deadlines, including IRQ8 wake
  from HLT; periodic and calendar phases remain an explicitly documented
  approximation. CRTC indices decode five bits; cursor comparison uses the full
  16-bit address while scanout wraps at the CPU aperture by assumption. FDC
  IRQ6 and DMA requests obey DOR bit 3. With RDY active, empty-bay SEEK succeeds,
  RECALIBRATE reports equipment check with PCN=0, and ID/data searches wait for
  reset without an interrupt. Per-device module matrices record the remaining
  unsupported cases and assumptions; changed expectations are listed in
  [scheduler adoption](design/scheduler.md#adoption).
- **IDE PIO:** one primary (1F0h/3F6h, IRQ14) or secondary (170h/376h,
  IRQ15) channel assembles from Rust configs and TOML `ide` cards. Master/slave
  fixed disks use explicit CHS geometry and volatile/readonly `SectorMedia`.
  Primary IDE splits the FDC decode at 3F6h. An assembled Bare rig transfers all
  256 IDENTIFY words with `rep insw`; firmware boards require a ROM declaring
  fixed-disk services, even for an empty IDE card. The XTIDE fixture and its
  `xub-r638-at` entry exist; option-ROM validation enforces its primary ATA
  resources and catalogue services. The IDE boot profile remains deferred. CMOS
  fixed-disk types follow the provider: option ROMs (including XUB)
  leave 12h/19h/1Ah zero; a hash-matched system BIOS that provides fixed-disk
  and declares `cmos-drive-types` derives IBM AT types 1–14/16–22 and the
  checksum. Unsupported geometries are refused rather than inventing a type-47
  convention. Kiselev declares no such capability; existing seeds are unchanged.
  Catalogued BIOS service
  declarations must match the catalogue, and omitted declarations inherit it. ATA write batches preserve complete sectors on interruption and
  reject readonly writes with ERR/ABRT. The host-only pending-write flush is
  available for future persistent sessions, and media capacity is checked at
  construction. Primary/secondary IDENTIFY interrupts reach the cascaded PIC.
  Existing ATA behavior tests are unchanged.
- **Floppy write modes:** volatile is the default (in memory only), readonly
  rejects writes, and explicit journal mode persists command-complete writes
  beside an immutable file-backed base and replays them on the next mount.
  Journal selection is available in Rust `MachineConfig` data and TOML/YAML
  floppy drive tables, with explicit per-layer journal paths and positive
  retention counts (default 10). Changing a drive image drops inherited journal
  settings while preserving readonly/volatile modes; a child can turn journaling
  off without repeating the image. Journal outputs cannot alias loaded images or
  machine-file layers, with source-positioned refusals before assembly.
  Fixture machines and goldens remain volatile. IDE journals and
  NVRAM readonly/journal remain unsupported. Offline CLI
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
- **CPU pin:** `73ed5d6ef4fc68f91cdad10b1e7c1c370363d030` (x86jit `main`).
- **CPU:** pinned x86jit `CpuMode::Real16` interpreter with exact step bounds,
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
- **Speed:** a release BIOS POST, FreeDOS boot and `DIR` session takes about
  0.50 s for roughly 11 million instructions; the CPU core takes 96–99 % of it
  and the machine side about 2 % (profile in
  [`design/machine-time-profile.md`](design/machine-time-profile.md)).

## In flight

The 286 foundation milestone is closed. Next is phase A of decision-16 (the
display through a real MDA card to a window, host keyboard input): epic
`skeleton` in beads. Translation-cache invalidation is still about half of the
DOS session's time after the CPU core's page-indexed rework; the remaining cost
was reported to the CPU core project.

## Broken / known defects

- **CPU reset is currently notification-only.** The 8042 output-port and pulse
  commands record a request; `Machine::run_bounded()` and `Machine::run()`
  report `ResetRequested` once and consume the pending event. No CPU state is
  reset yet. CLI and window run loops continue so guest reboot code can reach
  its fallback far jump through FFFF:0000, preserving the previous warm-boot
  path. Actual CPU restart is separate work. RAM, devices and virtual time are
  retained; an 8042 device reset still clears its pending request.
- **An empty 80287 socket is modelled on the I/O bus only.** Its assigned ports
  read FFh and ignore writes, with IRQ13 inactive. The pinned CPU core executes
  ungated x87 in Real16: FNINIT/FNSTCW writes 037Fh even on this board. An
  absent-FPU mode is requested from the CPU core project; the machine does not
  block x87 instructions locally.

- Multi-byte scalar accesses crossing a RAM/MMIO boundary in either direction
  end the run with `Exit::UnmappedMemory` in pinned x86jit
  `73ed5d6ef4fc68f91cdad10b1e7c1c370363d030` (READ). Its
  [`Memory::region_at`](https://github.com/unemu-org/x86jit/blob/73ed5d6ef4fc68f91cdad10b1e7c1c370363d030/x86jit-core/src/memory.rs#L1980-L2001)
  requires the whole transfer inside one region before read/write paths check
  for MMIO (`memory.rs:2170`, `memory.rs:2304`). Neither part of the scalar
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

- **BIOS/FreeDOS console through MDA:** the profiles route `0xB0000..=0xB7FFF`
  and display ports `0x3B0..=0x3BB` / `0x3BF` to the MDA card, excluding printer
  ports. Console capture and prompt detection use its scanout. VGA has independent
  CRTC state; golden cursor expectations now reflect the two cards.
  Prompt polling and final capture share the observation facade by `DeviceId`.
  `OutputConfig` selects Headless or per-display Capture sinks; boxed `FrameSink`
  subscribers receive the same frames at the existing capture boundaries.

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
  Current date is Tue 01-01-1980
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
  port 0x61 with a per-read refresh bit, and seeded CMOS NVRAM with a valid AT
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
  8259 PIC + 8253 PIT; PIT OUT → edge-detect → PIC → `inject_irq`; deterministic
  virtual clock from the retired-instruction counter. Boot golden passes.

## Not started

See `bd ready --exclude-type epic` and `bd list --type epic`. Don't duplicate the bead list here.
