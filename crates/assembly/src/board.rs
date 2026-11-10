// SPDX-License-Identifier: AGPL-3.0-or-later
//! Motherboard identity and standard wiring, as data.
//!
//! IBM Personal Computer AT Technical Reference, Section 1 "System Board":
//! "System Interrupts", "System Timers", "Direct Memory Access", "I/O Address
//! Map", "Keyboard Controller", and "Real-Time Clock/CMOS RAM Information".
//! See `backlog/docs/design/machine-config.md` §Board model for the coverage table.

use crate::device::{
    CmosCfg, DeviceConfig, DeviceId, DeviceKind, DmaCfg, I8042Cfg, PicCfg, PitCfg, PortWindow,
    PostPortCfg, SysControlCfg,
};

/// Hardware family derived from a motherboard, never selected independently.
/// See `backlog/docs/design/machine-config.md` §Board model.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MachineClass {
    /// AT interrupt, keyboard and CMOS wiring.
    At,
}

/// Named motherboard constraints on the flat device list.
///
/// IBM Personal Computer AT Technical Reference, Section 1 "System Board"
/// defines the AT wiring. The generic AT uses the existing 8253 timer model
/// and one 8237; DMA2, 80287 ports and CPU reset remain unmodelled. See
/// `backlog/docs/design/machine-config.md` §Board model.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BoardModel {
    /// Generic AT with the documented partial IBM 5170 device coverage.
    Ibm5170,
    /// Hand-wired Rust test rigs, without motherboard constraints or a class.
    Bare,
}

impl BoardModel {
    /// The hardware class implied by this model; a bare rig has no class.
    pub const fn class(self) -> Option<MachineClass> {
        match self {
            Self::Ibm5170 => Some(MachineClass::At),
            Self::Bare => None,
        }
    }

    /// Default motherboard roles in bus order, with standard ports and wiring.
    ///
    /// IBM Personal Computer AT Technical Reference, Section 1 "System Board",
    /// I/O Address Map and System Interrupts: slave PIC cascades on IRQ2, PIT
    /// drives IRQ0, keyboard IRQ1 and RTC IRQ8. Only DMA channels 0–3 exist in
    /// the current model; motherboard devices do not request DMA transfers.
    /// POST defaults to 80h, so DMA page decoding starts at 81h. Validation also
    /// accepts a relocated/omitted POST latch and the corresponding 80h page.
    pub fn motherboard_devices(self) -> Vec<DeviceConfig> {
        if self == Self::Bare {
            return Vec::new();
        }
        use DeviceId::*;
        vec![
            DeviceConfig::new(
                MasterPic,
                DeviceKind::Pic(PicCfg { cascade: true }),
                vec![PortWindow::new(0x20, 0x21)],
            ),
            DeviceConfig::new(
                SlavePic,
                DeviceKind::Pic(PicCfg { cascade: false }),
                vec![PortWindow::new(0xA0, 0xA1)],
            ),
            DeviceConfig::new(
                Pit,
                DeviceKind::Pit(PitCfg {}),
                vec![PortWindow::new(0x40, 0x43)],
            )
            .with_irq(0),
            DeviceConfig::new(
                Dma,
                DeviceKind::Dma(DmaCfg {}),
                vec![PortWindow::new(0x00, 0x0F), PortWindow::new(0x81, 0x8F)],
            ),
            DeviceConfig::new(
                Keyboard,
                DeviceKind::Keyboard(I8042Cfg {}),
                vec![PortWindow::single(0x60), PortWindow::single(0x64)],
            )
            .with_irq(1),
            DeviceConfig::new(
                Rtc,
                DeviceKind::Rtc(CmosCfg {
                    seed: true,
                    timing: None,
                }),
                vec![PortWindow::new(0x70, 0x71)],
            )
            .with_irq(8),
            DeviceConfig::new(
                SysControl,
                DeviceKind::SysControl(SysControlCfg {}),
                vec![PortWindow::single(0x61)],
            ),
            DeviceConfig::new(
                Post,
                DeviceKind::Post(PostPortCfg { port: 0x80 }),
                vec![PortWindow::single(0x80)],
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IBM Personal Computer AT Technical Reference, Section 1 "System Board",
    /// I/O Address Map and System Interrupts. Literal values keep the shared
    /// preset/validator table from becoming its own wiring oracle.
    #[test]
    fn ibm5170_wiring_matches_the_technical_reference() {
        use DeviceId::*;
        let devices = BoardModel::Ibm5170.motherboard_devices();
        let actual: Vec<_> = devices
            .iter()
            .map(|d| (d.id, d.kind, d.ports.clone(), d.irq, d.dma))
            .collect();
        assert_eq!(
            actual,
            vec![
                (
                    MasterPic,
                    DeviceKind::Pic(PicCfg { cascade: true }),
                    vec![PortWindow::new(0x20, 0x21)],
                    None,
                    None
                ),
                (
                    SlavePic,
                    DeviceKind::Pic(PicCfg { cascade: false }),
                    vec![PortWindow::new(0xA0, 0xA1)],
                    None,
                    None
                ),
                (
                    Pit,
                    DeviceKind::Pit(PitCfg {}),
                    vec![PortWindow::new(0x40, 0x43)],
                    Some(0),
                    None
                ),
                (
                    Dma,
                    DeviceKind::Dma(DmaCfg {}),
                    vec![PortWindow::new(0x00, 0x0F), PortWindow::new(0x81, 0x8F)],
                    None,
                    None
                ),
                (
                    Keyboard,
                    DeviceKind::Keyboard(I8042Cfg {}),
                    vec![PortWindow::single(0x60), PortWindow::single(0x64)],
                    Some(1),
                    None
                ),
                (
                    Rtc,
                    DeviceKind::Rtc(CmosCfg {
                        seed: true,
                        timing: None
                    }),
                    vec![PortWindow::new(0x70, 0x71)],
                    Some(8),
                    None
                ),
                (
                    SysControl,
                    DeviceKind::SysControl(SysControlCfg {}),
                    vec![PortWindow::single(0x61)],
                    None,
                    None
                ),
                (
                    Post,
                    DeviceKind::Post(PostPortCfg { port: 0x80 }),
                    vec![PortWindow::single(0x80)],
                    None,
                    None
                ),
            ]
        );
        assert!(devices
            .iter()
            .all(|d| d.access_dependents.is_empty() && !d.allow_port_overlap));
    }
}
