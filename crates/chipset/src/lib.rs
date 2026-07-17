// SPDX-License-Identifier: AGPL-3.0-or-later
//! Chipset swap seam (§3.2 seam 2, §3.3).
//!
//! A [`Chipset`] is the one place that **declares** the machine's port/MMIO
//! decode map, the device set behind it, and the interrupt controller (§3.3,
//! §5.1). The `bus` executes what the chipset declares; nothing else owns decode
//! policy, and the `machine` core reaches interrupts only through the
//! [`InterruptController`] trait object the chipset produces.
//!
//! Two concrete chipsets:
//! - [`SerialChipset`]: a single 16550 UART at COM1 and no interrupt controller
//!   — the Long64 serial walking skeleton (§7.2).
//! - [`AtChipset`]: a PC/AT-style set — UART at COM1, 8259 PIC at `0x20`/`0x21`,
//!   8253 PIT at `0x40`–`0x43`, 8042 keyboard controller at `0x60`/`0x64`, and a
//!   VGA text card (CRTC at `0x3D4`/`0x3D5`, text buffer at `0xB8000`) — with the
//!   PIC wired as the interrupt controller, enough to boot a real-mode guest that
//!   takes a timer or keyboard interrupt and paints the text screen (§5, §7, §9).

mod shared_pic;

pub use shared_pic::SharedPic;

use bus::{DecodeMap, MmioRange, PortRange};
use device::Device;
use devices::vga_text::{CRTC_DATA_PORT, CRTC_INDEX_PORT, TEXT_BUFFER_BASE, TEXT_BUFFER_LEN};
use devices::{Pit8253, Uart16550, VgaText, I8042};

// The interrupt-controller seam lives next to the `Device` trait; the chipset
// only needs to name it and provide a default no-op implementation.
pub use device::InterruptController;

/// COM1 base port on the IBM PC/AT (§7.2).
pub const COM1_BASE: u16 = 0x3F8;
/// COM1 top port (base + 7; the 16550 decodes eight registers).
pub const COM1_TOP: u16 = 0x3FF;

/// Master 8259 PIC command port (§7.3).
pub const PIC_COMMAND: u16 = 0x20;
/// Master 8259 PIC data port.
pub const PIC_DATA: u16 = 0x21;

/// 8253 PIT base port (counter 0 data) (§7.4).
pub const PIT_BASE: u16 = 0x40;
/// 8253 PIT top port (mode/command register).
pub const PIT_TOP: u16 = 0x43;

/// 8042 keyboard-controller data port (read: output buffer; write: to keyboard
/// or a pending command byte) (§7.5).
pub const KBD_DATA: u16 = 0x60;
/// 8042 keyboard-controller status port (read) / command port (write).
pub const KBD_CMD: u16 = 0x64;

/// A no-op interrupt controller: no lines, never acknowledges (§5.1).
///
/// The controller for a chipset that models no PIC (the serial skeleton): every
/// method is inert so no interrupt is ever taken. [`AtChipset`] uses the real
/// [`SharedPic`] instead.
#[derive(Default)]
pub struct NoInterruptController;

impl InterruptController for NoInterruptController {
    fn set_irq_line(&mut self, _irq: u8, _level: bool) {}

    fn poll(&self) -> Option<u8> {
        None
    }

    fn acknowledge(&mut self) -> Option<u8> {
        None
    }

    fn end_of_interrupt(&mut self) {}
}

/// The chipset swap seam (§3.2 seam 2, §3.3).
///
/// A chipset produces the device set, the matching decode map, and the interrupt
/// controller wired to the device IRQ lines. The machine assembles a `bus` from
/// the device set + map and reaches interrupts through the controller — never
/// naming a concrete chip.
pub trait Chipset {
    /// Consume the chipset and produce the ordered device set, the decode map
    /// whose indices reference it, and the interrupt controller (§3.3, §5.1).
    ///
    /// When the chipset models a PIC, the controller shares state with the PIC's
    /// [`Device`] entry in the returned set, so a guest EOI (a port write through
    /// the bus) is seen by the machine's INTA path.
    fn build(
        self,
    ) -> (
        Vec<Box<dyn Device>>,
        DecodeMap,
        Box<dyn InterruptController>,
    );
}

/// Serial-only chipset: one 16550 UART at COM1, no interrupts (§7.2).
#[derive(Default)]
pub struct SerialChipset;

impl SerialChipset {
    /// Create the serial chipset.
    pub fn new() -> Self {
        Self
    }
}

impl Chipset for SerialChipset {
    fn build(
        self,
    ) -> (
        Vec<Box<dyn Device>>,
        DecodeMap,
        Box<dyn InterruptController>,
    ) {
        let uart = Uart16550::new(COM1_BASE);
        let devices: Vec<Box<dyn Device>> = vec![Box::new(uart)];
        let map = DecodeMap {
            ports: vec![PortRange {
                ports: COM1_BASE..=COM1_TOP,
                device: 0,
            }],
            mmio: Vec::new(),
        };
        (devices, map, Box::new(NoInterruptController))
    }
}

/// Device index of the UART in the [`AtChipset`] device set (COM1 draining).
pub const AT_UART_INDEX: usize = 0;
/// Device index of the 8259 PIC in the [`AtChipset`] device set.
pub const AT_PIC_INDEX: usize = 1;
/// Device index of the 8253 PIT in the [`AtChipset`] device set (IRQ0 source).
pub const AT_PIT_INDEX: usize = 2;
/// Device index of the 8042 keyboard controller in the [`AtChipset`] device set.
///
/// Its keyboard-byte-ready line is the IRQ1 source; a frontend wires
/// `IrqLine { device: AT_I8042_INDEX, irq: 1 }` so the machine edge-detects it
/// onto the PIC (§5.1).
pub const AT_I8042_INDEX: usize = 3;
/// Device index of the VGA text card in the [`AtChipset`] device set. A renderer
/// reads its text buffer through the tracing seam after a run (§9.2).
pub const AT_VGA_INDEX: usize = 4;

/// PC/AT-style chipset: UART + 8259 PIC + 8253 PIT + 8042 keyboard + VGA text,
/// PIC as the controller (§7, §9).
///
/// The device set is `[UART, PIC, PIT, 8042, VGA]`. The PIC is a [`SharedPic`]:
/// the same live chip backs both its bus [`Device`] entry (guest programming +
/// EOI) and the [`InterruptController`] the machine drives. Two device lines are
/// interrupt sources the machine edge-detects onto the PIC (§5.1): the PIT's
/// counter-0 OUT (IRQ0) and the 8042's keyboard-byte-ready line (IRQ1). The VGA
/// card drives no line — it is memory the renderer reads after the run. The
/// chipset only declares the parts and the decode map.
pub struct AtChipset {
    pic: SharedPic,
    /// Scancodes pre-loaded into the keyboard controller's input queue before the
    /// machine runs. The frontend owns the concrete keyboard only here, before the
    /// devices are boxed behind `dyn Device`, so this is where key input is seeded
    /// without the machine core ever naming the 8042.
    keyboard_input: Vec<u8>,
}

impl AtChipset {
    /// Create the PC/AT chipset with a fresh, unprogrammed PIC, PIT, keyboard, and
    /// a blank VGA text card, and no pending key input.
    pub fn new() -> Self {
        Self {
            pic: SharedPic::new(),
            keyboard_input: Vec::new(),
        }
    }

    /// Create the PC/AT chipset with `scancodes` pre-loaded into the keyboard
    /// controller's input queue.
    ///
    /// Seeding here — while the frontend still owns the concrete 8042, before it
    /// is boxed into the device set — is how a headless run injects key input:
    /// the queued bytes surface at port `0x60` and raise IRQ1 once the guest
    /// enables keyboard interrupts, exactly as if typed at power-on.
    pub fn with_keyboard_input(scancodes: &[u8]) -> Self {
        Self {
            pic: SharedPic::new(),
            keyboard_input: scancodes.to_vec(),
        }
    }
}

impl Default for AtChipset {
    fn default() -> Self {
        Self::new()
    }
}

impl Chipset for AtChipset {
    fn build(
        self,
    ) -> (
        Vec<Box<dyn Device>>,
        DecodeMap,
        Box<dyn InterruptController>,
    ) {
        let uart = Uart16550::new(COM1_BASE);
        let pit = Pit8253::new();
        let mut keyboard = I8042::new();
        for &scancode in &self.keyboard_input {
            keyboard.push_scancode(scancode);
        }
        let vga = VgaText::new();
        // The PIC handle is cloned: one clone drives the bus device face, the
        // other becomes the machine's interrupt controller. Both share one chip.
        let pic_device = self.pic.clone();
        let controller: Box<dyn InterruptController> = Box::new(self.pic);

        let devices: Vec<Box<dyn Device>> = vec![
            Box::new(uart),       // index 0 (AT_UART_INDEX)
            Box::new(pic_device), // index 1 (AT_PIC_INDEX)
            Box::new(pit),        // index 2 (AT_PIT_INDEX)
            Box::new(keyboard),   // index 3 (AT_I8042_INDEX)
            Box::new(vga),        // index 4 (AT_VGA_INDEX)
        ];
        let map = DecodeMap {
            ports: vec![
                PortRange {
                    ports: COM1_BASE..=COM1_TOP,
                    device: AT_UART_INDEX,
                },
                PortRange {
                    ports: PIC_COMMAND..=PIC_DATA,
                    device: AT_PIC_INDEX,
                },
                PortRange {
                    ports: PIT_BASE..=PIT_TOP,
                    device: AT_PIT_INDEX,
                },
                // The 8042 decodes two non-adjacent ports (0x60 and 0x64); 0x61–
                // 0x63 belong to other AT hardware, so each is its own range.
                PortRange {
                    ports: KBD_DATA..=KBD_DATA,
                    device: AT_I8042_INDEX,
                },
                PortRange {
                    ports: KBD_CMD..=KBD_CMD,
                    device: AT_I8042_INDEX,
                },
                PortRange {
                    ports: CRTC_INDEX_PORT..=CRTC_DATA_PORT,
                    device: AT_VGA_INDEX,
                },
            ],
            mmio: vec![MmioRange {
                addrs: TEXT_BUFFER_BASE..=TEXT_BUFFER_BASE + TEXT_BUFFER_LEN as u64 - 1,
                device: AT_VGA_INDEX,
            }],
        };
        (devices, map, controller)
    }
}
