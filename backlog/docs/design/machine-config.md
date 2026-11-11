# Machine-configuration layer

Design for the **declarative machine-composition layer**: a plain-data
`MachineConfig` and a single data-driven assembler that turns it into a running
`Machine`. This is the missing home between the swap traits
([spec.md §3](spec.md), decision-7) and the ~13 hand-written `run_*` functions in
`crates/cli`.

Status: **implemented.** `assembly` owns the Rust data model and validation;
`machine-file` lowers TOML/YAML descriptions to it (see [machine-files.md](machine-files.md)). Section-number cross-references
(`§N`) point at [`spec.md`](spec.md).

Related: [`spec.md §3`](spec.md) (machine assembly), decision-7 (swap seams and
its task-28 annex), decision-4 (IRQ-injection / interrupt latency), decision-5
(retired-instruction clock), decision-8 (A20 always-on).

## The problem

The swap traits exist (`Cpu`, `Chipset`, `Bus`, `Device`, `FrameSink`), but
there is **no declarative layer above them** that says *what machine to build*.
Machine composition today is hand-written assembly code, and it has drifted into
three separate, partially-overlapping copies:

- **`crates/cli` `run_*` functions** — around thirteen of them
  (`run_bios`, `run_bios_boot`, `run_freedos`, `run_freedos_interactive`,
  `run_test386`, `run_serial`, `run_boot`, `run_keyboard`, `run_rtc`,
  `run_vga_text`, `run_floppy_boot`, …). Each hand-assembles a device list, a
  port/MMIO decode map, IRQ and DMA wiring, seeds the CPU, loads images, runs a
  bespoke loop, and drains outputs. They share structure by copy-paste, not by
  construction.
- **`AtChipset::build` / `SerialChipset::build`** in `crates/chipset` — the
  trait-based composition path. It declares a decode map and a device set, but
  it **cannot build the FreeDOS machine**: the FreeDOS path needs a floppy
  controller, a DMA controller, a POST-code latch, and a system-control port
  that `AtChipset` does not assemble.
- **`assemble_bios_machine` / `assemble_bios_machine_quantum`** in `crates/cli`
  — a *third* copy that does what `AtChipset` cannot, and therefore duplicates
  most of what `AtChipset` does.

On top of the duplication, device identity is **hand-counted integer indices**.
`crates/chipset` exports `AT_UART_INDEX = 0`, `AT_PIC_INDEX = 1`, …; `crates/cli`
re-declares its own parallel set `BIOS_UART_INDEX = 0`,
`BIOS_MASTER_PIC_INDEX = 1`, … up to `BIOS_DMA_INDEX = 10`. Every decode entry,
every IRQ line, every drain call addresses a device by a literal index that must
be kept in lock-step with the order devices were pushed onto a `Vec`. Insert a
device in the middle and every later index silently shifts.

The result is combinatorial debt with no home: N machines × M devices, each
combination transcribed by hand, with three drifting copies of the composition
logic and no single source of truth for "device X lives at index Y with these
ports and this IRQ line." The traits are the right seams; what is missing is a
**declarative description of a machine** and **one** function that realises it.

## Data model

A machine is described by a single value, `MachineConfig`, that names *what the
machine is* — not how it is run (that is `RunPolicy`, below) and not any
concrete Rust device type (that stays behind the assembler, decision-7).

```
MachineConfig {
    board:   BoardModel,         // motherboard constraints; class is derived
    cpu:     CpuConfig,
    clock:   ClockConfig,
    memory:  MemoryConfig,
    devices: Vec<DeviceConfig>,   // a flat list, produced by preset functions
    drives:  Vec<DriveConfig>,    // the ONE home for attached media
    images:  Vec<LoadImage>,      // the ONE home for pre-loaded bytes
    system_bios_image: Option<usize>, // declared BIOS index, if known
}
```

`MachineConfig` and every field type derive `Eq` and `Debug`. The config is
**identifiable**: two configs are equal iff they describe the same machine. That
is what lets a golden test pin the machine it ran against, and it is the
precondition for a future snapshot/save-restore feature (deferred — see
[`deferred.md`](../deferred.md)).

### Emulator-only debug console

`DeviceKind::DebugConsole(DebugConsoleCfg { port })` constructs the diagnostic
device at `DeviceId::DebugConsole`. `DebugConsoleCfg::default()` selects 402h;
the decode must be exactly `[PortWindow::single(port)]` (byte width), with no
IRQ or DMA. An IN byte returns E9h, the SeaBIOS debug-output presence check;
each OUT byte emits to `ByteStreamKind::DebugConsole`. See
[observation.md §Debug console stream](observation.md#debug-console-stream)
for the protocol reference, capture lifecycle and wider-access policy.

This is an emulator-only QEMU debugcon protocol, not period hardware.
Validation permits `BoardModel::Bare` for isolated tests and rejects every
currently available firmware board by device **kind**, regardless of role:
`debug-console port is emulator-only; it is allowed only on board generic-386-isa`.
Decision-19 confines emulator extensions to `generic-386-isa`, which does not
exist yet. Its board bead (`upc-o0j.6`) will wire this device. There is deliberately
no machine-file syntax or preset wiring in this change.

### Emulator-only firmware configuration

`DeviceKind::FwCfg(FwCfgCfg { files, sercon_from_com1 })` constructs the
read-only QEMU firmware configuration device at `DeviceId::FwCfg`. Each
`FwCfgFile { name, bytes }` contains inline bytes. This is the smallest useful
configuration: no new image resolver or host I/O is needed; callers may resolve
images before constructing the config. Files retain configuration order, with
selectors from 0020h. Names must be unique, nonempty ASCII without NUL, at most
55 bytes, leaving a terminating NUL in the directory's 56-byte field. Sizes
are BE32, selectors BE16, reserved bytes zero; the count is BE32. There are at
most 16352 files (selectors 0020h–3FFFh).
The directory follows configuration order (with generated `etc/sercon-port`
last), whereas QEMU sorts by name (`hw_nvram_fw_cfg.c:856–860`); the
specification does not require sorting and SeaBIOS/Linux do not depend on it.

`FwCfgCfg::port_windows()` returns a **Word** selector window at 510h and a
**Byte** data window at 511h. These are exactly the required resources, with no
IRQ or DMA. A byte-wide 510h window would make the existing ISA bus split
`OUTW` into byte writes at 510h and 511h, losing the selector. The selector
must receive the original word, including its architecture bit. This uses
existing bus support without changing the bus. The QEMU Firmware Configuration
(fw_cfg) Device specification, Selector (Control) Register, defines a 16-bit
selector; it does not define selecting via two byte writes. QEMU's combined
I/O handler ignores byte writes at either port. We follow that behavior,
including ignored data writes under the deprecated write-channel bit. Byte
reads at either port stream the selected item, as in QEMU's combined handler;
a word or dword IN at 510h reaches the device as a size-2 read and returns
`UnmappedRegister`, stopping the run with a bus error under the project's
invalid-width policy (as for the ATA data port), whereas QEMU drops the invalid
access and the guest continues.
A dword OUT at 510h is split into a word cycle that selects the low word and an
open-bus cycle at 512h, whereas QEMU rejects four-byte writes; SeaBIOS and Linux
use OUTW.
Reselecting rewinds the offset; every byte read advances it, and reads beyond
an item or from an unknown selector return zero.
Bit 14 aliases the corresponding read item; bit 15 selects the separate
architecture namespace, never the generic signature or ID.

The signature (0000h) is `QEMU`; ID (0001h) is LE32 value 1: traditional
interface, no DMA. Ports 514h/518h are not decoded. READ: SeaBIOS rel-1.17.0
`fw/paravirt.c:349–426,664–689` uses `OUTW` to select, `INSB` to read and repeated
`INB` to skip when ID bit 1 is clear; it never starts a DMA transfer. Firmware
file writes fail in SeaBIOS without DMA; device data-port writes are no-ops,
as specified for the deprecated legacy write path.

`sercon_from_com1` defaults to false. When explicitly true, assembly appends
`etc/sercon-port` containing the two-byte little-endian base of the UART whose
role is `DeviceId::Uart`. Missing or wrong-kind COM1 and duplicate file names
are errors. READ: SeaBIOS `sercon.c:519` calls `romfile_loadint`, which copies a
1/2/4/8-byte file into a zeroed integer (`romfile.c:84–100`); thus 03F8h is
encoded as F8h,03h. An explicit file also works with the convenience disabled.
The device does not infer files from UART presence or add option ROMs itself.

All remaining legacy items are empty: any requested bytes read as zero. This
includes boot-menu key 000Eh = 0, rather than an enabled interactive menu.
READ: SeaBIOS `fw/paravirt.c:333–346,528–541,599–655,722–727`:

| Key(s) | Zero/empty meaning and SeaBIOS consumer |
|---|---|
| 0002h UUID | Zero UUID if read; defined but not read by this paravirt.c. SMBIOS instead uses entries/files. |
| 0003h RAM size (LE64) | Zero; this SeaBIOS paravirt.c does not read the legacy RAM-size key. Without `etc/e820`, early sizing falls back to CMOS. No board RAM-size claim is made here. |
| 0004h nographic (LE16) | Zero; prevents SeaBIOS implicitly adding a default COM1 console. Explicit `etc/sercon-port` still works. |
| 0005h current CPU count (LE16) | Zero; `qemu_get_present_cpus_count` compares with the CMOS count when RTC is present. |
| 0006h machine ID, 000Ch boot device | Empty; not read by this paravirt.c. |
| 0007h–000Bh kernel address/size/cmdline, initrd address/size | Empty; not read by this paravirt.c. These are legacy direct-kernel-loader inputs; no kernel/initrd is provided. |
| 000Dh NUMA (LE64 count, then maps/nodes) | Zero nodes; SeaBIOS registers empty NUMA romfiles. |
| 000Eh boot menu (LE16) | Zero via legacy `etc/show-boot-menu`. |
| 000Fh max CPUs (LE16) | Zero via legacy `etc/max-cpus`; no NUMA CPU map entries. |
| 0010h–0018h kernel entry/data, initrd data, cmdline address/size/data, setup address/size/data | Empty; not read by this paravirt.c; no direct-kernel-loader payloads. |
| 8000h ACPI tables (LE16 count) | Zero tables; no legacy ACPI romfiles registered. |
| 8001h SMBIOS entries (LE16 count) | Zero entries; no legacy SMBIOS romfiles registered. |
| 8002h IRQ0 override | Zero via legacy `etc/irq0-override`. |
| 8003h legacy e820 reservations (LE32 count) | Zero reservations; no legacy entries added. |

`etc/e820` is absent unless explicitly supplied among the files. READ:
`paravirt.c:733–790` uses its 20-byte address/length/type entries for early RAM
sizing; `paravirt.c:560–594` otherwise handles legacy reservations and CMOS.
No e820, ACPI, SMBIOS, CPU topology or RAM map is synthesized before a board
owns those values. `vgaroms/*`, `genroms/*` and `bootorder` are ordinary files
when explicitly configured. File delivery alone does not make SeaBIOS POST
runnable on the current 286 core.

Validation permits only `BoardModel::Bare` and rejects firmware boards by
**kind**, regardless of role, with the exact message:
`fw-cfg port is emulator-only; it is allowed only on board generic-386-isa`.
The named future board does not exist yet. There is **no machine-file syntax
or preset wiring** in this change; board wiring remains pending. See
[firmware-and-machine-classes.md §3.6](firmware-and-machine-classes.md#36-phase-b-firmware-requirements-seabios-on-generic-386-isa).

There is no host I/O or wall clock during execution. Snapshot version 1 stores
selector (LE16), offset (LE64) and virtual trace clock (LE64), preceded by the
version byte. Files and directory stay with the receiving configuration;
restore requires the same configured contents, as with other immutable device
configuration. Trace policy defaults to Off, supports the shared bounded ring,
and is excluded from snapshots. Reset rewinds to signature without discarding
files. The device is dormant and never requests an IRQ.

### A20 line and System Control Port A

`DeviceKind::SysCtrlPortA` at role `DeviceId::SysCtrlPortA` decodes exactly one
byte-wide port, 92h, with no IRQ or DMA. It is opt-in on `BoardModel::Bare`;
`generic-386-isa` can admit it when that board lands. IBM 5170 has no port 92h.
Validation checks the device kind even if assigned a different role, and rejects
it on that board with the exact diagnostic:

```text
sys-ctrl-port-a at 0x92 is absent on ibm-5170; it is allowed only on bare rigs and future generic-386-isa
```

Assembly owns one shared `devices::A20Line`. The 8042 output-port bit 1 and
92h bit 1 drive separate sources. **ASSUMED board policy:** effective A20 is
the logical OR of these sources. IBM PC AT Technical Reference, Keyboard
Controller, Output Port, describes the 8042 source; IBM Personal System/2
Hardware Interface Technical Reference, "System Control Port A (Hex 0092)",
describes the alternate source. The 8042 retains its existing enabled power-on
source and reset output high; 92h bits 0 and 1 power on clear. Existing presets
do not add port 92h.

With this OR policy and the enabled 8042 source, firmware using only 92h cannot
turn effective A20 off. SeaBIOS uses only that port (`x86.h:268–277`,
`system.c:18–35`). QEMU instead implements last-writer-wins in `pc.c`,
`handle_a20_line_change`, whose comment says "XXX: add logic to handle multiple
A20 line sources". The `generic-386-isa` board bead (`upc-o0j.6`) must choose OR
versus last-writer-wins and test SeaBIOS INT 15h AX=2400h (disable) and AX=2402h
(query). Today's ASSUMED OR default does not establish that firmware contract.

`AssembledMachine::a20_line.enabled()` exposes the effective state;
`change_count()` counts effective transitions, including attachment and device
reset/restore, wrapping at `u64::MAX`. Attaching the enabled 8042 to a fresh
line counts one transition: assembled 8042 machines start at 1. An absent
device contributes false; a machine with no A20 source reports disabled even
though CPU addressing remains flat. Final source defaults and the counter
baseline belong to the CPU-mask bead (`upc-o0j.5`). The counter is diagnostic
history, excluded from snapshots. Device snapshots retain their own source
latches; restoring one updates only that source. No clock, scheduled event or
guest read is needed to observe a change. **Not yet applied to CPU addressing;
the CPU-side mask lands with the CPU core's A20 support.**

Port 92h bit 0 requests reset through the **same** `Rc<Cell<bool>>` that the
8042 and machine use. IBM ThinkPad 560 Technical Reference, "System Control
Port A (Hex 0092)", reproduces the PS/2 wording: bit 0 is cleared by a system
reset or writing zero; a 0→1 write pulses reset, and the latch remains set
during reset so POST can read it. Bit 1 is zero after a system reset. Intel386
EX User's Manual, PORT92, CPURST, confirms that bit 0 remains one after
CPU-RESET and must be cleared before another reset. Repeated writes of 1 do
not retrigger; writing 0 does not cancel an already pending event. A CPU-only
warm restart preserves both A20 sources and the 92h latch; device reset models
system-reset clearing of these controls. **ASSUMED pulse timing:** duration is
not modelled, only the request event. The existing `ResetRequested` boundary
and reset-vector resumption require no second reset path or machine-core change.

The model uses read/write mask CBh. Bit 3 is security lock; the IBM ThinkPad
560 reference permits writing it only while it is zero. That write-once
restriction and password protection are not modelled. **ASSUMED behavior for
bits 3 and 6–7:** ordinary read/write latches initially zero; bits 6–7 retain
fixed-disk activity-light readback without a physical light. Reserved bits 2
and 5 read zero and ignore writes. Bit 4 is read-only watchdog-timeout status
(Bochs PORTS.LST, port 0092); it reads zero (no timeout) and ignores writes.
Snapshot v1 validates version, exact size, latch mask and reset flag before
mutation, and retains the absolute trace clock and pending shared reset request
without generating a new edge. As with the 8042, restore is a board-state
operation: both device snapshots must come from the same boundary because they
contain the same pending reset line. Device reset clears that shared pending
event; CPU warm reset consumes it without resetting devices.

### Board model

`MachineConfig.board` is `BoardModel::Ibm5170` or `BoardModel::Bare`. The BIOS
preset selects `Ibm5170` when floppy media is attached. Its no-media variant
is a partial motherboard rig on `Bare`; all demos and hand-wired Rust test
presets also select `Bare`. `BoardModel::class()` derives `Some(MachineClass::At)` for the AT and
`None` for a bare rig. Board identity is part of config equality, never an
execution option. The assembler still constructs only the flat device list;
selecting a board validates its wiring rather than inserting hidden devices.

`Ibm5170.motherboard_devices()` supplies the default motherboard data in bus
order. Cards follow in their declared order. The wiring reference is the **IBM
Personal Computer AT Technical Reference**, Section 1 **System Board**, under
**System Interrupts**, **System Timers**, **Direct Memory Access**, **I/O Address
Map**, **Keyboard Controller**, and **Real-Time Clock/CMOS RAM Information**.

| Order / part | Modelled wiring and coverage | Missing / deviation |
|---|---|---|
| 1. `MasterPic` | 8259 at 20h–21h, cascaded master | — |
| 2. `SlavePic` | 8259 at A0h–A1h, slave on master IRQ2 | — |
| 3. `Pit` | 40h–43h, IRQ0 | Existing 8253 model stands in for the AT's 8254 |
| 4. `Dma` | 8237 at 00h–0Fh, channels 0–3, page latches 81h–88h | No refresh DMA requester |
| 5. `DmaWord` | Second 8237 at C0h–DFh (odd ports alias the preceding even register), pages 89h–8Fh, channels 4–7; channel 4 is cascade | No cycle-level arbitration between banks |
| 6. `Keyboard` | 8042 at 60h/64h, IRQ1; D1h output bit 0 and F0h–FFh pulses restart only the CPU | A20 source latch is retained; CPU address masking is pending; pulse duration and temporary output levels are not timed |
| 7. `Rtc` | MC146818 at 70h–71h, IRQ8, NMI-mask state | Seed and virtual timing remain configurable |
| 8. `Coprocessor` | Empty 80287 socket: F0h, F1h, F8h–FFh; IRQ13 wired inactive | Empty bus socket only; CPU x87 is currently ungated |
| 9. `SysControl` | 61h control/status | Shares PIT state: refresh flip-flop from OUT1, OUT2 status, bit-0 GATE2 |
| 10. `Post` | Diagnostic latch at 80h by default | Optional, relocatable diagnostic decode |

The **IBM Personal Computer AT Technical Reference** (September 1985),
System Board, **Direct Memory Access**, pp. 1-9–1-11, supplies the two-bank
address/page wiring. The word bank uses local channel numbers 0–3 for system
channels 4–7. Channel 4 never transfers payload. Channels 5–7 step one word
per count, with physical address `(page & FEh) << 16 | address << 1`; wrapping
stays within the same 128 KiB page. Pages 8Bh/89h/8Ah serve channels 5/6/7;
8Fh is the refresh latch, not a channel-4 address extension. Register access
is still byte-wide. The System Board **I/O Address Map** (p. 1-37) and
DMA wiring decode all of C0h–DFh: SA1–SA4 select controller registers, so
`(port - C0h) >> 1` maps odd ports to the preceding even register. A CPU word
OUT is split into two byte cycles that access the same flip-flop register.
The Intel 8237A datasheet's **First/Last Flip-Flop**,
**Master Clear**, **Current Address/Word Count**, **Autoinitialization**, and
**Cascade Mode** semantics. Both DMA directions support complete words;
an unmatched trailing byte is left untouched. No assembled card requests
channels 5–7 yet. The two banks retain the existing explicit-service model:
no HRQ/HLDA arbitration latency or gating of DMA1 by DMA2 is simulated.

The same IBM reference, **I/O Address Map** (p. 1-37), **NMI and Coprocessor
Controls** (p. 1-38), **System Interrupts**, and Section 2 **Coprocessor**, assigns
F0h to clearing latched BUSY, F1h to coprocessor reset (both write-only;
software should write zero), and F8h–FFh to the 80287. F2h–F7h remain
unmapped. An empty socket does not drive read data or IRQ13: reads on every
assigned port return the emulator's open-bus byte FFh, and writes of any
value are discarded. FFh is the emulator's bus convention, not an IBM-defined
absent-coprocessor status value. This describes only the I/O bus socket.
The pinned CPU core currently executes x87 in Real16 without an FPU-presence
gate: FNINIT/FNSTCW writes 037Fh to a flat scratch word, despite the empty
board socket. An absent-FPU model is requested from the CPU core project;
this repository does not intercept or block x87 instructions.

**Keyboard Controller**, **Commands** and **Output Port** (pp. 1-53–1-55)
connect output bit 0 to active-low System Reset. D1h lowering that bit latches
a request; F0h–FFh pulse commands latch one whenever command bit 0 is zero,
including FEh. Odd pulse commands do not request reset. D1h uses only the
falling edge of output bit 0: the model restarts immediately even if the latch
stays low, and further low writes do not request another restart. On a real
AT, the low level holds the CPU in reset until released; that sustained reset
level is not modelled. The approximately
6 microsecond pulse is represented as an event rather than a timed pin level;
D0h reads the retained output latch. The pending request survives device
snapshot/restore and is cleared by device reset. Assembly shares a signal
with the machine without teaching the machine core about an 8042. At the
completed instruction boundary the machine returns `RunOutcome::ResetRequested`,
even at a coincident cap, and consumes the pending event. `Machine::run()`
also returns that outcome explicitly. The machine retains a reset-pending flag:
the next `run_bounded()` or `run()` resets the CPU before execution or a cap
check. An explicit `Machine::reset_cpu()` clears that flag only after reset
succeeds. A persistence error after the reported boundary leaves reset pending;
resuming cannot continue the old instruction stream. The run layer checks persistence at
that boundary, resets the CPU, and continues from F000:FFF0 with the same
absolute cap. CLI natural, prompt, post-prompt and machine-file runs all use
this path; prompt stimuli are not repeated. The window performs the same
reset after its persistence check. It counts requests separately from performed
resets, logs `cpu reset` after the first performed reset, and logs the performed
reset total on event-loop exit, including error exits. A persistence failure
before reset produces no reset diagnostic. Saturating counters and message
selection use no host clock.
A reset neither seals a journal generation nor ends a media session.

Intel **80286 and 80287 Programmer's Reference Manual**, §5.3 **System
Initialization**, Table 5-3 **Processor State after RESET**, specifies
CS=F000h, IP=FFF0h, FLAGS=0002h and DS=ES=SS=0. The physical processor uses
hidden CS base FF0000h until the first far transfer; the 1 MiB Real16 model
fetches at FFFF0h. General registers are unspecified by the table and are
preserved. MSW=FFF0h and IDTR base=0, limit=03FFh (§10.4 Initialization) are
not modelled by the Real16 core. The backend abandons pending execution,
IRQ/completion latches and interrupt shadows before installing these registers,
and initializes its emulated x87 environment with FNINIT.

The IBM reference's **Keyboard Controller Output Port** resets only the CPU:
RAM, CMOS (including shutdown byte 0Fh), peripheral state, retired-instruction
and completed-step counters, and virtual time survive. The 8042 output-port
latch survives too: D1h bit 1 sets its retained A20 state, and pulse commands
do not change that latch. Actual address-line masking is not implemented;
reset does not change that limitation. An explicit device reset still clears
its pending request.

Every `Ibm5170` requires the 8237 in its canonical position, even if an FDC
entry is undecoded or unwired. The no-media BIOS preset keeps that historical
partial machine on `Bare`: an undecoded, unwired FDC derives CMOS drive A: while
no 8237 is assembled. Changing that preset's board identity affects validation
only, preserving its device list and execution.

Omitting or moving `Post` away from 80h changes the expected page-latch range
to 80h–88h. POST is validated before deriving this range, so a wrong kind or
decode is diagnosed against `Post` itself. Removing the optional POST role
closes up the order.

Board validation follows unique-ID validation and precedes general wiring
checks. Each rule returns a stable, exact-message-tested `ConfigError`:

| Rule | Error |
|---|---|
| Required motherboard role is present | `BoardDeviceMissing` |
| Motherboard devices precede cards, in the table's order | `BoardDeviceOrder` |
| Entire `DeviceConfig` matches board data, except the contents of `CmosCfg` | `BoardDeviceWiring` |
| Card IRQ is 3–7, 9–12, 14 or 15; 0/1/2/8/13 stay reserved | `BoardCardIrq` |
| Card DMA channel is 0–3, the modelled 8237's range | `BoardCardDma` |
| Card DMA controller is the board's `DeviceId::Dma` | `BoardCardDmaController` |

`BoardDeviceWiring` names the device, differing field, expected value and actual
value. The full comparison includes port-overlap allowances and scheduling
`access_dependents`; neither can silently override board wiring. RTC
initialization and virtual timing remain configurable. A literal-table unit test pins the board
ports, IRQs and DMA requests independently of both preset and validator.

There are no resource override rules in this implementation. Port-overlap
allowances do not authorize IRQ or DMA overrides. Existing IRQ conflicts and
DMA-controller-presence checks still apply. `Bare` skips board rules and firmware service requirements;
general decode, resource and drive-binding validation still runs.

### Drive binding and write mode

```rust
DriveConfig {
    kind: DriveKind,             // Floppy or HardDisk, with geometry
    image: ImageSource,
    controller: DeviceId,        // named device, never a global slot number
    unit: u8,                   // unit local to that controller
    write: WriteMode,           // Volatile (default), ReadOnly, or Journal
}
```

A binding must name an existing device (`DriveControllerMissing`) of the right
kind (`DriveControllerKind`): `Floppy` on `DeviceKind::Fdc`, or `HardDisk` on
`DeviceKind::Ide`. The unit must be below the controller's declared slot count
and hardware limit (four for FDC, two for IDE; `DriveUnitUnavailable`). Two drives
cannot bind to the same `(controller, unit)` (`DuplicateDriveUnit`). Each error
has an exact-message unit test. These rules apply on both boards.

AT CMOS describes only diskette drives bound to the primary `DeviceId::Fdc`.
Floppies on another named controller do not contribute type nibbles or the
equipment-byte drive count, even when their local unit is 0 or 1. This rule also
applies to `Bare` rigs using the same CMOS derivation. A primary 1.2 MB unit 0
and a secondary 720 KB unit 0 therefore derive `floppy_types = 0x20` and
`equipment = 0x01`.

`WriteMode::Volatile` replaces `write_protect = false`: guest writes modify only
the mounted in-memory image. `WriteMode::ReadOnly` replaces `write_protect = true`
and uses the same `Drive::new_read_only` constructor: WRITE DATA reports the
existing not-writable status before requesting DMA or modifying media. Neither
mode writes the host image or creates host output.

`WriteMode::Journal { path, keep_generations }` requires an explicit journal
path and positive retention count. `WriteMode::journal(path)` supplies the
default count of ten. The enum makes journal-only fields on other modes
unrepresentable. Journal mode requires `ImageSource::File`, a floppy and
512-byte sectors. Assembly rejects normalized-path and existing-file-identity
aliases (including hard links) between journal/lock outputs, any mounted drive
or load image, and other journal/lock outputs before creating files. It reads
the base read-only once, then opens, recovers and replays the journal before the
first guest instruction. A mount refusal names the drive and its cause.

The assembled session owns per-drive persistence handles. Embedders use
`run_session` (or check handles at their own completed service boundaries),
`checkpoint` for explicit recovery points, and `finish_persistence` only after
successful execution or a normal policy-limited end. Dropping a failed run
leaves its commands unsealed for recovery; host failures never enter controller
status or virtual time. The retention count is retained for the separate
retention service; automatic pruning and offline tools are not wired yet.
See [Persistent media](persistent-media.md#assembly-session-api).

### `CpuConfig` and `ClockConfig` — minimal but the right home

These two structs start **deliberately minimal** but exist so that future
bus-speed and CPU-type work is a *field addition*, never a re-architecture.

```
CpuConfig {
    chip:         ModelledChip,   // marker for the modelled part (today: one variant)
    reset_vector: SegOff,         // CS:IP the CPU starts at
    segments:     SeedSegments,   // DS/ES/SS + SP seeded at reset (see below)
}
```

The CPU core models segment *bases*, not selector loads (a real-mode guest that
does not run its own segment setup relies on the machine seeding CS/DS/ES/SS/SP;
see [`deferred.md`](../deferred.md) and the `new_real16*` constructors in
`crates/cpu`). `CpuConfig` is where that seeding is declared, so the assembler
picks the right constructor from data instead of the caller picking it by hand.

```
ClockConfig {
    retired_per_pit_clock:             u64, // positive numerator A
    retired_per_pit_clock_denominator: u64, // positive denominator B
}
```

The CPU rate A/B counts completed steps per PIT input clock. An instruction is
one step and each completed REP iteration is one step. The carried remainder
preserves the ratio across CPU calls. This is a modelling ratio, not a nominal
frequency. Block quantum and interrupt-timing selection have been removed:
interrupts are accepted only at completed instruction boundaries after bus and
DMA effects, with IF and STI/SS-load shadows respected. Cooperation size is not
machine identity. Production always uses the deadline engine; the non-default
`reference-scheduler` feature exposes a per-clock oracle only to tests, outside
`MachineConfig`. Assembly probes zero-step execution and returns a clear
`StepBoundedCpuRequired` config error for an unsupported CPU.

The `ibm-5170` default is **8/13 completed steps per PIT clock**, calibrated
for a 6 MHz AT against the IBM Personal Computer AT Technical Reference BIOS
listing, TEST1 checkpoint 11 (speed/refresh clock rates). **MEASURED by the
supervisor** with user-supplied firmware and the 80286 core: `4/7` through `2/3`
passes; `11/20` stops at checkpoint `04`, and `27/40` stops at checkpoint `11`.
The loop uses three steps per polling iteration and two (`DEC BL / JNZ`) per
36-PIT-clock bit-4 cycle. **DERIVED**: `n = (36R - 2)/3`, and centring 256
cycles on 1710 iterations gives `R = (3 × 1710/256 + 2)/36 ≈ 0.6122`.
The selected `8/13 ≈ 0.6154` lies near that centre. Board defaults, the shipped
IBM AT preset, and the FreeDOS TOML/YAML examples use this ratio; the IDE
example inherits it from FreeDOS. Explicit machine-file rates override the
board default. Bare rigs, including the historical no-controller POST rig,
retain their own rates. No nominal-MHz axis or per-opcode cycle table is implied.

Neither struct carries a frequency axis or a behavioural CPU-type toggle today —
see [Non-goals](#non-goals) for why, and why keeping the structs is nonetheless
the point: when a real consumer and a real oracle appear, they arrive as new
fields here rather than a redesign of everything above.

### `MemoryConfig` — a semantic memory map

`MemoryConfig` describes the address space as regions with *kinds*, not a flat
byte count. Two things it must express that a naive "N kB of RAM" cannot:

- **CPU A20 masking.** CPU addressing remains a flat 1 MB+ linear space with
  no A20 wrap (decision-8, §4.4). The board line and its 8042/92h source latches
  are observable, but applying the effective state awaits CPU-core support.

  **A20 as a model constant.** A20 is always-on (decision-8, §4.4). The memory
  map is a flat 1 MB+ linear space with no wrap; A20 is not a configurable knob.
  The config records this as a fixed property of the modelled machine, so nobody
  reintroduces it as a per-machine option.
  The always-on statement describes current CPU address translation; the board
  line separately exposes its controller-source state.
- **Adapter space belongs to the board.**
  **Firmware scan space is not system RAM.** IBM PC AT Technical Reference,
  Section 1 "System Board", **System Memory Map** assigns C0000h–DFFFFh to
  I/O expansion ROM, E0000h–EFFFFh to reserved system-board space, and
  F0000h–FFFFFh to system-board ROM. On a board with a firmware catalogue,
  assembly fills gaps from C0000h up to the system BIOS image's base with
  read-only zeroes, carving around ROM images and card MMIO windows. This
  physical address policy is independent of the selected firmware's option-ROM
  scan window, including a diagnostic with no scan window. With Kiselev at
  F8000h, the gap mapping still ends at F8000h; with a 64 KiB BIOS it ends at
  F0000h. Guest writes cannot change gap bytes, and `RamLoad` cannot create
  hidden firmware headers. Zero is retained to preserve the goldens; whether
  empty sockets should read FFh is an open hardware-fidelity question requiring
  authoritative electrical evidence. The historical no-floppy `Bare` rig
  retains its map.

The MMIO list retains per-device windows. For CPU mapping, assembly sorts them
by address and coalesces each run of touching windows into one `RegionKind::Trap`
region. Non-adjacent windows stay separate; gaps and RAM are never included in
the merged trap regions. The bus retains the per-device decode map and splits
cross-window accesses into little-endian sub-accesses. This lets accesses across
adjacent devices reach the bus despite the pinned CPU's single-region containment
requirement. RAM/MMIO crossings remain a [known CPU limitation](../status.md#broken--known-defects).

### Port window width

`PortWindow.width` and the corresponding `bus::PortRange.width` in `DecodeMap`
use `PortWidth::{Byte, Word}`. The enum excludes unsupported widths.
`PortWidth::default()`, `PortWindow::new`, `single`, and `overlapping` all select
`Byte`; `.with_width(PortWidth::Word)` opts in. Every existing preset retains
byte cycles. Assembly carries each window's width unchanged through decode-map
sorting. Validation rejects a word window starting at an odd port with
`UnalignedWordPortWindow`. A kind-based table declares the permitted word
register addresses; every even cycle address covered by a word window must be
listed for that kind, otherwise validation returns `UnsupportedWordPort`.
The production table is empty because no assembled device kind currently
implements word registers. Synthetic declarations exercise the accepting path
only in validation tests; POST remains byte-only. The ATA card will declare its
data register when it is wired. Board validation also checks width as part of
the fixed motherboard wiring and reports a changed word width explicitly.

The width describes a cycle starting at a decoded port, rather than ownership
of individual data lanes. An aligned word in a word window reaches the device
once with size 2, even when the window covers only that even port and the next
port belongs to a byte window or is undecoded. This permits a word data register
beside a separately decoded byte register without reading both registers.
A word beginning at an odd port uses two independently decoded byte cycles.
An aligned 32-bit access to word windows uses word cycles at port and port+2. Each cycle
selects width from the first matching window at its own starting port, so mixed
windows can produce word/byte/byte or byte/byte/word sequences. Values are
composed/sliced little-endian; port numbers wrap at 16 bits. A fault stops after
completed cycles. Traces describe actual device/open-bus cycles, including size.

The hardware reference is the **IBM Personal Computer AT Technical Reference**,
Section 1, **System Board**, **I/O Channel**, **I/O Channel Signal Description**,
**-I/O CS16** (IOCS16#): the card selects a 16-bit I/O transfer. The odd-alignment
and mixed-window rules above define the emulator's dispatch contract; physical
byte-lane signalling and wait states are not modelled. No ATA card or device
configuration is added here; its data window can opt in when the card is wired.

### `DeviceConfig` — one device, fully described

The device list is **flat**. Each entry fully describes one hardware unit and
carries a stable identity:

```
DeviceConfig {
    id:    DeviceId,              // stable name; replaces the hand-counted indices
    kind:  DeviceKind,           // which modelled part (UART, PIC, PIT, …)
    ports: Vec<PortRange>,       // port range(s) it decodes
    mmio:  Vec<MmioRange>,       // MMIO range(s) it decodes (e.g. text buffer)
    irq:   Option<IrqLine>,      // the PIC input it drives, if any
    dma:   Option<DmaWiring>,    // requester/controller wiring, if any
    access_dependents: Vec<DeviceId>, // timers an access can affect; empty by default
}
```

`DeviceId` is the single fix for hand-counted indices. Devices are named, not
numbered; the assembler (below) returns a `DeviceId -> bus-index` map so every
drain/inject addresses a device by name. Inserting a device no longer shifts a
column of literals.

`IrqLine` and `DmaWiring` are the home for wiring the CLI frontend used to
declare by hand. This is the concrete fix for the partial decode-ownership
violation noted under decision-7: the IRQ/DMA relationships that were scattered
across `run_*` bodies now live in the device data the preset produces.

`access_dependents` declares a directed scheduling dependency: an access to
this device can change another device's next event. Validation requires every
dependent to be present and distinct from the source. Assembly resolves each
role to a valid bus index and registers the edge with `Bus::add_access_coupling`,
which also rejects invalid indices and self-coupling. Repeated edges are
idempotent; directed cycles are allowed because synchronization does not recurse.
Every indirectly affected timer must be listed explicitly.

Before a port cycle, MMIO fragment, or mutable borrow, the bus synchronizes the
target first, then its dependents in configuration order, regardless of edge
declaration order. Unrelated devices receive no access synchronization, and
legacy every-clock devices receive no extra sync. This makes dependent deadlines
start at the current access clock rather than their earlier lazy-sync time. The
field does not implement hardware signal wiring. No preset needs it today:
the shared PIC faces are dormant and have no autonomous timer, and system
control does not yet drive the PIT's speaker gate. See
[scheduler.md §Adoption](scheduler.md#adoption) for cache and settle ordering.

### Single homes (no triplication)

Two data-model decisions exist specifically to prevent the same information
living in three places:

- **`images`: one `LoadImage` list, not three.** Firmware ROM, a `memory.rom`
  blob, and a CPU code blob were three separate concepts. They collapse to one
  list of `LoadImage { base, bytes, kind: Rom | RamLoad }`. `kind` says whether
  the bytes land in a ROM region or are loaded into RAM (SMC-visible per
  decision-6). One list, one meaning.
- **`drives`: one top-level list, routed by controller and unit.** Attached media were split
  between an ad-hoc floppy argument and a controller's own drive list. They
  collapse to one top-level `Vec<DriveConfig>`, each routed to its controller by
  `controller: DeviceId` and `unit: u8` fields. The floppy controller does not own a private drive
  list; drives are attached to the machine and routed to controllers.

`DriveConfig` and floppy geometry are fleshed out in task-34; the top-level
placement and controller-local unit routing are fixed here.

### The fixed-disk (IDE/HDD) attachment seam

`DriveKind` is `Floppy { geometry } | HardDisk { geometry }`. The floppy side is
wired end-to-end: the assembler routes each `Floppy` drive into the µPD765's
drive slots by `controller` and `unit`, and the CMOS `floppy_types` nibbles and
equipment byte are *derived* from the attached floppies, so attaching a second
drive of a different geometry needs no manual CMOS edit — the whole point of the
"attach different drive types" story.

`HardDisk` binds to `DeviceKind::Ide(IdeCfg { base, control })` by controller
and unit: master 0, slave 1 (a slave requires a master). One channel is supported,
with base `0x1F0` or `0x170`, control at base + `0x206`, IRQ14 or IRQ15,
and no DMA. Only the data register at base supports word cycles; all other
task-file and control registers use byte cycles. Lowering splits primary FDC
windows with `FdcCfg::port_windows(true)`; Rust callers supply the same split.
The secondary channel leaves the FDC's legacy decode intact.

Assembly mounts `InMemoryMedia` through `SectorMedia`, selecting volatile or
readonly writes. Geometry must have 1–16383 cylinders, 1–16 heads, 1–63 sectors
per track and 512-byte sectors; the image must exactly match it. Journal mode
remains floppy-only. ATA submits one batch per completed write command, including
complete sectors before an address failure, command replacement, diagnostics,
reset or successful snapshot restore. A partial sector is discarded. Protected
writes finish with ERR/ABRT and submit no batch. Mounted media and the completed
prefix of an active command remain outside snapshots; restore does not roll back
committed sectors. The host-only `Ata::flush_pending()` submits any complete
prefix without changing guest-visible state. A future persistent IDE backend
must call it only at session end, when the guest will no longer finish or abort
the pending command; calling it mid-command splits one command into multiple
batches. The host reaches ATA through the device's `as_any_mut` seam.
`SectorMedia::sector_count()` bounds the advertised ATA capacity at construction. ATA-2 §§6.2.8 and 8.4 define abort reporting and
PIO data-out.

CMOS fixed-disk types follow the firmware provider. Registers 12h, 19h and
1Ah stay zero when an option ROM provides `fixed-disk`, including XUB. They
are derived from the primary IDE `HardDisk` units only when the hash-matched
system BIOS catalogue entry both provides `fixed-disk` and declares the
`cmos-drive-types` capability. Kiselev does not declare that capability.
The IBM AT September 1985 fixed-disk tables supply standard types 1–14 and
extended types 16–22 (nibble F plus 19h/1Ah). CHS duplicates choose the lowest
number. Other geometries return `UnsupportedCmosDiskGeometry`: IBM reserves
type 47, so no vendor-specific user-defined layout is invented. See
[firmware and machine classes](firmware-and-machine-classes.md), Derived
configuration, for the reference and firmware-source evidence. The XTIDE
boot profile remains separate work.

A byte cycle on the ATA data register (`in al, 1F0h`) stops the run with
`UnmappedRegister`: this AT model accepts only a word cycle at the data
register, matching its IOCS16# word transfer contract. XTIDE AT builds use
word port I/O. The other task-file registers remain byte-wide.

## Firmware services

`LoadImage.provides: Vec<FirmwareService>` declares guest services supplied by
a ROM. The initial vocabulary is deliberately small:

| Variant | File vocabulary | Guest interface |
|---|---|---|
| `Floppy` | `floppy` | INT 13h diskette services |
| `FixedDisk` | `fixed-disk` | INT 13h fixed-disk services |

System BIOS and option ROM images use the same field. The BIOS preset declares
`Floppy`, matching the machine-file examples; this validation metadata does not
change guest execution. Firmware identification uses the separate catalogue
below. A recognized BIOS or option ROM supplies its catalogue services when the
declaration is omitted or empty; a nonempty restatement must match the catalogue
set. A conflict returns `FirmwareProvidesMismatch` with both sets. Unlisted
firmware under the escape option uses its declared services.

On firmware boards (every board other than `Bare`), requirements follow from a
table of device kinds. An FDC with at least one drive bound to its device id
needs `Floppy`. The table records the matching kind, required service and whether
a bound drive is necessary. IDE requires `FixedDisk` even without a bound drive.

Validation checks required services against the union of ROM declarations and
returns `NoProvider { device, service }` in device/table order. `Bare` rigs skip
these requirements, and an FDC without bound drives needs no provider. A ROM
service does not require a consumer. RAM loads with nonempty `provides` fail with
`ServicesOnRamLoad`; a RAM program cannot satisfy a firmware requirement.

## Firmware catalogue

`BoardModel::firmware_catalogue()` owns the reviewed `FirmwareEntry` values.
The IBM 5170 catalogue contains `xub-r638-at` (8 KiB, fixed-disk service,
primary ATA requirement) and `kiselev-xi8088-1.0.2`, the unmodified 32 KiB
Xi 8088 build pinned by `vendor/kiselev-bios/fetch.sh`. Its GPL-3.0-or-later
licence, upstream revision, SHA-256, load rule, service metadata, required
8042/RTC/cascaded PIC parts, 80286 minimum and tested status are data in
`crates/assembly/src/firmware.rs`. `Bare` owns no catalogue and assembly skips
both catalogue and BIOS-placement checks for these hand-wired rigs.

The catalogue also lists user-supplied IBM 5170 system BIOS versions
1984-01-10, 1985-06-10 and 1985-11-15: 64 KiB, top of first MiB,
C8000h..E0000h scan window, floppy/fixed-disk services, CMOS drive types,
and 8042/RTC/cascaded PIC requirements. Their hashes and alternative chip
layouts are metadata only; status remains Listed and no preset selects them.
See `vendor/user-roms/README.md` and firmware-and-machine-classes §3.5.
`ImageSource::Interleaved` is restricted to system ROMs and resolves to the
same bytes and identity as a pre-interleaved file.

Assembly resolves file sources once, then hashes the actual bytes before any
ROM mapping; inline and generated system BIOS images follow the same check.
Option ROMs are also hashed and allowlisted; `OptionRomNotListed` lists the
known adapter builds. The ROM ending at `0x100000` is the system BIOS,
regardless of image-list order. Its size must be positive and divisible by 8 KiB,
and its end must be `0x100000`. This check precedes the existing general region bounds
check, so an overlong system BIOS reports `BiosPlacement`, including its base
and an exact size (bytes when not divisible by 1024). Catalogue boards reject a
missing BIOS (`MissingSystemBios`), a reset other than power-on
`ResetVector F000:FFF0` (`FirmwareReset`), RAM shadowing the BIOS or its reset
byte (`SystemBiosOverlapsRam`), and other overlapping images (`ImageOverlap`).
Since every valid system BIOS ends at 1 MiB, at most one can satisfy placement
without an overlap. If none ends at 1 MiB, `system_bios_image` identifies the
explicitly declared BIOS; its invalid size or end reports `BiosPlacement`.
The BIOS preset (including the CLI `--bios` path) and machine-file lowering set
this index. When the index is unset, a sole ROM is also a BIOS candidate and
reports `BiosPlacement`. Zero ROMs, or multiple undeclared ROMs with no top-aligned
candidate, report `MissingSystemBios`; address order never resolves ambiguity.
Callers reordering images must update the declared index. File-backed ranges
and identity are checked after resolution; machine files validate their explicit
BIOS placement at its defining image origin.
Other image/region errors retain their existing variants. The catalogue's
`base` field is informational for system BIOS entries: all use the common top
alignment rule, which must also be enforced before looking up unknown bytes.
Option ROMs require explicit 2 KiB-aligned bases and must fit the BIOS entry's
`scan_window` (Kiselev: [C8000h, F8000h)). The 55AAh signature, nonzero length
byte × 512 equal to the image size, zero byte-sum checksum, and absence of image
overlap are checked before mapping. Firmware without a scan window refuses
option ROMs with `FirmwareDoesNotScanOptionRoms`. The scan window controls
firmware discovery and placement validation only; it does not decide whether
empty adapter addresses are mapped. `AssembledMachine.option_roms` contains the
resolved images with effective services in ascending address order. Suggested
catalogue bases remain informational; machine files specify the base.
Unknown bytes report `FirmwareNotListed` with the full SHA-256 and
every known entry's distribution and fetch script. Listed entries must also
match the board, CPU minimum and required parts (rules 7–9 in the firmware design).

`AssemblyOptions { allow_unlisted_firmware: true }` is a run-only escape hatch,
passed to `assemble_with_options` or `assemble_with_observation_and_options`.
It never enters `MachineConfig`, machine files or goldens. Accepted unknown
hashes are returned in `AssembledMachine.unlisted_firmware`; frontends print
one warning per hash. The option only relaxes catalogue membership: listed
entries still satisfy compatibility checks, and every checked BIOS still
satisfies placement. Required BIOS presence, memory isolation and power-on
reset are also mandatory with the escape option. Synthetic IBM 5170 assembly
tests may opt in for unknown hashes; rigs needing custom entry points select
`Bare` explicitly without changing their device set.

The historical no-floppy POST preset remains a `Bare` rig to preserve its
hardware and golden. The CLI POST runner and scheduler comparison explicitly
check its BIOS against the IBM 5170 catalogue through the shared assembly
helper `check_board_system_bios` before assembling that partial rig. Complete
BIOS/floppy and FreeDOS machines use normal strict assembly; all existing
golden expectations remain unchanged.

## RTC start and NVRAM source

`CmosCfg { ram: CmosRam, start: RtcStart, nvram: NvramSource, timing }` separates the fixed
calendar seed from the initial store. `RtcStart` holds binary `year`, `month`,
`day`, `hour`, `minute`, and `second`; the weekday is derived, with Sunday = 1.
Its default is exactly **2000-01-01 00:00:00, Saturday (7)**. This implementation
accepts valid dates in 1980–2099, matching the existing two-digit year and
four-year leap cycle; invalid dates, hours, minutes and seconds fail with
`InvalidRtcStart` before assembly, naming the first invalid field and showing
the supplied time as `YYYY-MM-DDTHH:MM:SS`. Host time has no config variant or
runtime clock read. The default RTC second is exactly **3,579,545 / 3 PIT
input clocks**, derived from the nominal **14,318,180 / 12 Hz** timer input
(IBM PC AT Technical Reference, System Board, **System Timers** and
**Oscillator**). The calendar phase counts thirds of a PIT clock. For the
32,768 Hz periodic divider, `oscillator_phase` carries a remainder in units of
1/3,579,545 of an oscillator clock: each PIT clock adds `3 * 32768`, and division
by 3,579,545 yields whole oscillator clocks for `divider_phase`. Both counters
retain their remainders; deadlines round up to whole PIT clocks, so batching
cannot accumulate drift. The UIP window keeps the existing approximate
1 ms policy: floor(3,579,545 / 1000) thirds = **1193 PIT clocks** before each
update. Motorola MC146818 datasheet, **Update Cycle**, specifies the once-per-second
update and 244 µs from UIP assertion to update start; the model retains its
longer polling window. Explicit `CmosCfg.timing: Some((second_ticks, uip_ticks))`
and `with_timing(second_ticks, uip_ticks)` retain integer PIT-clock units,
including the CLI RTC demonstration's small timing ratio. Snapshots include the
phase-unit scale as well as the calendar and periodic remainders.

`CmosRam::Bytes64` is the default MC146818 part, decoding six index bits.
`CmosRam::Bytes128` selects DS12887-compatible extended RAM: 40h–7Fh are
independent general storage, and 00h–0Dh use identical clock/control logic.
Dallas Semiconductor DS12887 datasheet, **Address Map**, describes 14
clock/control bytes and 114 general RAM bytes. ASSUMED: PC/AT port 70h bit 7
remains the external NMI mask; it never selects RTC RAM in either variant.
`ibm-5170` requires the 64-byte part and rejects a 128-byte RTC with
`board ibm-5170 requires a 64-byte RTC: the IBM AT has an MC146818`.
Bare Rust rigs may select either size. No machine-file size syntax is added;
files continue to lower to the 64-byte part.

64-byte snapshots retain version 4 and their exact layout. 128-byte snapshots
use version 5 with the same fields following the longer store. Restore accepts
version 4 only on a configured 64-byte device and version 5 only on a configured
128-byte device. A mismatch is rejected atomically with
`snapshot RAM size does not match configured RTC`; restore never changes the
configured capacity. Legacy 64-byte snapshots still restore on the 64-byte part.
The journal observer still exposes only 0Eh–3Fh, even on the extended part.

`NvramSource` has four variants:

| Variant | Initialization |
|---|---|
| `Derived` (default) | Existing machine-derived AT seed and checksum |
| `Blank` | Existing unseeded chip, retaining its control defaults |
| `Image([u8; 64])` | Volatile writable bytes copied without checksum repair; read-only status initialized to power-on values |
| `Journal { image, expected_sha256, journal, keep_generations }` | Assembly reads and verifies the immutable 64-byte file, replays general RAM 0Eh–3Fh except 0Fh and retains RTC defaults for 00h–0Dh |

Images and journals remain 64-byte only. A 128-byte RTC combined with either
source is rejected before file access with
`128-byte NVRAM images are not supported yet`. `Derived` and `Blank` work
with either capacity; no journal format changes are made.

The fixed array enforces volatile image length at the Rust boundary. The
machine-file loader rejects other lengths; journal assembly checks the bytes
actually mounted against the digest before opening outputs. Journal paths and
retention follow drive images. `CmosCfg`, `NvramSource` and `DeviceKind` are
cloneable data because journal paths are owned. Readonly NVRAM is rejected:
the MC146818 has no write-protect input and AT POST writes 0Eh/0Fh.

Journal sessions share `PersistenceHandle` with drives. Changed general RAM,
excluding shutdown byte 0Fh, appends only before CPU reset and at finish. Run
stops, polls and frame slices only check error latches. Reset records keep the
session generation open; finish closes it once with the session-end reason and
runs retention. An execution error leaves complete commands for recovery on the
next mount. Clock/control and 0Fh writes never trigger persistence. At mount,
0Fh takes the base image's value; live CPU resets preserve it within the session.
See `persistent-media.md` § CMOS NVRAM journals for clock/century consequences,
crash limits and offline tools.

For volatile images, time-register backing bytes are preserved, but reads use
`start`, encoded according to the image's register B. Journal mode instead uses
RTC defaults: B=02h (24-hour BCD), A=00h, zero alarm registers, C=00h and D=80h.
For volatile images, loading clears A's read-only UIP
bit and all of C, recomputes IRQF, and sets D to the device's valid-RAM value
(80h, reserved bits clear). This prevents stale image status from raising IRQ8
at power-on or producing an unrestorable snapshot. Subsequent control reads
retain normal semantics: A overlays live UIP, C clears flags after reading,
and D reports valid RAM/time. These register meanings follow the Motorola
MC146818 datasheet, **RAM**, **Time, Calendar, and Alarm Locations (Table 3)**,
and **Registers A–D**. Volatile general RAM and alarm bytes are not normalized; neither image mode
repairs the AT checksum. `Derived` with the default start retains
every register and backing byte of the prior construction; tests compare the
entire device snapshot and all 64 port-visible registers, alongside unchanged
machine goldens.

The default AT layout does **not** populate the BIOS century byte at 32h:
`Derived` leaves it at 00h and `Image` retains the supplied byte. The optional
`cmos-qemu-layout` firmware capability instead derives 32h in BCD from
`RtcStart.year / 100`. An NVRAM image can supply 32h for other firmware.
The live year register remains the two-digit year, including the 99-to-00 rollover;
the derived century is an initial seed, not an automatic rollover update.

## The assembler

`fn assemble(config: &MachineConfig) -> Result<Assembled, ConfigError>` is the
one function that turns data into a `Machine`. It replaces `AtChipset::build`,
`SerialChipset::build`, and `assemble_bios_machine` with a single realiser.

**Placement (hard constraint).** The assembler lives in a **new crate above the
`machine` crate**. The machine core must never name a concrete device or CPU
type (decision-7, §3.2); it depends only on the traits. The assembler is
precisely the layer that *does* know the concrete `DeviceKind -> Box<dyn Device>`
mapping, so it sits above `machine`, alongside where the concrete devices are
known — not inside the core. Built in task-30.

**Return value.** The assembler returns the built `Machine` **and a
`DeviceId -> bus-index` map**. Drain and inject helpers address devices through
this map, by id, so the hand-counted `*_INDEX` constants disappear entirely.

**CMOS defaults to a derived configuration.** The assembler derives floppy
types, equipment and memory sizes from the attached drives and memory map.
IBM fixed-disk types additionally require the capable system BIOS provider
described above. `derive_cmos_config` returns a fallible, complete 64-byte seed, including
12h/19h/1Ah and the AT checksum at 2Eh/2Fh (sum of 10h–2Dh). An explicit NVRAM
image bypasses derivation; the caller is then responsible for its contents. Calendar initialization is separate
from the NVRAM source, as described below.

A hash-matched system BIOS may opt into `cmos-qemu-layout`; no production
catalogue entry enables it yet. It adds QEMU/Bochs conventions without changing
the existing equipment and floppy bytes or checksum range:

- 30h/31h duplicate 17h/18h: RAM above 1 MiB and below 4 GiB, in whole KiB,
  saturated at 65535. 34h/35h count RAM from 16 MiB to 4 GiB in whole 64 KiB
  units, saturated at 65535. 5Bh–5Dh hold the low 24 bits of the count of
  whole 64 KiB units above 4 GiB, as QEMU writes them. RAM is counted in its
  physical windows, so high RAM does not inflate low-memory fields.
- 32h holds the RTC start century in BCD. 3Dh=21h selects floppy, then hard
  disk; 38h=00h enables floppy boot-signature checks. This corresponds to
  QEMU `-boot order=ac` with its default `fd_bootchk=true`. QEMU 29760ef's
  default PC order is `cad`: 3Dh=12h and 38h=30h. There is no new boot-order
  machine-file syntax. 5Fh=00h means one CPU.
- Primary IDE drives use nibble F at 12h, type 47 at 19h/1Ah and nine-byte
  geometry records at 1Bh/24h: cylinders, heads, FFFFh write precompensation,
  C0h control (plus bit 3 for more than eight heads), cylinder landing zone,
  sectors per track. These describe hardware even with a fixed-disk option ROM.
  39h assigns two translation bits to each drive, primary master first, then
  primary slave, secondary master and secondary slave. READ: QEMU
  `hw/block/block.c`, `blkconf_geometry`, maps explicit CHS with AUTO
  translation through `hw/block/hd-geometry.c`, `hd_bios_chs_auto_trans`:
  none within 1024/16/63, LBA otherwise. Validation caps geometry at
  16383/16/63, so in valid configs this reduces to cylinders > 1024.
  The current config permits one IDE channel at a time; either primary or
  secondary maps to its conventional slots.

`cmos-qemu-layout` owns its disk convention independently of
`cmos-drive-types`; combining them is rejected during firmware validation with
`firmware <id> cannot combine cmos-drive-types and cmos-qemu-layout`.
READ: QEMU `hw/i386/pc.c` (29760ef), `pc_cmos_init_late`, `cmos_init_hd`,
`set_boot_dev`, `pc_machine_initfn` (`fd_bootchk=true`) and
`pc_machine_class_init` (default order `cad`);
`hw/rtc/mc146818rtc.c`, `rtc_set_cmos`. SeaBIOS `src/fw/paravirt.c`,
`qemu_preinit`, `qemu_cfg_e820`, `qemu_get_present_cpus_count`;
`src/boot.c`, `boot_init`; `src/block.c`, `get_translation`;
`src/clock.c`, `clock_setup`; `src/hw/floppy.c`, `floppy_setup`.

`derive_cmos_layout` returns all 128 bytes without allocating guest RAM, so
large-memory arithmetic can be tested on synthetic maps. Assembly loads the
whole seed for `CmosRam::Bytes128` and only its low 64 bytes for `Bytes64`.
The existing `derive_cmos_config` API, 64-byte RTC snapshots, NVRAM files and
goldens retain their current format. Firmware validation implicitly requires
`rtc-cmos-128` whenever `cmos-qemu-layout` is enabled, even if the entry omits
it from `requires`. A configured 128-byte RTC satisfies that requirement on
Bare rigs; the future board will select it before enabling the SeaBIOS entry.
The MC146818 still masks addresses with 3Fh: 5Bh–5Dh alias 1Bh–1Dh, and
5Fh aliases 1Fh. A type-47 geometry record there would make SeaBIOS report
phantom RAM above 4 GiB and up to 256 CPUs. Pure derivation remains available
for synthetic tests without device validation.

Two deliberate divergences preserve the AT seed. QEMU 29760ef writes no
2Eh/2Fh checksum; we retain the AT sum over 10h–2Dh. QEMU
`pc_cmos_init_late` overwrites 14h with 06h (FPU and PS/2 mouse), whereas we
retain the AT equipment byte derived from the modelled machine. SeaBIOS reads
neither that equipment byte nor the checksum.

**Explicit decode-conflict policy.** Port/MMIO decode is **first-match order**,
and that order is *semantics*, not luck. The canonical example: `0x80` is the
POST diagnostic latch while `0x81` is a DMA page register — adjacent ports owned
by different devices, where a sloppy range would swallow the wrong one. The
assembler therefore:

1. Sorts port entries by `(start, end)` and MMIO entries by `(base, end)`
   **deterministically**.
2. Rejects every unflagged port overlap with `PortOverlap`, and every MMIO overlap.
   A port overlap flagged by `PortWindow.allow_overlap` or
   `DeviceConfig.allow_port_overlap` is accepted only when one window is strictly
   contained in the other and both start at the same port. Sorting then places
   the narrower window first, giving it every shared port. Identical windows,
   partial overlaps and containment with different starts fail with
   `PortOverlapDecodeOrder`, which names both devices, both windows and the first
   shared port and asks the caller to split the wider window or move one.

Deterministic sorting and validation make the decode map a pure function of the
config, and an accidental collision fails loudly at assembly time instead of
misrouting a port at run time.

`FdcCfg::port_windows(primary_ata_control)` supplies the FDC's AT decode. Existing
presets pass `false`, preserving `0x3F0–0x3F7`. With a primary ATA control register
at `0x3F6`, callers pass `true`: the FDC gets `0x3F0–0x3F5` and `0x3F7`, with no
overlap flags. The IBM Personal Computer AT Technical Reference, System Board,
**I/O Address Map** lists `0x3F0–0x3F7` for the diskette controller and
`0x1F0–0x1F8` for fixed disk. Its **Fixed Disk and Diskette Drive Adapter** section
identifies the fixed-disk register at `0x3F6` and the diskette registers at
`0x3F2`, `0x3F4`, `0x3F5` and `0x3F7`. The split decode is this model's choice:
it keeps unused `0x3F0`, `0x3F1` and `0x3F3` with the FDC, as the unsplit window
does. This model keeps `0x3F7` entirely with the FDC
(digital input register on read, configuration control register on write).
The real adapter shares read bits there with the fixed disk; that bit-level
sharing is not modelled.

**Validation before assembly.** A `ConfigError` pass runs *before* any device is
built: an absent or incompatible drive controller, an unavailable unit, an IRQ line with no controller, a `LoadImage`
outside the memory map, a decode overlap. Assembly of a validated config does not
fail for configuration reasons.

### Observation and presentation policy

`ObservationPolicy.output: OutputConfig { frame_sink: FrameSinkKind }` selects
initial presentation wiring for every requested text route. It is run-scoped
observation setup, outside the hardware-only `MachineConfig` and machine-file
lowering. The default `Headless` installs no subscriber and keeps capture and
prompt detection available.
`Capture` installs one `video::CaptureFrameSink` per route; the facade retains
only its last frame, available through `Observation::captured_frame(DeviceId)`.
Selecting Capture without any requested display does not add observation routes.
`Observation::bind` applies this selection for assembly and inline hosts alike.
Hosts can later call public `Observation::configure_output(OutputConfig)`;
reconfiguration clears configured last frames and preserves external subscribers.

A host presenter attaches with `Observation::subscribe(DeviceId, Box<dyn FrameSink>)`.
The route must already be bound. Subscribers receive the exact owned frame
returned by capture, at existing prompt polls and final captures. They receive
no machine reference and must not block, introduce host timing or call back into
the machine. Assembly depends on the video trait, with no window dependency.
Observation policy owns presentation selection, routes and byte limits;
RunPolicy owns poll cadence. Presentation never changes execution step bounds. See
[observation.md §Sinks, bounds and lifecycle](observation.md#sinks-bounds-and-lifecycle).

## Presets

Presets are the ergonomic layer: **plain functions** that return a
`MachineConfig`.

```
fn at_class(params: AtParams) -> MachineConfig { … }
fn serial() -> MachineConfig { … }
```

They are ordinary functions, not a trait and not a builder DSL — a preset just
fills in a `MachineConfig`. Today's `AtChipset` and `SerialChipset` become
presets: the knowledge they encode (which devices, at which ports, with which
wiring) moves into the `DeviceConfig` list a preset produces. A caller wanting a
one-off variation clones a preset's config and edits fields. Added in task-31.

### Canonical BIOS device order

Device order is part of the machine: the assembler preserves `devices` order,
and the bus serves due devices in bus order at each whole PIT boundary. Devices
due at the same boundary therefore run in that order. Decode-map sorting does
not change this scheduling order.

The BIOS preset follows the board model's fixed motherboard order first:
`MasterPic`, `SlavePic`, `Pit`, `Dma`, `DmaWord`, `Keyboard`, `Rtc`,
`Coprocessor`, `SysControl`, `Post`.
Cards follow in file order; the BIOS preset's card order is `Mda`, `Vga`, `Uart`,
`Fdc`, matching [machine-files.md §Canonical order](machine-files.md#canonical-order). The no-floppy
BIOS rig omits both DMA banks and the empty coprocessor socket, and keeps its undecoded, unwired FDC. Other Rust test-rig
presets retain their existing order. The BIOS MMIO windows are sorted by base:
`Mda` at `0xB0000`, then `Vga` at `0xB8000`.

### Display cards

Existing presets retain `DeviceKind::Vga(VgaTextCfg {})`, the text-only
`vga-text` model. The register-level alternative is
`DeviceKind::VgaRegisters(VgaCfg {})`, lowered from `kind = "vga"` in TOML/YAML.
Both use `DeviceId::Vga` and support the `TextScanout` observation route.
For `vga`, that display route follows AC graphics selection and delivers
`Frame::Text` or owned indexed `Frame::Pixels` (CGA-compatible 06h,
planar 0Dh/0Eh/10h/12h and chain-4 13h); the card needs no separate graphics configuration.
`VgaCfg::port_windows()` declares byte ports 3B4–3B5, 3BA, 3C0–3C2,
3C4–3CA, 3CC, 3CE–3CF, 3D4–3D5 and 3DA. Its MMIO window owns
A0000h–BFFFFh; RAM must exclude that aperture, including unselected VGA maps.
The monochrome printer ports remain unclaimed. A separate MDA conflicts with
this VGA's monochrome registers and memory and fails ordinary decode validation.

Validation rejects a configuration containing both VGA models before duplicate
role or decode checks with the exact message:
`vga and vga-text cards are mutually exclusive (shared ports and MMIO)`.

The new card starts unprogrammed, with cleared registers, fonts and DAC.
There is no option ROM yet: a BIOS that does not program VGA registers and
load a font sees an unprogrammed display. SeaVGABIOS delivery through `fw_cfg`
belongs to the forthcoming 386 board. Existing presets and golden identities
continue to use the text-only card. See
[observation.md §Register-level VGA text scanout](observation.md#register-level-vga-text-scanout)
for the font/palette frame extension and model limits, and
[graphics scanout](observation.md#register-level-vga-graphics-scanout) for
logical geometry, palette ownership and unsupported graphics operations.

### BIOS monochrome display routing

The `bios` preset, shared by BIOS and FreeDOS runners, includes independent VGA
and MDA text cards. `DeviceKind::Mda(MdaTextCfg {})` constructs `MdaText` at role
`DeviceId::Mda`. Its decoded MMIO aperture is `0xB0000..=0xB7FFF`, removed from
RAM; the card mirrors 4 KiB of physical text RAM across that aperture. Its ports
are `0x3B0..=0x3BB` and `0x3BF`, excluding the unmodelled printer function at
`0x3BC..=0x3BE`. VGA retains its colour ports and `0xB8000..=0xBFFFF` aperture;
monochrome CRTC writes no longer update VGA registers. Other presets retain
their existing device and memory maps.

These ranges follow the IBM PC Technical Reference, Options and Adapters,
Monochrome Display and Printer Adapter, Programming Considerations. The device
module documents the status/open-bus assumptions. Validation applies the same
missing-device, port/MMIO conflict and RAM-exclusion checks as for VGA.

Prompt polling's `ConsoleProbe.device` names a bound display by `DeviceId`,
without a guest-memory address or geometry override. Polling and final capture
both call `Observation::capture_text`, which renders `Device::text_scanout` once
through `video::render_text_mode`, including the CRTC start address and cursor,
then delivers that frame to the route's subscribers. Poll cadence and keyboard
injection stay fixed.

## RunPolicy — "how we run" is not "what the machine is"

`MachineConfig` describes the machine. It does **not** describe the run. Those
are split (implemented in task-32, in the `assembly` crate next to the assembler):

```
RunPolicy {
    retired_cap: Option<u64>,          // safety bound on virtual time (retired + halted clocks)
    idle_limit:  Option<u64>,          // halted PIT clocks with no wake-up before stopping
    stop:        StopCondition,        // Natural | PromptThen { console, needle, poll_step, then_budget }
    stimuli:     Vec<KeyboardStimulus>,// scancodes + WHEN to inject (InjectionSchedule)
}
```

The stop *shape* is an enum, not a `bool`: `StopCondition::Natural` runs to a
halt/quiescence (the POST boots and demos); `StopCondition::PromptThen` folds the
old `boot_to_date_prompt` loop — poll the `console` in `poll_step` retired-instruction
chunks until `needle` shows, then, if stimuli are scheduled, inject them and run
`then_budget` more. Scancode injection is a `KeyboardStimulus` (bytes + an
`InjectionSchedule`, today `AfterPrompt`); it is the **mechanism** the interactive
FreeDOS port (task-36) drives — task-32 provides it, task-36 is the caller.

`idle_limit` is a run-policy concern: assembly starts without an idle wait and
`run(machine, observation, policy)` applies it before executing. The active cap counts steps,
including REP iterations, plus accumulated halted instruction equivalents. The
cap is exact during execution; halted caps round up to a whole PIT clock.

**The injection cadence is part of the golden identity.** *When* scancodes are
injected and how often the loop polls for a pending IRQ is not a free tuning
knob: it changes which interrupts are taken and therefore the observable output.
`RunPolicy` derives `Eq`/`Debug` for exactly this reason. A golden trace is valid
only against a fixed `RunPolicy` cadence, exactly as it is valid only against a
pinned CPU revision. Introduced in task-32.

## Migration strategy

The old hand-written assembly paths and the new config path **coexist** until the
final deletion. Nothing is deleted until the config path is proven to reproduce
the old path byte-for-byte.

**A/B equality per ported runner.** Every runner ported to the config path is
guarded by an **A/B test that asserts full-result equality** — the *entire*
return value, compared by the runner's own return type (`BiosRun`,
`Test386Run`, `Vec<u8>`). `BiosRun` has no `PartialEq` today, so the comparison
goes through a **shared A/B harness** (task-35) that compares every observable
field (POST bytes, VGA render, serial stream, stop reason). "Equal" means the
new path is indistinguishable from the old, not merely "looks similar."

**Porting order — safest first, scariest last.**

1. `run_test386` and `run_floppy_boot` first — narrowest surface.
2. The PC-family boot runners next.
3. `run_freedos_interactive` **last** — it has the finest interrupt cadence and
   the most fragile keyboard-injection timing, so it is ported only once the
   harness and the easier runners have shaken the config path out (task-32 /
   task-36).

**The deletion gate (task-33).** Removing the `Chipset` trait and the ad-hoc
assembly is a **deletion-only commit**. It is gated on **all A/B tests being
green on the pre-deletion commit** — i.e. the commit *before* the deletion must
have every runner passing A/B equality. The A/B tests are then deleted *together
with* the old paths they compared against (they have no meaning once one side is
gone). task-33 lands the decision-7 annex verification and removes the trait; it
does not itself port anything.

## The demos and `run_serial`

Resolved here so the trait removal (task-33) is unblocked:

- **The four Real16 demos** — `run_boot`, `run_keyboard`, `run_rtc`,
  `run_vga_text` — are **ported to the config path in task-31**. They are small
  and the AT-class config already covers everything they need; there is no reason
  to keep them as hand-written assembly.
- **`run_serial` and its Long64 golden are intentionally removed.** The Long64
  walking skeleton has served its purpose. `HELLO_PROGRAM`, flat Long64 blob
  loading and the default CLI demo are retired with the legacy machine loop.
  `--boot` remains the Real16 timer demo; no arguments print usage and point at
  DOS goldens. The CPU adapter retains its own Long64 unit tests. Machine config
  has no CPU-mode axis and this removal is not a regression to restore.

## Non-goals

Deliberately out of scope. Do not implement these unprompted.

- **No MHz / frequency axis.** Virtual time is retired instructions
  (decision-5); a nominal "MHz" measures nothing in this model. `ClockConfig`
  carries a *ratio*, not a frequency. A frequency axis arrives only if a consumer
  needs it *and* there is an oracle to validate it against.
- **No behavioural CPU-type toggle.** There is no oracle for an 8086/8088
  behavioural mode, and the CPU core models one part. `CpuConfig.chip` is a
  marker with one variant today; a second *behavioural* variant waits for a
  consumer and a differential oracle.
- **No CPU-mode axis in the config.** `run_serial`'s Long64 case was intentionally
  deleted, without adding a mode dimension to `MachineConfig` (see
  above).
- **Any deleted demo.** `run_serial` (and its golden) was deleted;
  that removal is intentional and recorded — not a regression to
  restore.
- **No protected mode / 386, no cycle-accurate timing.** Unchanged from the
  project's standing scope (§1.3, [`deferred.md`](../deferred.md)).
- **Snapshot / save-restore is deferred, not designed here.** The config is made
  identifiable (`Eq`/`Debug`) so a future snapshot feature has a stable machine
  identity to hang off — but building it is deferred
  ([`deferred.md`](../deferred.md)).

## Milestone map

The layer is built across task-28 … task-37. Dependencies mirror the backlog.

| Task | What | Depends on |
|------|------|------------|
| **task-28** | This design doc + decision-7 annex (the Chipset-trait replacement) + deferred.md snapshot entry. Docs only. | — |
| **task-29** | Config data model + `ConfigError` validation, in a new assembly crate. | task-28 |
| **task-30** | The assembler: `MachineConfig -> Machine` (+ `DeviceId -> index` map), in the assembly crate. | task-29 |
| **task-31** | Presets + port the PC-family `run_*` (and the four Real16 demos) to config, under A/B equality. | task-30 |
| **task-32** | `RunPolicy` split + port `run_freedos_interactive` (highest risk). | task-31 |
| **task-33** | Remove the `Chipset` trait, delete the ad-hoc assembly, verify the decision-7 annex against final code. Deletion-only, gated on all A/B green. | task-36 |
| **task-34** | `DriveConfig` + floppy geometries — the seam under future IDE/HDD. | task-30 |
| **task-35** | Shared A/B result-equality test harness (gives `BiosRun` a comparison). | task-30 |
| **task-36** | Port `run_freedos_interactive` to config + `RunPolicy` (the scary one). | task-32 |
| **task-37** | `DriveConfig` wiring + CMOS derived from attached drives. | task-33 |
