# Firmware, board models and machine classes

Spike for bead `upc-6le.2` (epic `upc-6le`, phase A′ of decision-16), written
2026-11-10 at `main` 99a669e. **Design proposal; implementation notes in §9.** It answers:
which machines a description can select, which open firmware runs each, and
which devices each needs.

Related: [`machine-config.md`](machine-config.md) (the `MachineConfig` this
layer lowers to), decision-12 (Kiselev BIOS choice), decision-8 (A20),
decision-16 and its 2026-11-10 amendments (phases A′ to D), the description-file
spike `upc-6le.1` and the persistent-media spike `upc-6le.3`.

Evidence labels:

- **MEASURED**: observed in this repository, or computed on a fetched file.
- **READ**: read in source code or a document; the source is cited.
- **ASSUMED**: not checked in this spike. Check it before relying on it.

## 1. Context

Today there is one firmware machine (READ `crates/assembly/src/presets.rs`,
`bios()`). It is an 80286 Real16 CPU with a 1 MiB memory mosaic and the
following parts:

- a cascaded 8259 pair, an 8253, an 8042, an MC146818 with seeded CMOS, port
  61h, a POST latch at 80h and a COM1 16550;
- MDA and VGA text cards;
- one 8237 with page latches 81h–8Fh, and a µPD765 at 3F0h–3F7h on IRQ6 and
  DMA channel 2;
- Sergey Kiselev's `8088_bios`, built for the Xi 8088 board, as a 32 KiB ROM at
  F8000h.

FreeDOS boots from floppy and runs `DIR` (MEASURED: status.md, slow goldens).
Firmware is hard-wired into the preset. The BIOS has no fixed-disk INT 13h, and
FreeDOS reports no hard disks.

The maintainer agreed a first shape in the `upc-6le.1` notes:

- `[firmware]` with `provides` and `option_roms`;
- an `ide` card that is rejected unless something provides `fixed-disk`;
- `class = "at" | "xt"`;
- `cpu.model = "8088" | "8086" | "v20"` reserved until x86jit models them
  (`xj-50w (x86jit)`, P4).

**A later scope change replaces `class` with a named board model.** The
description says `[board] model = "ibm-5170"`, the way 86Box and PCem name
machines, and the class is derived from the model. Real BIOSes are built per
board or chipset. They program chipset registers and expect chipset-integrated
peripherals. So firmware compatibility is a property of the board, not of an
abstract class.

The forces on the design:

- **Goldens.** Every headless golden pins a firmware image (`fetch.sh` checks
  SHA-256), a CPU revision and a run cadence. Changing the default BIOS changes
  every golden.
- **Phase B** needs A20 control and INT 15h AT services (87h, 88h, 89h). The
  Kiselev BIOS has neither (READ below), so a second firmware is needed anyway.
- **Hard disk.** A hard disk in phase A′ needs a fixed-disk INT 13h provider.
- **XT.** An XT-class machine needs an 8088 CPU model that x86jit does not have
  yet.

## 2. Firmware survey table

"AT services" means INT 15h 87h (block move), 88h (extended memory size), 89h
(enter protected mode), A20 control, and use of AT CMOS.

| Firmware (version, date) | License | CPU min | Board / class | AT services | Fixed disk | Video | Boots FreeDOS | Build | Source |
|---|---|---|---|---|---|---|---|---|---|
| Kiselev `8088_bios`, Xi 8088 config (1.0.2, BIOS date 03/27/26; commit `a4af458`, 2026-07-16) | GPL-3.0-or-later | 8088; runs on our 286 | Xi 8088: AT-like, with 2×8259, 8042, MC146818, **one** 8237 | **None.** INT 15h has 4Fh, 90h/91h, C0h and C2h only; no A20 code; reads AT CMOS and checksums it | None. INT 13h serves DL ≤ 1; IPL falls back to INT 13h DL=80h | MDA/CGA INT 10h built in; EGA/VGA through a ROM at C000h | **Yes** (MEASURED, goldens) | NASM (open) | [skiselev/8088_bios](https://github.com/skiselev/8088_bios) |
| Kiselev `8088_bios`, XT configs (`MACHINE_XT` 16 KiB, Book8088, FE2010A) | GPL-3.0-or-later | 8088 / V20 | XT: 8255 PPI, one 8259, DIP switches | None | None | MDA/CGA, chosen by switches | Not verified | NASM | same; `config.inc` says `MACHINE_XT` is "FIXME: not implemented yet" and the README says "not tested" (READ) |
| GLaBIOS (`VER_NUM 0.4.3` at commit `68c65a5`, 2026-07-23; the README also lists a newer beta) | GPL-3.0-or-later | 8088 (V20 builds need a V20) | XT presets: 5150, 5160, Turbo, several chipsets, `ARCH_EMU` | None. INT 15h is cassette-only on 5150, else AH=86h/CF. No RTC; RTC comes from the GLaTICK option ROM | None. INT 13h is floppy only (READ) | MDA/CGA routines; EGA/VGA through its ROM | Upstream lists no OS tests (ASSUMED yes) | MASM 5 + LINK (proprietary); releases ship ROMs | [640-KB/GLaBIOS](https://github.com/640-KB/GLaBIOS) |
| Super PC/Turbo XT BIOS 3.1 (2017-10-28) | Public domain (bundled XUB is GPL-2, floppy BIOS GPL-3) | 8088 / V20 | XT / 5150 | None. RTC is detected but not served | Needs a controller ROM (bundles XUB) | Tests MDA/Hercules/CGA memory; colour boot screen | Not stated | Open Watcom | [readme](https://www.retrocmp.de/software/pcxtbios/readme.txt) |
| 8086tiny BIOS | ASSUMED MIT | 8086 | Emulator-specific | None | Emulator traps | — | — | — | Rejected in decision-12: it needs emulator-only opcodes |
| **XTIDE Universal BIOS (XUB)** r638 (builds dated 2026-06-09) | GPL-2.0-or-later (source headers; READ) | `ide_xt` 8088, `ide_xtp` 186/V20, `ide_at`/`ide_atl` **286**, `ide_386` 386 | Option ROM, any board | — | **Provides fixed-disk INT 13h** (see §2.2) | Prints through INT 10h | ASSUMED yes | NASM + make (open) | [xtideuniversalbios.org](https://www.xtideuniversalbios.org/), [binaries r638](https://www.xtideuniversalbios.org/binaries/r638/) |
| Bochs `rombios` (`BIOS-bochs-legacy`) (master, © 2001–2026) | LGPL-2.1-or-later | **386** (`#error Only 386+ cpu supported`) | AT / Bochs PCI (i430FX, i440FX, i440BX host IDs) | **Yes**: INT 15h 24h, 87h, 88h, 89h; A20 through **port 92h** | Built-in ATA/ATAPI driver, El Torito | **None.** Needs a VGA ROM at C000h; PCI-BIOS builds halt without one | ASSUMED yes | bcc/as86 (dev86) | [bochs/bios/rombios.c](https://github.com/bochs-emu/Bochs/blob/master/bochs/bios/rombios.c) |
| SeaBIOS 1.17.0 (2025-06-10), QEMU `isapc` config (128 KiB, `PCIBIOS=n`, `USB=n`, `ACPI=n`) | LGPL-3.0-or-later | **386** (32-bit flat POST) | ISA-only 486-class PC (QEMU `isapc`) | Yes (ASSUMED in detail); RAM size from CMOS when there is no fw_cfg (READ) | Built-in ATA PIO, floppy | None. SeaVGABIOS ROM, or SERCON (VGA output redirected to serial) | ASSUMED yes | gcc + binutils | [seabios.org](https://www.seabios.org/), QEMU `roms/config.seabios-128k` |
| SeaBIOS 1.17.0, QEMU `pc` config | LGPL-3.0-or-later | 386 | i440FX + PIIX3/PIIX4, Q35 | Yes, plus E820, PCI BIOS, ACPI | ATA, AHCI, USB, … | SeaVGABIOS: std VGA, Cirrus, Bochs VBE | ASSUMED yes | gcc | as above |
| SeaVGABIOS / LGPL VGABIOS (video option ROMs) | LGPL-3.0 / LGPL-2.1 | ASSUMED 386 | Video option ROM | — | — | VGA; VBE with Bochs VBE | — | gcc / bcc | SeaBIOS `vgasrc/`, Bochs |
| fysnet i440fx BIOS 1.0 (2025-02-10) | **Freeware, not open source** | — | i440FX / 440BX | — | — | — | — | NBASM | [fysnet.github.io/i440fx](https://fysnet.github.io/i440fx); excluded on license grounds |
| IBM 5150/5160/5170 ROMs; AMI/Award/Phoenix board BIOSes | Proprietary | per board | per board | per board | per board | per board | per board | — | **User-supplied only.** Hashes are listed, the images are never shipped |

**No open BIOS was found that targets a real 286 AT and provides INT 15h
87h/89h** (READ: searches turned up none; ASSUMED none exists). Both open
AT-service BIOSes, Bochs `rombios` and SeaBIOS, need a 386. Phase B therefore
moves to a 386 board for firmware reasons as well as CPU reasons.

### 2.1 Kiselev Xi 8088 configuration, in detail (READ: pinned clone, `src/`)

- **Settings.** `MACHINE_XI8088` sets `AT_RTC`, `AT_RTC_NVRAM`, `AT_NMI` (NMI
  mask at 70h), `AT_DELAY` (61h refresh bit), `SECOND_PIC`, `AT_KEYBOARD`,
  `PS2_MOUSE` and model byte FCh. `MAX_RAM_SIZE` is 640 and `EBDA_SIZE` 1
  (`config.inc`).
- **Option-ROM scan.** It scans for a video BIOS from C000h to C800h, then for
  extensions from C800h to F800h (to F000h when a setup flag is set). The step is
  2 KiB. A ROM must have a 55AA header and a size byte, and its checksum must be
  zero. The scan skips zero-sized headers (`bios.asm` `extension_scan`,
  `detect_rom_ext`).
- **IPL.** INT 19h tries floppy 0 four times, then `INT 13h AH=0Dh/02h DL=80h`
  (`bios.asm` `ipl`). An option ROM that hooks INT 13h (or INT 19h) can
  therefore supply the hard disk with no BIOS change.
- **INT 15h.** It handles only 4Fh, 9001h, 90FDh, 9101h, C0h and C2h. Everything
  else returns AH=86h with CF set (`misc.inc`). There is no 87h, 88h, 89h or
  24xxh.
- **CPU detection.** It is written for 8088, 80C88 and V20 (`cpu.inc` header).
  On our 286 it prints an 8088 variant (ASSUMED; cosmetic).

### 2.2 XUB, in detail (READ: SVN trunk sources; MEASURED: r638 `ide_at.bin`)

- **Installation.** The ROM-scan entry `Initialize_FromMainBiosRomSearch`
  installs an **INT 19h boot loader** and returns. Drives are detected when INT
  19h runs (late initialization). `Interrupts.asm` then saves the old INT 13h in
  `RAMVARS.fpOldI13h` and hooks INT 13h, chaining requests it does not own
  (floppies). If no handler is present, it copies the old INT 13h to INT 40h.
- **Defaults of the AT build** (`Main.asm`, `USE_AT`):
  - full mode, which steals **1 KiB** of conventional memory;
  - two controllers: primary 16-bit ATA at **1F0h** (control block at 3F6h)
    and secondary at 170h;
  - IRQ **14/15** when `MODULE_IRQ` is built in.

  So the stock AT build matches a standard AT IDE channel with no configuration
  step.
- **Transfers.** `IdePioBlock.asm` uses **`rep insw`/`rep outsw`** when
  `USE_186`, which every AT build sets. The 8088 build uses unrolled `in ax,dx`
  loops instead.
- **MEASURED: the r638 `ide_at.bin` is 6,646 bytes.** It has the 55AA header,
  a size byte of 16 (8 KiB) and a non-zero checksum (byte sum B0h). **It is not
  a valid option ROM as downloaded.** The upstream flow pads it to 8 KiB and
  fixes the checksum through `XTIDECFG.COM`. A fixture script must do the same
  padding deterministically and pin both hashes. Raw hash:
  `6d5007bca3be0bffeb210969a299b67199368b4845f554b5a00715e2718dfea8`.

### 2.3 Bochs `rombios` and SeaBIOS: requirements (READ)

- **Bochs `rombios`.**
  - `rombios.h` sets `BX_CPU 3`, `BX_PCIBIOS 1`, `BX_APM 1`, `BX_USE_ATADRV 1`
    and `BX_ELTORITO_BOOT 1`. `LEGACY` only removes `rombios32`.
  - `PORT_A20` is `0x0092`.
  - It has no INT 10h of its own (`int10_handler: iret`). It scans C000h for a
    VGA ROM, and in PCI-BIOS builds it halts if there is no 55AA there.
  - It scans option ROMs from C800h to E000h.
- **SeaBIOS.**
  - POST runs in 32-bit flat mode (`docs/Memory_Model.md`).
  - Shadowing unlocks the F-segment through **i440FX (82441) or Q35 PAM**
    registers (`src/fw/shadow.c`). Without a bridge it logs "Unable to unlock
    ram". QEMU worked around this for `isapc` with a writable BIOS region, and
    the board model must do the same (READ, a 2013 QEMU patch summarized by
    search).
  - Its PCI init knows PIIX3 (82371SB) and PIIX4 (82371AB) ISA bridges, PIIX
    IDE, PIIX4 PM and ICH9 (`src/fw/pciinit.c`).
  - QEMU detection reads PCI 00:00.0 and the fw_cfg signature at 510h. Both read
    open bus on a plain ISA board, so it falls back to CMOS memory size
    (`src/fw/paravirt.c`).

## 3. Board models

### 3.1 What a board model is

A board model is a named, versioned piece of data owned by the `assembly` crate.
The description file selects it with `[board] model = "…"`. It lowers to the
existing flat `MachineConfig`, so the assembler, validator and goldens keep their
contract (machine-config.md). A board model has these parts:

1. **Identity.** An `id`, a display name, the derived **class** (`xt`, `at`,
   `at386`, `pci`), and the reference document (the Technical Reference or
   chipset datasheet, by name and section).
2. **CPU sockets.** The allowed `cpu.model` values and an optional FPU socket. A
   value that is allowed but not yet modelled stays **reserved**.
3. **Motherboard device set and standard wiring.** A `Vec<DeviceConfig>` with
   ports, IRQs, DMA and access couplings, plus the memory map: ROM window, RAM
   holes and shadow regions. This includes chipset-integrated peripherals.
   - For example, the C&T 82C206 integrated peripheral controller of the NEAT
     chipset combines the 8259s, 8237s, 8254 and RTC (ASSUMED part mapping).
   - Such a chip reuses the existing device kinds at their standard ports. It
     is a composition, not a new device.
4. **Configuration store.** Either CMOS NVRAM in a layout derived from the
   description (AT), or DIP switches derived from the description (XT). Neither
   is free-configured, as with CMOS today.
5. **Chipset register model (optional).** Registers that real firmware programs:
   - shadow RAM and PAM;
   - cache control;
   - turbo;
   - fast A20 at port 92h;
   - PCI configuration space;
   - PIRQ routing;
   - ELCR.

   `ibm-5150`, `ibm-5160` and `ibm-5170` have none.
6. **Card bus.** The slot width (ISA-8, ISA-16, PCI) and the IRQ and DMA lines
   left free for cards.
7. **Firmware catalogue.** A list of `FirmwareEntry` values:

   | Field | Meaning |
   |---|---|
   | `id` | Stable name, for example `kiselev-xi8088-1.0.2` |
   | `kind` | `system-bios`, `option-rom` or `video-bios` |
   | `sha256` | Image identity, checked when the bytes are read |
   | `size`, `base` | Load rule: a system BIOS is top-aligned under 1 MiB, an option ROM has a suggested base |
   | `provides` | Services the image supplies to the guest (`floppy`, `fixed-disk`, `video`, `at-services`, `pci-bios`, `vbe`) |
   | `requires` | Parts the image needs (`kbc-8042`, `ppi-8255`, `rtc-cmos`, `pic-cascade`, `video-rom`, `port-92h`, `ata@1F0/14`) |
   | `cpu_min` | Minimum CPU model |
   | `distribution` | `open` (a fetch script under `vendor/`, with license and provenance) or `user-supplied` (hash only, never shipped) |
   | `status` | `tested` (a golden runs it) or `listed` |

8. **Default firmware.** The open, tested entry, when there is one.

### 3.2 Board catalogue (proposed)

| `model` | Class | CPU sockets | Chipset registers | Open firmware | User-supplied examples | Status |
|---|---|---|---|---|---|---|
| `ibm-5170` | at | 80286 (80287 later) | none | **Kiselev Xi 8088 1.0.2** (tested; SHA-256 `1ad458ac…81a8`, MEASURED by `fetch.sh`), plus XUB AT r638 option ROM | IBM 5170 BIOS (even/odd pair) | **available now** |
| `ibm-5160` | xt | 8088, v20 (reserved) | none | GLaBIOS `ARCH_5160`, Kiselev `MACHINE_XT`, Super PC/Turbo XT | IBM 5160 BIOS | reserved until `xj-50w.2 (x86jit)` |
| `ibm-5150` | xt | 8088 (reserved) | none | GLaBIOS `ARCH_5150` | IBM 5150 BIOS | optional, later |
| `generic-386-isa` | at386 | 80386 (reserved) | port 92h only | SeaBIOS (`isapc` config) + SeaVGABIOS; Bochs `rombios` legacy | — | reserved until phase B |
| `i440fx-piix3` | pci | Pentium Pro / Pentium II (reserved) | 82441FX PAM/SMRAM, PIIX3 PIRQ/ELCR | SeaBIOS (QEMU `pc` config); Bochs `rombios` | — | candidate for phases C/D |
| `i440bx-piix4` | pci + AGP | Pentium II/III | 82443BX, PIIX4 | none for a real board (see §3.4) | Award/AMI/Phoenix images of named boards | enthusiast |
| `neat-286` | at | 286 / 386SX | C&T CS8221 index/data registers (ASSUMED 22h/23h) | none | AMI/Phoenix NEAT BIOSes | enthusiast |
| `i430tx` | pci | Pentium MMX | 82439TX, PIIX4 | none | Award/AMI | enthusiast |

Today's machine is `ibm-5170` with listed deviations (§4.2). Its devices are a
subset of the 5170: there is no second 8237, no 80287 ports, no reset line and
no A20 masking. The name is the target, and each missing part is its own bead.

### 3.3 Do the Kiselev BIOS and GLaBIOS use only standard parts? (READ)

The **Kiselev Xi 8088 build on `ibm-5170`** mostly uses 5170 parts:

- 2×8259;
- an 8042 (with PS/2 aux probing);
- the MC146818 with the NMI mask at 70h;
- the 61h refresh bit;
- an 8237;
- POST codes at 80h.

It deviates from a 5170 in these places:

- **Port 61h bit 2 is the Xi 8088 "turbo" bit** (`set_cpu_clk_turbo`). On a
  5170 that bit enables the parity check. Writes are harmless.
- **Port C0h is written as an "unused" register** for I/O delays and bus
  floating in serial and printer detection (`unused_reg equ 0C0h`). On a 5170,
  C0h is the second 8237's channel-4 address register. Those writes are
  harmless (channel 4 is the cascade), but they will change device state once
  DMA2 is modelled.
- **Only one 8237 is initialized.** The 16-bit channels stay unprogrammed.
- **No extended memory.** The RAM scan stops at 640 KiB and INT 15h 88h is
  absent.
- **PS/2 aux.** It probes the aux port, which a 5170 8042 lacks. Our 8042
  answers like a PS/2 controller; the BIOS tolerates either.

The **Kiselev `MACHINE_XT` build on `ibm-5160`** uses standard 5160 parts:

- 8255 control word 99h;
- DIP switches through PB3 select and port C;
- NMI mask at A0h.

Its POST code at 80h lands on the XT DMA page-register file (ASSUMED harmless).
It is untested upstream.

**GLaBIOS `ARCH_5160`** uses standard 5160 parts. It sets `TURBO_NONE`, takes
the drive count from IPL switch SW1 and supports the expansion unit. Its default
`ARCH_TURBO` preset toggles a non-standard 61h turbo bit, so the catalogue lists
only `ARCH_5160` (or `ARCH_EMU`) builds for `ibm-5160`. `ARCH_EMU` turns off
DRAM refresh and several POST tests for emulators.

### 3.4 i440FX + PIIX3 with SeaBIOS: candidate board for phases C/D

- **SeaBIOS chipsets (READ `pciinit.c`, `shadow.c`).** The supported host
  bridges are i440FX (82441, PAM0 at 59h) and Q35 (MCH). The ISA bridges are
  PIIX3 (82371SB) and PIIX4 (82371AB), plus ICH9 LPC. IDE setup covers PIIX3
  and PIIX4 function 1. Power management is PIIX4 function 3 or ICH9 SMBus.
  There is **no 440BX support**.
- **Devices the board needs** (ASSUMED register details; datasheets: Intel
  82441FX/82442FX 290549-001, May 1996; Intel 82371SB PIIX3):
  - **Host bridge.** 82441FX PMC with configuration mechanism #1 at CF8h/CFCh,
    PAM0–6 shadow control and SMRAM. The 82442FX DBX has no visible registers.
  - **PIIX3 function 0.** The PCI-to-ISA bridge, with PIRQ routing, the top of
    memory and ELCR. It also embeds the AT parts we already model: 2×8259,
    2×8237, 8254, port 61h, NMI and RTC decode.
  - **PIIX3 function 1.** IDE with legacy 1F0h/170h on IRQ14/15; bus-master IDE
    is optional.
  - **PIIX3 function 2.** UHCI USB, optional.
  - **PIIX4 function 3** (ACPI PM timer and SCI), only for ACPI guests. QEMU
    documents PIIX3 as the default south bridge and PIIX4 as an experimental
    alternative (READ, QEMU `i386/pc` docs).
  - **On ISA:** a super-I/O set (8042, FDC, UARTs, LPT).
  - **On PCI:** a VGA card, later with VBE through SeaVGABIOS's Bochs VBE
    variant, and the Voodoo.
- **Windows 98 SE.**
  - **Bochs** documents Windows 98 SE installing and running on its BIOS with
    `pci: chipset=i440fx` and a Cirrus VGA (READ,
    [Bochs guide](https://bochs.sourceforge.io/doc/docbook/user/guest-win98.html)).
  - **QEMU `pc`** with SeaBIOS is reported to work, but with caveats and version
    regressions (READ, secondary):
    - a community guide pins `pc-i440fx-2.0` and recommends patched installers;
    - a 2019 QEMU bug reports failures after 2.7/2.8;
    - a 2012 SeaBIOS list thread reports a Plug and Play BIOS installer issue.

  This is good evidence that the chipset and firmware pair runs Windows 98 SE.
  The rough edges are in the PnP and ACPI tables, which we would control.
- **Period fit.** The 440FX was released in May 1996. It was the Pentium Pro
  chipset and the first to support the Pentium II, with a 66 MHz FSB and **no
  AGP** (READ, Wikipedia/AnandTech by search). A PCI Voodoo or Voodoo2 fits a
  PCI-only board (ASSUMED card bus facts). A Pentium II on 440FX matches
  decision-16's "Pentium II/III-class" target at its low end; Pentium III
  officially needs 440BX or later.
- **440BX + PIIX4, for comparison.**
  - It dates from April 1998, with a 100 MHz FSB and AGP. It is the
    period-perfect Pentium II/III board.
  - Every real 440BX board BIOS (Award, AMI, Phoenix) is proprietary and would
    be user-supplied.
  - Bochs `rombios` recognizes an i440BX host-bridge ID (READ
    `PCI_FIXED_HOST_BRIDGE3`), but only for Bochs's own 440BX emulation, not a
    real board.
  - SeaBIOS would need 440BX PAM support upstream.
  - Verdict: `i440fx-piix3` is the open critical path. `i440bx-piix4` is an
    enthusiast profile on user ROMs.

### 3.5 Enthusiast boards and user-supplied ROMs

86Box and PCem show the demand: NEAT 286/386SX boards (the Arche AMA-2010 is a
NEAT machine in 86Box; READ by search), 430TX and 440BX boards. These boards are
**extras, not the critical path**. Each one needs its chipset register model,
and its firmware is proprietary.

- **Reference.** The description gives an `image` path (relative to the file)
  and optionally `sha256` and `id`. **Identity is the hash, not the file
  name.**
- **Allowlist.** Each user-supplied entry in a board's catalogue carries only
  `sha256`, `size`, `base`, `provides`, `requires` and `cpu_min`. The repository
  holds hashes of known-good dumps, never the bytes.
- **Split ROMs.** AT-era boards often ship even/odd EPROM pairs (ASSUMED for the
  5170). The description gives `parts = [{ image, lane = "even" }, { image,
  lane = "odd" }]`. The catalogue pins each part's hash and the interleaved
  image's hash.
- **Unknown hash.** The image is rejected with `FirmwareNotListed` (§6), which
  lists the board's known entries and where each comes from. **Resolved by
  decision-18:** `--allow-unlisted-firmware` is a run-only developer escape
  hatch that warns with the full hash. It is never a machine-file field or part
  of golden identity, and it relaxes only allowlist membership.
- **CMOS.** Vendor BIOSes keep extra CMOS bytes and checksums beyond 2Dh. The
  derived standard bytes may trigger a "CMOS checksum" F1 prompt, so these
  boards need the NVRAM-file feature sketched in `upc-6le.1` (§8).

## 4. Classes and device sets

### 4.1 Standard wiring per class

Sources:

- *IBM Personal Computer XT Technical Reference* (1983; 5155/5160 edition
  6139821, 1986), Section 1 "System Board". It covers System Timers, System
  Interrupts, DMA, "8255A I/O Bit Map" (p. 1-27) and Appendix G "Switch
  Settings".
- *IBM Personal Computer AT Technical Reference* (1502494, March 1984; 6183355,
  March 1986), Section 1 "System Board". It covers System Interrupts, System
  Timers, Direct Memory Access, the I/O Address Map, "Keyboard Controller"
  (p. 1-42, 1986) and "Real-Time Clock/CMOS RAM Information" (p. 1-56, 1986).
- The fixed-disk interface is in the AT's *Fixed Disk and Diskette Drive
  Adapter* (Options and Adapters).

The section titles are READ from tables of contents. The register values below
are ASSUMED from these standard references and must be re-checked in each
device bead.

| Part | xt (`ibm-5160`) | at (`ibm-5170`) | at386 (`generic-386-isa`) | In the repo today |
|---|---|---|---|---|
| CPU | 8088 (+8087) | 80286 (+80287 at F0h–FFh, IRQ13) | 80386 (+387) | 80286 Real16 only |
| Interrupts | **one** 8259A, 20h–21h, IRQ0–7, single mode | 8259A pair, 20h/A0h, slave on IRQ2 | as AT | cascaded pair ✓ |
| Timer | 8253, 40h–43h. ch0 → IRQ0, ch1 → DRAM refresh (DMA ch0), ch2 → speaker through PPI PB0/PB1 | 8254. ch1 refresh shows in 61h bit 4, ch2 → speaker through 61h | as AT | 8253 ✓; refresh bit is a per-read toggle |
| DMA | **one** 8237A at 00h–0Fh, ch0 refresh. Page latches at 81h–83h (ch2, ch3, ch1) | **two** 8237A: 00h–1Fh (ch0–3) and C0h–DFh (ch4–7, ch4 cascade). Pages at 80h–8Fh | as AT | one 8237 with pages 81h–8Fh ✓; **DMA2 ✗** |
| Keyboard interface | **8255A PPI at 60h–63h**: PA scan code, PB control (PB7 clear, PB6 clock, PB3 switch select), PC switches and status | **8042** at 60h/64h; output port bit 1 = A20, bit 0 = reset | 8042 + port 92h | 8042 ✓ (A20 bit is state only, `FEh` pulse ✗); **8255 ✗** |
| Keyboard | **83-key**, scan set 1, serial into PPI port A, IRQ1 | 84-key (101 later), set 2 translated to set 1 | as AT | host → set 1 encoder with E0 codes ✓ (an XT has no E0 keys) |
| Config store | **DIP SW1** read through the PPI | MC146818 CMOS at 70h/71h, IRQ8; NMI mask is 70h bit 7 | as AT, plus extended memory bytes | CMOS ✓, derived |
| NMI mask | port **A0h** bit 7 | 70h bit 7 | as AT | AT form ✓ |
| A20 | none (20-bit bus) | 8042 output port | 8042 + **port 92h** bit 1 | always on (decision-8) |
| CPU reset | — | 8042 `FEh`, CMOS 0Fh shutdown code | + port 92h bit 0 | ✗ |
| ROM | F0000h–FFFFFh; an 8 KiB BIOS at FE000h | 64 KiB at F0000h (even/odd) | same, aliased at 4 GiB − size | `ImageKind::Rom` ✓ |
| Card bus | 8-bit, IRQ2–7, DMA1–3 | 16-bit, IRQ3–7, 9–12, 14, 15 | as AT | no slot abstraction (decision-16 §8) |
| Floppy | adapter with DOR 3F2h and MSR/data 3F4h/3F5h; ASSUMED no CCR at 3F7h | 3F0h–3F7h, IRQ6, DMA2 | as AT | µPD765 ✓ |
| Fixed disk | Xebec MFM adapter at 320h with its own ROM; open alternative is an 8-bit XT-IDE card + XUB `ide_xt` | WD1003-style task file at 1F0h–1F7h plus 3F6h, IRQ14. The IBM BIOS reads a CMOS drive type | as AT | ATA-2 PIO channel assembled with volatile/readonly media; firmware boot profile deferred |
| Video | MDA/CGA, chosen by SW1 5–6 | MDA/CGA through CMOS; EGA/VGA by ROM | VGA ROM + register-level VGA | MDA ✓; VGA text (CRTC + CGA registers) ✓ |

### 4.2 What is missing, per board

- **`ibm-5170`, to match its reference:**
  - a second 8237 at C0h–DFh with page registers 89h–8Fh;
  - 80287 ports F0h–FFh;
  - the 8042 `FEh` reset pulse and a CPU-reset seam (CMOS 0Fh shutdown);
  - an **ATA device**;
  - String port I/O is now available at x86jit `73ed5d6`: Real16 INS/OUTS,
    including REP, segment overrides on OUTS and per-element IRQ boundaries.
    The historical `96a49a9` probe did not lift opcodes 6C–6F.
- **`ibm-5160`:**
  - the 8255 PPI with DIP-switch derivation;
  - the XT keyboard serial interface (PB7/PB6 handshake, 83-key set 1, no E0
    codes);
  - a single-mode 8259 (ASSUMED: the PIC model must honour ICW1 SNGL);
  - the XT page-register map;
  - the NMI mask at A0h, which clashes with an AT slave PIC, so it belongs to
    the board, not a global decode;
  - an FDC without CCR;
  - an 8-bit card limit and IRQ ≤ 7;
  - the 8088 CPU model (`xj-50w.2 (x86jit)`).
- **`generic-386-isa`, phase B:**
  - all of the 5170 items;
  - port 92h;
  - A20 **masking** in the CPU core (decision-8 revisit);
  - extended RAM;
  - a VGA option ROM with a register-level VGA (sequencer, graphics controller,
    attribute controller, DAC), or SeaBIOS SERCON for headless runs;
  - a writable BIOS shadow for SeaBIOS.

## 5. Config expression

The description layer (`upc-6le.1`) lowers to `MachineConfig`. The new layer
is `[board]` and `[firmware]`; `class` is never written.

**Today's FreeDOS machine (lowers to exactly `presets::bios(rom, Some(floppy))`):**

```toml
schema = 1
name = "freedos-floppy"

[board]
model = "ibm-5170"            # class "at" is derived

[cpu]
model = "80286"
rate = "1/1"

[memory]
conventional_kib = 640

[firmware]
bios = { image = "../vendor/kiselev-bios/bios-xi8088.bin",
         sha256 = "1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8" }

[[card]]
kind = "mda"

[[card]]
kind = "fdc-765"
drives = [ { slot = 0, type = "1.44M", image = "../vendor/freedos/…", write = "volatile" } ]
```

**AT with a hard disk (new profile; old goldens untouched):**

```toml
[board]
model = "ibm-5170"

[firmware]
bios = { image = "…/bios-xi8088.bin", sha256 = "1ad458ac…81a8" }
option_roms = [
  { image = "…/xub-r638-at.rom", sha256 = "<padded hash>", base = 0xC8000 },
]

[[card]]
kind = "ide"                    # defaults: 0x1F0–0x1F7, 0x3F6, IRQ 14
drives = [ { channel = "master", image = "hd.img",
             geometry = { cylinders = 615, heads = 4, sectors = 17 }, write = "volatile" } ]
```

Rules:

- **Identity.** A firmware entry is identified by the SHA-256 of its bytes,
  after split parts are interleaved. `sha256` in the file is optional; when
  present it must match the bytes. `id` is an optional label; when present it
  must name the entry the hash resolves to.
- **`provides` comes from the catalogue.** A file may restate it, and it must
  then equal the catalogue's. It is mandatory only for an unlisted image, if the
  maintainer allows those (§8).
- **System BIOS placement.**
  - A ROM ending at 1 MiB identifies the BIOS before any declaration. If none
    does, the config's declared BIOS index is used; without a declaration, a
    sole ROM is the candidate. These candidates report `BiosPlacement` when
    mis-sized or misplaced. Only zero ROMs or several undeclared candidates
    report `MissingSystemBios`; the highest address is never a fallback.
  - `base = 1 MiB − size`, which puts the reset vector F000:FFF0 at offset
    `size − 16`. The size must be a multiple of 8 KiB.
  - On 386 and PCI boards the image is also aliased at `4 GiB − size`, because
    SeaBIOS assumes this (`shadow.c`: "the bios at 0xf0000 is also at
    0xffff0000").
  - Boards that need a writable shadow map it as RAM preloaded with the image,
    or emulate PAM.
- **Option ROMs.**
  - `base` is required in machine files; catalogue suggestions are informational.
    The size is
    the header's `size byte × 512`, and the image must be exactly that long.
  - Lowering maps each ROM as `ImageKind::Rom`. The IBM PC AT Technical
    Reference, Section 1, **System Memory Map**, labels C0000h–DFFFFh as
    I/O expansion ROM, E0000h–EFFFFh as reserved on the system board, and
    F0000h–FFFFFh as system board ROM. The model protects C0000h–F7FFFh
    from system RAM because Kiselev scans through F7FFFh; this wider protected
    range is not all AT adapter space. Card MMIO is legitimate here: assembly
    carves zero-fill around MMIO and ROM windows. Image/MMIO overlap is refused.
    Empty gaps deliberately read 00h to preserve goldens; real empty sockets
    float, often reading FFh. These model gaps are read-only, so guest writes
    and `RamLoad` images cannot introduce uncatalogued extension code.
  - A video ROM (`kind = video-bios`) sits in C0000h–C7FFFh.
- **Scan order** is ascending address, because every surveyed BIOS scans upward
  in 2 KiB steps:
  - Kiselev C800h–F800h, Bochs C800h–E000h, Super PC/Turbo XT C000h–FE00h
    (READ);
  - GLaBIOS with `ROM_SCAN_ALIGN = 1` (READ);
  - the IBM 5170 (ASSUMED).

  The file's list order carries no meaning. Assembly records resolved ROMs,
  including their effective catalogue services, in ascending address order in
  `AssembledMachine.option_roms` for diagnostics and tests.
- **Derived configuration.**
  - CMOS (AT) and DIP switches (XT) are derived from the memory, drives,
    cards and the fixed-disk provider.
  - The CMOS fixed-disk type bytes (12h, 19h, 1Ah) stay **0** when an option
    ROM provides `fixed-disk`. They are derived from `HardDisk` drives only when
    the system BIOS's entry declares `cmos-drive-types`.
  - This refines machine-config.md, which derives the nibble whenever a hard
    disk is attached.

Rust sketch (lowering target unchanged):

```rust
pub struct BoardConfig { pub model: BoardModelId }          // class derived
pub struct FirmwareConfig { pub bios: FirmwareImage, pub option_roms: Vec<OptionRom> }
pub struct FirmwareImage { pub source: FirmwareSource, pub sha256: Option<[u8; 32]>, pub id: Option<FirmwareId> }
pub enum FirmwareSource { Single(ImageSource), Split { even: ImageSource, odd: ImageSource } }
pub struct OptionRom { pub image: FirmwareImage, pub base: Option<u64> }
// board::lower(&Description) -> Result<MachineConfig, ConfigError>
```

## 6. Validation rules

Description-level checks run before lowering. Hash checks run where file bytes
are read (assembly already validates file lengths after reading). The lowered
`MachineConfig` then passes the existing `validate()`. Each rule is a new
`ConfigError` variant with an exact message under test.

| # | Rule | Error | Example message |
|---|---|---|---|
| 1 | The board model is known | `UnknownBoardModel` | `unknown board model "ibm-5171"; known: ibm-5170, ibm-5160 (reserved), …` |
| 2 | `class` is not written | `ClassIsDerived` | `"class" is derived from [board] model; remove it (ibm-5170 is class "at")` |
| 3 | The CPU model is a socket option of the board | `CpuNotOnBoard` | `board "ibm-5160" takes cpu 8088 or v20, not "80286"` |
| 4 | The CPU model is implemented by the core | `CpuModelUnavailable` | `cpu model "8088" is reserved: the CPU core does not model it yet (x86jit xj-50w.2)` |
| 5 | The board model is not reserved | `BoardUnavailable` | `board "ibm-5160" is reserved until an XT-class CPU model exists` |
| 6 | The firmware hash is in the board's catalogue | `FirmwareNotListed` | `bios image (sha256 6d50…) is not a known image for board "ibm-5170"; known: kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh), ibm-5170-bios-1986 (user-supplied)` |
| 7 | The catalogue entry fits this board | `FirmwareBoardMismatch` | `firmware "glabios-0.4.3-5160" is built for ibm-5160, not ibm-5170` |
| 8 | The CPU meets the firmware minimum | `FirmwareNeedsCpu` | `firmware "seabios-1.17.0-isapc" needs an 80386 or later; cpu is 80286` |
| 9 | The firmware's required parts are present | `FirmwareRequiresDevice` | `firmware "bochs-rombios-legacy" requires a video-bios option ROM at 0xC0000; none is configured` |
| 10 | Each card that needs a service has a provider | `NoProvider` | `device Ide needs firmware service fixed-disk; no ROM provides it` |
| 11 | An option ROM's baked resources match a card | `OptionRomCardMismatch` | `option ROM "xub-r638-at" drives ATA at 0x1F0/IRQ14, but no ide card decodes 0x1F0` |
| 12 | An option ROM is aligned and inside the firmware's scan window | `OptionRomMisaligned`, `OptionRomOutsideScanWindow` | `option ROM at 0xF8000 is outside the scan window 0xC8000..0xF8000 of "kiselev-xi8088-1.0.2"` |
| 13 | An option ROM's header, length and checksum are valid | `OptionRomBadHeader`, `OptionRomBadChecksum` | `option ROM xub-r638-at: byte sum is 0xB0, must be 0 (pad and checksum the image)` |
| 14 | No two ROMs share a base | (existing `ImageOverlap`) | — |
| 15 | Each card fits the board bus | `CardNeedsSlot` | `card "ide" is a 16-bit ISA card; board "ibm-5160" has 8-bit slots only` |
| 16 | The IRQ line exists on the board | `InvalidIrqLine` (bounded per board) | `device Fdc uses irq line 9; board "ibm-5160" has irq lines 0 through 7` |
| 17 | A system BIOS is top-aligned with a valid size | `BiosPlacement` | `system BIOS of 24 KiB at 0xf8000 must have a positive size that is a multiple of 8 KiB and end at 0x100000` |

Rule 10 is implemented through `LoadImage.provides` and kind-owned requirements:
IDE requires `FixedDisk` even without drives; FDC requires `Floppy` when a drive
is bound. `Bare` rigs skip these rules. The stable assembly diagnostic is
`device Ide needs firmware service fixed-disk; no ROM provides it`; machine-file
lowering adds the card id and defining file/line. A recognized system BIOS takes its services from the catalogue; an omitted or
empty declaration uses that set, and a nonempty restatement must match it.
A mismatch reports both sets as `FirmwareProvidesMismatch`, even with the
unlisted-firmware escape enabled. Unlisted firmware retains declared services.
Listed option ROMs use the same comparison and SHA-256 allowlist. Their
board, CPU minimum and required parts are mandatory, including under the escape.
An ATA resource mismatch reports `OptionRomCardMismatch`; other missing parts
report `FirmwareRequiresDevice`. System-BIOS-provided CMOS drive types remain
separate work. IDE currently leaves CMOS fixed-disk types zero.

A system BIOS entry with no scan window rejects every option ROM with
`FirmwareDoesNotScanOptionRoms`: `firmware X does not scan for option ROMs`.
Machine-file loaders accept run-only `AssemblyOptions` through their
`*_with_options` / `load_with_reader_and_options` APIs. Defaults stay strict;
allowing unlisted option ROMs retains the defining file/line and does not
bypass format, placement or compatibility checks.

Implementation diagnostics additionally pin these exact examples:

- `option ROM at 0xC8001 must be 2 KiB aligned` (`OptionRomMisaligned`).
- `option ROM at 0xC8000: expected 55AAh signature and a length byte`
  (`OptionRomBadHeader`).
- `option ROM at 0xC8000: length byte declares 1536 bytes, image has 2048 (must match and be nonzero)`
  (`OptionRomBadHeader`). Exact length prevents trailing bytes from hiding
  additional executable images outside the declared, checksummed module.
- `board "ibm-5170" adapter ROM space overlaps ram at 0xc8000`
  (`AdapterSpaceOverlapsRam`); a RAM load without RAM backing retains
  `ImageOutsideRam`.
- `option ROM image (sha256 …) is not a known image for board "ibm-5170"; known: xub-r638-at (open, vendor/xtide-universal-bios/fetch.sh)`
  (`OptionRomNotListed`).

**READ hardware reference:** *IBM Personal Computer AT Technical Reference*
(March 1984), System BIOS, **Adapters with System-Accessible ROM Modules**,
p. 5-11, specifies the 55h/AAh signature, length in 512-byte blocks,
modulo-256 byte sum of zero, 2 KiB scan steps and FAR CALL at offset 3.
It documents **C8000h through E0000h**, not the XT's F4000h limit.
[Original IBM scan](https://ftp3.us.freebsd.org/pub/misc/bitsavers/pdf/ibm/pc/at/1502494_PC_AT_Technical_Reference_Mar84.pdf).
**READ firmware:** pinned Kiselev `8088_bios/src/bios.asm`, `detect_rom_ext`
(lines 1390–1435), scans **[C8000h, F8000h)** with default CMOS; a setup bit
can shorten it to F0000h. `extension_scan` (lines 1532–1571) verifies the
signature, nonzero length and checksum, advances past the declared module,
rounds upward to 2 KiB, and returns offset 3 for FAR CALL. The earlier video
scan covers [C0000h, C8000h); that whole interval is isolated from RAM too.
Catalogue placement uses the default extension window. Custom NVRAM that
shortens the scan can make an otherwise valid F0000h–F7FFFh ROM undiscoverable;
`option_roms` records the default scan order, not execution under altered NVRAM.

Rule 13 rejects the raw r638 `ide_at.bin` (MEASURED byte sum B0h), which is the
intended behaviour: the fixture must pad and checksum it.

## 7. Recommendation

**(a) A hard disk soon on the AT machine.** Keep the Kiselev Xi 8088 BIOS as
it is. Add the **XUB AT build (`ide_at`, r638, GPL-2.0-or-later) as an option
ROM at C8000h** on a **new profile** next to the old one. Add an **ATA device**
at 1F0h–1F7h and 3F6h on IRQ14. This works for four reasons:

- the Kiselev IPL already falls back to INT 13h DL=80h;
- XUB installs its own INT 19h loader and chains INT 13h (READ);
- the AT build's defaults are exactly the standard primary channel (READ);
- upstream itself ships combined Xi 8088 + XUB ROMs.

The original investigation identified two prerequisites:

1. **x86jit string port I/O** (`INS`/`OUTS` with `REP`) in Real16 is now
   available at the current pin `73ed5d6`, with machine integration tests.
   The historical measurement at `96a49a9` found it missing; an XUB boot
   remains a separate qualification.
2. **Port split at 3F6h.** The FDC's decode shrinks from 3F0h–3F7h to
   3F0h–3F5h plus 3F7h, so the IDE owns 3F6h.

The fallback, if the CPU seam is late, is the 8088 build `ide_xt`, which uses
`in ax,dx` loops. It defaults to an 8-bit XT-IDE at 300h, so it would need a
source build with AT defaults. Prefer waiting for the seam.

**(b) Phase B.** Add a `generic-386-isa` board with **SeaBIOS built with QEMU's
`isapc` configuration** (LGPL-3.0, 128 KiB, no PCI) plus **SeaVGABIOS**. The
reasons:

- it is the only maintained open firmware that targets an ISA-only 386/486
  board (READ: QEMU `config.seabios-128k`, `isapc.c`);
- it provides AT services and A20;
- it builds with gcc;
- the same firmware family carries into phases C/D on `i440fx-piix3`.

Bochs `rombios` legacy (LGPL-2.1, 386+, port-92h A20, built-in ATA) is the
fallback. Both need a VGA option ROM and therefore a register-level VGA, so
phase B pulls VGA forward, unless headless phase B goldens use SeaBIOS SERCON
(§8). Neither runs on the 286 board, so `ibm-5170` keeps the Kiselev BIOS
permanently as the smallest test bed.

**(c) An XT-class machine later.** Use an `ibm-5160` board with **GLaBIOS
`ARCH_5160`** as its default firmware. It explicitly targets the standard 5160
and emulators, and it is GPL-3.0. Pin the upstream release ROM by SHA-256,
because the build needs MASM. List **Kiselev `MACHINE_XT`** as a second entry
for differential runs; it builds with NASM like today's fixture, but upstream
has not tested it. The board waits for `xj-50w.2 (x86jit)`. The 8255, the XT
keyboard and the DIP-switch device work can land before the CPU model.

**Goldens.**

- Never switch a profile's firmware. **A new firmware is a new profile with new
  goldens.** The old `presets::bios()` profile stays byte-identical.
- The first bead re-expresses `presets::bios()` as `ibm-5170` + the Kiselev
  catalogue entry. It proves `MachineConfig` equality. Config is `Eq`, so this
  is a plain assertion, not an A/B run.
- The firmware hash becomes part of the machine identity, alongside the
  x86jit pin and the run policy.
- Board completion work (DMA2, reset line) proves the old goldens unchanged, or
  re-baselines deliberately with the reason stated.

## 8. Risks and open questions

Risks:

- **CPU qualification for (a).** The INS/OUTS prerequisite is satisfied at
  `73ed5d6`. The XUB AT build may use another unlifted opcode, which only a
  boot will show.
- **XUB OS hooks.** XUB calls INT 15h 90h/91h with AL=00h. The Kiselev BIOS
  answers AH=86h with CF set for those (READ). ASSUMED: XUB ignores the result.
- **Conventional memory.** XUB full mode takes 1 KiB and the Kiselev BIOS
  reserves a 1 KiB EBDA. Upstream lists sharing them as a TODO (READ README).
  `MEM` output will differ in the new profile, which is expected.
- **The 3F6h/3F7h split** changes the FDC's decode in the new profile only.
  On real hardware 3F7h is shared: bit 7 is the floppy disk change, bits 0–6
  belong to the fixed-disk adapter. Bits 0–6 are not modelled.
- **SeaBIOS assumes QEMU** in places: the 0x402 debug port, fw_cfg absence,
  PAM shadowing and its CMOS memory layout. Each needs checking on a non-QEMU
  board (ASSUMED harmless or worked around, as QEMU `isapc` does).
- **Windows 98 SE on SeaBIOS** has PnP and ACPI quirks and QEMU-version
  regressions (READ, secondary sources).
- **Section citations.** The Technical Reference section titles were read from
  tables of contents, not from the full scans.
- **GLaBIOS builds need MASM.** Pinning release binaries trades
  source-reproducibility for availability. An open assembler (JWasm/UASM) is
  unverified.
- **User-supplied ROMs** may trip vendor CMOS checksums without an NVRAM file.

Open questions for the maintainer:

1. **Naming.** Is it acceptable to call today's machine `ibm-5170` while it
   lacks DMA2, 80287 ports and the reset line (listed deviations), or should it
   carry an interim name until those land?
2. **Unlisted firmware — resolved (decision-18).** A strict allowlist by
   default, with the run-only `--allow-unlisted-firmware` developer flag and
   a full-hash warning. The flag is absent from machine files and goldens;
   it does not bypass BIOS presence, placement, reset or memory isolation.
3. **XUB fixture.** Should the fixture build from a pinned SVN revision with
   NASM, or download r638 and pad and checksum it deterministically? Both pin
   two hashes.
4. **Phase B console.** Should phase B pull a register-level VGA forward (for
   SeaVGABIOS), or run headless goldens on SeaBIOS SERCON (serial) first?
5. **XT default firmware.** GLaBIOS (tested on a 5160; MASM-built binary) or
   Kiselev `MACHINE_XT` (NASM; untested upstream)?
6. **NVRAM file.** Should a CMOS NVRAM file (from the `upc-6le.1` notes) be in
   schema v1 for user-supplied BIOSes, or wait for the first enthusiast board?
7. **386 board name.** The proposed name is `generic-386-isa`.

## 9. Implementation beads

Each bead is narrow; dependencies are in brackets. Board models replace `class`.
The generic AT comes first.

1. **Board model `ibm-5170` as data (generic AT).** `presets::bios()` becomes
   `ibm-5170` + firmware. The class `at` is derived. The board doc table lists
   modelled and missing 5170 parts with AT Technical Reference citations.
   - *Acceptance:* `MachineConfig` equality with today's preset for both floppy
     variants; every headless and slow golden unchanged.
   - *Depends on:* none.
2. **Firmware catalogue with a SHA-256 allowlist.** Add `FirmwareEntry` (id,
   kind, sha256, size, base rule, provides, requires, cpu_min, boards,
   distribution, status) and the `kiselev-xi8088-1.0.2` entry. Hashes are
   checked at image read. Errors 6–9 and 17.
   - *Acceptance:* each error has an exact-message test; a one-byte-changed ROM
     is rejected with the list of known entries; the goldens load through the
     catalogue.
   - *Implemented:* board-owned system BIOS catalogue in `assembly`, checked
     after image resolution and before ROM mapping; rules 6–9 and 17 have exact
     diagnostic tests. The CLI and window support the run-only
     `--allow-unlisted-firmware` escape hatch with a full-hash warning. Bare rigs
     skip assembly checks; the historical POST runner checks its intended BIOS
     explicitly. System BIOS and option ROM service claims now come from catalogue entries.
   - *Depends on:* 1.
3. **Description `[board]` and `[firmware]` sections.** `model` drives
   everything; `class` is rejected with a hint. Reserved CPU models (`8088`,
   `8086`, `v20`, `80386`, …) and reserved boards are rejected (rules 1–5).
   - *Acceptance:* the TOML and YAML of today's FreeDOS machine lower to a
     config equal to the preset; reserved values give tested messages.
   - *Depends on:* 2, and the description-loader bead from `upc-6le.1`.
4. **Option-ROM placement and scan.** Alignment, scan window, header, length
   and checksum (rules 12–14); RAM holes; ROM mapping; the effective scan order
   in `Assembled`.
   - *Acceptance:* a synthetic 2 KiB test ROM (inline bytes) is called by the
     Kiselev BIOS (the extension-init POST code is seen) in a slow test; old
     goldens unchanged.
   - *Implemented:* rules 11–14, catalogue-owned services and strict option-ROM
     membership, system-BIOS identification by its end at 1 MiB, read-only
     adapter space, and `AssembledMachine.option_roms`. A fixture-gated slow test
     checks a synthetic C8000h ROM's RAM marker and POST code between extension
     detection and completion. Existing golden expectations are unchanged.
   - *Depends on:* 2.
5. **x86jit request: Real16 string port I/O.** `INS`/`OUTS` bytes and words
   with `REP`, restartable per element, filed and landed in x86jit, then
   re-pinned here. The CPU core supports this at `73ed5d6`; machine tests
   cover short transfers. The 256-word sector transfer remains this later
   bead's acceptance test.
   - *Acceptance (here):* a machine test where `rep insw` reads 256 words from
     a mock port device; all goldens re-run green at the new pin.
   - *Depends on:* the x86jit bead.
6. **ATA device, isolated.** The task file at 1F0h–1F7h, device control and
   alternate status at 3F6h, IRQ14, master and slave, PIO. Commands: IDENTIFY
   DEVICE (CHS + LBA28), READ/WRITE SECTORS, READ/WRITE MULTIPLE, SET MULTIPLE
   MODE, INITIALIZE DEVICE PARAMETERS, SET FEATURES, RECALIBRATE, SEEK, EXECUTE
   DEVICE DIAGNOSTIC and SRST. Writes are volatile; journaling follows
   `upc-6le.3`. The coverage matrix cites ATA-2 and the AT *Fixed Disk and
   Diskette Drive Adapter*.
   - *Acceptance:* mock-bus tests per command and error path; a snapshot round
     trip.
   - *Depends on:* none.
7. **FDC/IDE port split.** With an `ide` card on the primary channel, the FDC
   decodes 3F0h–3F5h and 3F7h, and the IDE decodes 3F6h.
   - *Acceptance:* the unsplit overlap is rejected; presets without `ide` are
     unchanged.
   - *Depends on:* 6.
8. **XUB fixture.** Add `vendor/xtide-universal-bios/fetch.sh`, which builds or
   fetches a pinned AT build, pads it to 8 KiB, fixes the checksum and checks
   both hashes. Its README records the binary download centre's exact GPL
   version 2 statement (without an "or later" grant), separately from the
   source-header survey above. Add the catalogue
   entry `xub-r638-at` (provides `fixed-disk`; requires a CPU ≥ 80286 and
   `ata@1F0/14`). Only the script and README are committed in the vendor directory.
   - *Acceptance:* deterministic output and an offline rebuild when cached.
   - *Implemented:* r638 `ide_at.bin` is hash-checked, padded with 00h to
     8 KiB and checksummed at the last byte (50h). The output SHA-256 is
     `6409de912eab620ea144b79e723bed478814047dc2766fc19fd6e2e3690662d0`.
     `FirmwareEntry::XUB_R638_AT` records the `Listed` option-ROM identity,
     minimum CPU, fixed-disk service and primary ATA requirement. It remains
     part of the board's enforced firmware catalogue for option-ROM validation. Primary IDE assembly now satisfies the ATA requirement
     with the 1F0h task file, 3F6h control and IRQ14 wiring. Catalogue/recipe and
     fixture-gated hash tests cover the entry.
   - *Depends on:* 2.
9. **Fixed-disk provider rules.** Rules 10–11, and the CMOS fixed-disk type
   follows the provider (0 when it is an option ROM).
   - *Acceptance:* tests; the existing CMOS seed is byte-identical.
   - *Depends on:* 2, 6, 8.
10. **Hard-disk image fixture.** A script builds a partitioned FAT16 image (MBR,
    one partition, a marker file) with open tools; the hash is pinned and the
    image is gitignored.
    - *Acceptance:* identical bytes across runs.
    - *Depends on:* none.
11. **Profile `ibm-5170` + XUB + IDE: FreeDOS sees `C:`.** A new description or
    preset next to the old one, with a new slow golden (floppy boot, `DIR C:`
    lists the marker).
    - *Acceptance:* the new golden is green; the old goldens are byte-identical.
    - *Depends on:* 4, 5, 7, 8, 9, 10.
12. **`ibm-5170` completion: second 8237** at C0h–DFh with pages 89h–8Fh.
    - *Acceptance:* device tests; goldens unchanged, or re-baselined with a
      stated reason (the BIOS writes C0h for I/O delays).
    - *Depends on:* 1.
13. **Phase B firmware spike (coarse).** SeaBIOS `isapc` build on
    `generic-386-isa`. The output is the requirement list (VGA ROM or SERCON,
    writable shadow, port 92h, A20 masking, 8042 reset, extended-memory CMOS)
    and the x86jit requests it files.
    - *Depends on:* the phase B gap analysis, 1–3.
14. **XT device set, device level.** The 8255 PPI with DIP-switch derivation,
    the XT keyboard interface (83-key, set 1, PB6/PB7), a single-mode 8259
    check, the XT page-register map and the NMI mask at A0h, citing XT Technical
    Reference §1. The `ibm-5160` board stays reserved.
    - *Acceptance:* mock-bus tests; validation keeps rejecting the board and
      the 8088 model with tested messages.
    - *Depends on:* 1, 3.
15. **XT firmware and first XT golden.** The GLaBIOS `ARCH_5160` release ROM
    pinned by hash; the Kiselev `MACHINE_XT` NASM build as a second entry. The
    golden boots the FreeDOS 8086 floppy on `ibm-5160`.
    - *Depends on:* 14, `xj-50w.2 (x86jit)` and its re-pin.
16. **User-supplied ROM entries.** Hash-only catalogue entries, even/odd parts
    and the `FirmwareNotListed` listing. No chipset models are implied.
    - *Acceptance:* tests with synthetic bytes only.
    - *Depends on:* 2. Lowest priority.

## Sources

- Kiselev `8088_bios`: <https://github.com/skiselev/8088_bios>, pinned at
  `a4af458` (2026-07-16): `src/config.inc`, `bios.asm`, `misc.inc`,
  `cpu.inc`, `floppy2.inc`, README.
- GLaBIOS: <https://github.com/640-KB/GLaBIOS>, `src/GLABIOS.ASM` and
  `MAKE.BAT` at `68c65a5` (2026-07-23).
- Super PC/Turbo XT BIOS 3.1: <https://www.retrocmp.de/software/pcxtbios/readme.txt>,
  <https://www.phatcode.net/downloads.php?id=101>.
- XTIDE Universal BIOS: <https://www.xtideuniversalbios.org/> and SVN trunk
  `Src/Main.asm`, `Src/Initialization/Initialize.asm`,
  `Src/Initialization/Interrupts.asm`, `Src/Handlers/Int13h.asm`,
  `Src/Device/IDE/IdePioBlock.asm`; binaries
  <https://www.xtideuniversalbios.org/binaries/r638/> (2026-06-09).
- Bochs `rombios`: <https://github.com/bochs-emu/Bochs/tree/master/bochs/bios>
  (`rombios.c`, `rombios.h`, `Makefile.in`); Windows 98 SE guide
  <https://bochs.sourceforge.io/doc/docbook/user/guest-win98.html>.
- SeaBIOS: <https://github.com/coreboot/seabios> master (`docs/Memory_Model.md`,
  `docs/Releases.md`, `src/fw/shadow.c`, `src/fw/pciinit.c`,
  `src/fw/paravirt.c`, `src/Kconfig`, `vgasrc/Kconfig`).
- QEMU: `roms/config.seabios-128k` (last change 2023-09-25, "seabios: remove
  PCI drivers from bios.bin"), `hw/i386/isapc.c`,
  <https://www.qemu.org/docs/master/system/i386/pc.html>.
- Windows 98 SE on QEMU: <https://bugs.launchpad.net/bugs/1819289> (2019),
  <https://mail.coreboot.org/pipermail/seabios/2012-May/003887.html>.
- fysnet i440fx BIOS: <https://fysnet.github.io/i440fx>.
- Intel 440FX: <https://en.wikipedia.org/wiki/Intel_440FX>; 440BX:
  <https://en.wikipedia.org/wiki/Intel_440BX>; datasheet 290549-001 (May 1996).
- IBM Technical References: AT 1502494 (March 1984) and 6183355 (March 1986) on
  bitsavers; XT 6139821 (1986) tables of contents.
- x86jit pin `96a49a9`: `x86jit-core/src/lift/mod.rs`, port I/O note on
  `ins`/`outs`.
