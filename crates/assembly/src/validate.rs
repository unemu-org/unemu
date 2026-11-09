// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration validation (§3.1).
//!
//! [`crate::MachineConfig::validate`] runs a fixed sequence of checks over the
//! pure-data config and returns the first [`ConfigError`] it finds. Each check is
//! a small pure function over the config; none assembles a machine, so the whole
//! module is unit-testable in isolation.

use crate::device::DeviceKind;
use crate::error::ConfigError;
use crate::machine::MachineConfig;

/// Whether two half-open ranges `[a_base, a_end)` and `[b_base, b_end)` share any
/// byte.
fn ranges_overlap(a_base: u64, a_end: u64, b_base: u64, b_end: u64) -> bool {
    a_base < b_end && b_base < a_end
}

impl MachineConfig {
    /// Validate this configuration, returning the first inconsistency found.
    ///
    /// The checks run in a fixed order (device identity, port decode, MMIO
    /// decode, address-space bounds, interrupt wiring, DMA wiring, drive
    /// binding, image geometry, clock quantum, CPU rate) so a malformed config always
    /// yields the same [`ConfigError`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.check_unique_device_ids()?;
        self.check_port_overlaps()?;
        self.check_mmio_devices_present()?;
        self.check_mmio_overlaps()?;
        self.check_mmio_vs_ram()?;
        self.check_regions_in_space()?;
        self.check_irq_conflicts()?;
        self.check_dma_controllers_present()?;
        self.check_drive_controllers()?;
        self.check_image_geometry()?;
        self.check_quantum()?;
        self.clock.cpu_rate()?;
        Ok(())
    }

    /// Every CPU run must yield after a finite, positive number of blocks so
    /// device ticks and the virtual-time cap cannot be starved by guest code.
    fn check_quantum(&self) -> Result<(), ConfigError> {
        match self.clock.quantum {
            Some(1..) => Ok(()),
            quantum => Err(ConfigError::InvalidQuantum(quantum)),
        }
    }

    /// No [`DeviceId`] appears twice.
    fn check_unique_device_ids(&self) -> Result<(), ConfigError> {
        for (i, dev) in self.devices.iter().enumerate() {
            if self.devices[..i].iter().any(|d| d.id == dev.id) {
                return Err(ConfigError::DuplicateDeviceId(dev.id));
            }
        }
        Ok(())
    }

    /// No two devices decode an overlapping port range unless the overlap is
    /// explicitly allowed (first-match order then resolves it).
    fn check_port_overlaps(&self) -> Result<(), ConfigError> {
        for (i, a) in self.devices.iter().enumerate() {
            for b in &self.devices[i + 1..] {
                for wa in &a.ports {
                    for wb in &b.ports {
                        if wa.overlaps(wb) {
                            let allowed = a.allow_port_overlap
                                || b.allow_port_overlap
                                || wa.allow_overlap
                                || wb.allow_overlap;
                            if !allowed {
                                return Err(ConfigError::PortOverlap {
                                    a: a.id,
                                    b: b.id,
                                    port: wa.start.max(wb.start),
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Every MMIO window traps to a device that is present.
    fn check_mmio_devices_present(&self) -> Result<(), ConfigError> {
        for w in &self.memory.mmio {
            if !self.devices.iter().any(|d| d.id == w.device) {
                return Err(ConfigError::MmioDeviceMissing(w.device));
            }
        }
        Ok(())
    }

    /// No two MMIO windows cover an overlapping address range.
    fn check_mmio_overlaps(&self) -> Result<(), ConfigError> {
        let mmio = &self.memory.mmio;
        for (i, a) in mmio.iter().enumerate() {
            for b in &mmio[i + 1..] {
                if ranges_overlap(a.base, a.end(), b.base, b.end()) {
                    return Err(ConfigError::MmioOverlap {
                        a: a.device,
                        b: b.device,
                        addr: a.base.max(b.base),
                    });
                }
            }
        }
        Ok(())
    }

    /// No trapped MMIO window overlaps a RAM region (the CPU core forbids it).
    fn check_mmio_vs_ram(&self) -> Result<(), ConfigError> {
        for w in &self.memory.mmio {
            for r in &self.memory.ram {
                if ranges_overlap(w.base, w.end(), r.base, r.end()) {
                    return Err(ConfigError::MmioOverlapsRam {
                        device: w.device,
                        addr: w.base.max(r.base),
                    });
                }
            }
        }
        Ok(())
    }

    /// Every RAM region, MMIO window, and loaded image fits inside the address
    /// space.
    fn check_regions_in_space(&self) -> Result<(), ConfigError> {
        let space = self.memory.address_space_bytes;
        let check = |base: u64, size: u64| -> Result<(), ConfigError> {
            let end = base.saturating_add(size);
            if base > space || end > space {
                Err(ConfigError::RegionOutOfSpace { base, end, space })
            } else {
                Ok(())
            }
        };
        for r in &self.memory.ram {
            check(r.base, r.size)?;
        }
        for w in &self.memory.mmio {
            check(w.base, w.size)?;
        }
        for img in &self.images {
            check(img.base, img.source.known_len().unwrap_or(0))?;
        }
        Ok(())
    }

    /// No two devices drive the same PIC input line.
    fn check_irq_conflicts(&self) -> Result<(), ConfigError> {
        for (i, a) in self.devices.iter().enumerate() {
            let Some(line) = a.irq else { continue };
            for b in &self.devices[i + 1..] {
                if b.irq == Some(line) {
                    return Err(ConfigError::IrqConflict {
                        a: a.id,
                        b: b.id,
                        line,
                    });
                }
            }
        }
        Ok(())
    }

    /// Every device's DMA wiring names a controller that is present.
    fn check_dma_controllers_present(&self) -> Result<(), ConfigError> {
        for dev in &self.devices {
            if let Some(dma) = &dev.dma {
                if !self.devices.iter().any(|d| d.id == dma.controller) {
                    return Err(ConfigError::DmaControllerMissing {
                        requester: dev.id,
                        controller: dma.controller,
                    });
                }
            }
        }
        Ok(())
    }

    /// Every drive binds to a controller slot that a controller offers.
    fn check_drive_controllers(&self) -> Result<(), ConfigError> {
        let available: u16 = self
            .devices
            .iter()
            .filter_map(|d| match &d.kind {
                DeviceKind::Fdc(fdc) => Some(u16::from(fdc.drive_slots)),
                _ => None,
            })
            .sum();
        for (drive_index, drive) in self.drives.iter().enumerate() {
            if u16::from(drive.controller_slot) >= available {
                return Err(ConfigError::MissingController {
                    drive_index,
                    slot: drive.controller_slot,
                    available: available.min(u16::from(u8::MAX)) as u8,
                });
            }
        }
        Ok(())
    }

    /// Every drive's backing image (when its size is known without I/O) matches
    /// its geometry.
    fn check_image_geometry(&self) -> Result<(), ConfigError> {
        for (drive_index, drive) in self.drives.iter().enumerate() {
            if let Some(actual) = drive.image.known_len() {
                let expected = drive.kind.image_bytes();
                if actual != expected {
                    return Err(ConfigError::ImageGeometryMismatch {
                        drive_index,
                        expected,
                        actual,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Locate a device by role in a config being mutated by a test.
#[cfg(test)]
fn find_device(
    config: &mut MachineConfig,
    id: crate::device::DeviceId,
) -> &mut crate::device::DeviceConfig {
    config
        .devices
        .iter_mut()
        .find(|d| d.id == id)
        .expect("device present")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ClockConfig;
    use crate::cpu::{CpuChip, CpuConfig, ResetSpec, SegmentSeed};
    use crate::device::{
        CmosCfg, DeviceConfig, DeviceId, DmaCfg, DmaSpec, FdcCfg, I8042Cfg, PicCfg, PitCfg,
        PortWindow, PostPortCfg, SysControlCfg, UartCfg, VgaTextCfg,
    };
    use crate::drive::{DriveConfig, DriveKind, FloppyGeometry};
    use crate::image::{ImageKind, ImageSource, LoadImage};
    use crate::memory::{MemoryConfig, MmioWindow, RamRegion};
    use crate::output::OutputConfig;

    const SPACE: u64 = 0x10_0000;
    const TEXT_BASE: u64 = 0xB_8000;
    const TEXT_LEN: u64 = 0x8000;
    const ROM_BASE: u64 = 0xF_8000;
    const ROM_LEN: u64 = 0x8000;

    fn dev(id: DeviceId, kind: DeviceKind, ports: Vec<PortWindow>) -> DeviceConfig {
        DeviceConfig::new(id, kind, ports)
    }

    /// A complete, internally consistent AT-class machine used as the baseline;
    /// each malformed-config test mutates one aspect of a clone.
    fn valid_config() -> MachineConfig {
        let devices = vec![
            dev(
                DeviceId::Uart,
                DeviceKind::Uart(UartCfg { base: 0x3F8 }),
                vec![PortWindow::new(0x3F8, 0x3FF)],
            ),
            dev(
                DeviceId::Pit,
                DeviceKind::Pit(PitCfg { base: 0x40 }),
                vec![PortWindow::new(0x40, 0x43)],
            )
            .with_irq(0),
            dev(
                DeviceId::MasterPic,
                DeviceKind::Pic(PicCfg {
                    command: 0x20,
                    cascade: true,
                }),
                vec![PortWindow::new(0x20, 0x21)],
            ),
            dev(
                DeviceId::SlavePic,
                DeviceKind::Pic(PicCfg {
                    command: 0xA0,
                    cascade: false,
                }),
                vec![PortWindow::new(0xA0, 0xA1)],
            ),
            dev(
                DeviceId::Keyboard,
                DeviceKind::Keyboard(I8042Cfg {
                    data: 0x60,
                    command: 0x64,
                }),
                vec![PortWindow::single(0x60), PortWindow::single(0x64)],
            )
            .with_irq(1),
            dev(
                DeviceId::Rtc,
                DeviceKind::Rtc(CmosCfg {
                    index: 0x70,
                    data: 0x71,
                    seed: true,
                    timing: None,
                }),
                vec![PortWindow::new(0x70, 0x71)],
            )
            .with_irq(8),
            dev(
                DeviceId::Vga,
                DeviceKind::Vga(VgaTextCfg { crtc_index: 0x3D4 }),
                vec![PortWindow::new(0x3D4, 0x3D5)],
            ),
            dev(
                DeviceId::SysControl,
                DeviceKind::SysControl(SysControlCfg { port: 0x61 }),
                vec![PortWindow::single(0x61)],
            ),
            dev(
                DeviceId::Post,
                DeviceKind::Post(PostPortCfg { port: 0x190 }),
                vec![PortWindow::single(0x190)],
            ),
            dev(
                DeviceId::Fdc,
                DeviceKind::Fdc(FdcCfg {
                    base: 0x3F0,
                    drive_slots: 2,
                }),
                vec![PortWindow::new(0x3F0, 0x3F7)],
            )
            .with_irq(6)
            .with_dma(DmaSpec {
                controller: DeviceId::Dma,
                channel: 2,
            }),
            dev(
                DeviceId::Dma,
                DeviceKind::Dma(DmaCfg { base: 0x00 }),
                vec![PortWindow::new(0x00, 0x0F), PortWindow::new(0x81, 0x8F)],
            ),
        ];

        let memory = MemoryConfig {
            address_space_bytes: SPACE,
            ram: vec![
                RamRegion::new(0, TEXT_BASE),
                RamRegion::new(TEXT_BASE + TEXT_LEN, ROM_BASE - (TEXT_BASE + TEXT_LEN)),
            ],
            mmio: vec![MmioWindow::new(TEXT_BASE, TEXT_LEN, DeviceId::Vga)],
        };

        let images = vec![LoadImage {
            base: ROM_BASE,
            source: ImageSource::Inline(vec![0u8; ROM_LEN as usize]),
            kind: ImageKind::Rom,
        }];

        let drives = vec![DriveConfig {
            kind: DriveKind::Floppy {
                geometry: FloppyGeometry::HD_1_44,
            },
            image: ImageSource::Blank {
                size: FloppyGeometry::HD_1_44.image_bytes(),
            },
            controller_slot: 0,
            write_protect: false,
        }];

        MachineConfig {
            cpu: CpuConfig {
                chip: CpuChip::I80286,
                reset: ResetSpec::reset_vector(),
                segments: SegmentSeed::flat(0xF000),
            },
            clock: ClockConfig::default(),
            memory,
            devices,
            drives,
            images,
            output: OutputConfig::default(),
        }
    }

    #[test]
    fn valid_config_passes() {
        assert_eq!(valid_config().validate(), Ok(()));
    }

    #[test]
    fn zero_cpu_rate_is_rejected() {
        for (numerator, denominator) in [(0, 1), (1, 0), (0, 0)] {
            let mut config = valid_config();
            config.clock.retired_per_pit_clock = numerator;
            config.clock.retired_per_pit_clock_denominator = denominator;
            assert_eq!(
                config.validate(),
                Err(ConfigError::InvalidCpuRate {
                    numerator,
                    denominator,
                })
            );
            assert_eq!(
                config.validate().expect_err("invalid CPU rate").to_string(),
                format!("cpu rate must have a positive numerator and denominator (got {numerator}/{denominator})")
            );
        }
    }

    #[test]
    fn positive_cpu_rates_are_valid() {
        for (numerator, denominator) in [(1, 1), (2, 1), (1, 3), (3, 2), (u64::MAX, u64::MAX)] {
            let mut config = valid_config();
            config.clock.retired_per_pit_clock = numerator;
            config.clock.retired_per_pit_clock_denominator = denominator;
            assert_eq!(config.validate(), Ok(()));
        }
    }

    #[test]
    fn an_unbounded_or_zero_quantum_is_rejected() {
        for quantum in [None, Some(0)] {
            let mut config = valid_config();
            config.clock.quantum = quantum;
            assert_eq!(config.validate(), Err(ConfigError::InvalidQuantum(quantum)));
            assert_eq!(
                config.validate().expect_err("invalid quantum").to_string(),
                format!("clock quantum must be a positive finite block budget (got {quantum:?})")
            );
        }
    }

    #[test]
    fn a_positive_finite_quantum_passes() {
        let mut config = valid_config();
        config.clock.quantum = Some(1);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn the_four_standard_floppy_geometries_have_the_expected_sizes() {
        assert_eq!(FloppyGeometry::HD_1_44.image_bytes(), 1_474_560);
        assert_eq!(FloppyGeometry::HD_1_2.image_bytes(), 1_228_800);
        assert_eq!(FloppyGeometry::DD_720K.image_bytes(), 737_280);
        assert_eq!(FloppyGeometry::DD_360K.image_bytes(), 368_640);
    }

    #[test]
    fn duplicate_device_id_is_rejected() {
        let mut config = valid_config();
        config.devices.push(dev(
            DeviceId::Uart,
            DeviceKind::Uart(UartCfg { base: 0x2F8 }),
            vec![PortWindow::new(0x2F8, 0x2FF)],
        ));
        assert_eq!(
            config.validate(),
            Err(ConfigError::DuplicateDeviceId(DeviceId::Uart))
        );
    }

    #[test]
    fn port_overlap_is_rejected() {
        let mut config = valid_config();
        // Give the UART a stray window over the PIT's 0x40 base.
        find_device(&mut config, DeviceId::Uart)
            .ports
            .push(PortWindow::single(0x40));
        assert_eq!(
            config.validate(),
            Err(ConfigError::PortOverlap {
                a: DeviceId::Uart,
                b: DeviceId::Pit,
                port: 0x40,
            })
        );
    }

    #[test]
    fn an_explicitly_allowed_port_overlap_passes() {
        let mut config = valid_config();
        find_device(&mut config, DeviceId::Uart)
            .ports
            .push(PortWindow::overlapping(0x40, 0x40));
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn a_device_level_overlap_allowance_passes() {
        let mut config = valid_config();
        {
            let uart = find_device(&mut config, DeviceId::Uart);
            uart.ports.push(PortWindow::single(0x40));
            uart.allow_port_overlap = true;
        }
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn mmio_window_to_absent_device_is_rejected() {
        let mut config = valid_config();
        config.devices.retain(|d| d.id != DeviceId::Vga);
        assert_eq!(
            config.validate(),
            Err(ConfigError::MmioDeviceMissing(DeviceId::Vga))
        );
    }

    #[test]
    fn overlapping_mmio_windows_are_rejected() {
        let mut config = valid_config();
        config
            .memory
            .mmio
            .push(MmioWindow::new(TEXT_BASE, TEXT_LEN, DeviceId::Vga));
        assert_eq!(
            config.validate(),
            Err(ConfigError::MmioOverlap {
                a: DeviceId::Vga,
                b: DeviceId::Vga,
                addr: TEXT_BASE,
            })
        );
    }

    #[test]
    fn mmio_over_ram_is_rejected() {
        let mut config = valid_config();
        // Extend the low RAM region over the trapped text buffer.
        config.memory.ram[0] = RamRegion::new(0, TEXT_BASE + TEXT_LEN);
        assert_eq!(
            config.validate(),
            Err(ConfigError::MmioOverlapsRam {
                device: DeviceId::Vga,
                addr: TEXT_BASE,
            })
        );
    }

    #[test]
    fn a_region_outside_the_address_space_is_rejected() {
        let mut config = valid_config();
        config.memory.ram.push(RamRegion::new(0xF_FF00, 0x1000));
        assert_eq!(
            config.validate(),
            Err(ConfigError::RegionOutOfSpace {
                base: 0xF_FF00,
                end: 0x10_0F00,
                space: SPACE,
            })
        );
    }

    #[test]
    fn an_image_running_past_the_address_space_is_rejected() {
        let mut config = valid_config();
        // A ROM one byte too long overruns the top of the 1 MiB space.
        config.images[0].source = ImageSource::Inline(vec![0u8; ROM_LEN as usize + 1]);
        assert_eq!(
            config.validate(),
            Err(ConfigError::RegionOutOfSpace {
                base: ROM_BASE,
                end: SPACE + 1,
                space: SPACE,
            })
        );
    }

    #[test]
    fn an_irq_line_conflict_is_rejected() {
        let mut config = valid_config();
        // Wire the VGA onto IRQ0, already the PIT's line.
        find_device(&mut config, DeviceId::Vga).irq = Some(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(
            err,
            ConfigError::IrqConflict {
                line: 0,
                a: DeviceId::Pit,
                b: DeviceId::Vga,
            }
        ));
    }

    #[test]
    fn dma_wiring_to_an_absent_controller_is_rejected() {
        let mut config = valid_config();
        config.devices.retain(|d| d.id != DeviceId::Dma);
        assert_eq!(
            config.validate(),
            Err(ConfigError::DmaControllerMissing {
                requester: DeviceId::Fdc,
                controller: DeviceId::Dma,
            })
        );
    }

    #[test]
    fn a_drive_on_a_nonexistent_controller_slot_is_rejected() {
        let mut config = valid_config();
        config.drives[0].controller_slot = 5; // the FDC offers only slots 0 and 1
        assert_eq!(
            config.validate(),
            Err(ConfigError::MissingController {
                drive_index: 0,
                slot: 5,
                available: 2,
            })
        );
    }

    #[test]
    fn a_drive_with_no_controller_present_is_rejected() {
        let mut config = valid_config();
        // Drop the FDC and its DMA wiring source; the floppy now has no
        // controller at all (zero available slots).
        config
            .devices
            .retain(|d| d.id != DeviceId::Fdc && d.id != DeviceId::Dma);
        assert_eq!(
            config.validate(),
            Err(ConfigError::MissingController {
                drive_index: 0,
                slot: 0,
                available: 0,
            })
        );
    }

    #[test]
    fn a_floppy_image_of_the_wrong_size_is_rejected() {
        let mut config = valid_config();
        config.drives[0].image = ImageSource::Blank { size: 1000 };
        assert_eq!(
            config.validate(),
            Err(ConfigError::ImageGeometryMismatch {
                drive_index: 0,
                expected: 1_474_560,
                actual: 1000,
            })
        );
    }

    #[test]
    fn a_file_backed_image_skips_the_geometry_check() {
        let mut config = valid_config();
        // A file's size is unknown without I/O, so geometry cannot be checked
        // here — validation must not fail on it.
        config.drives[0].image = ImageSource::File("disk.img".into());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn segment_seed_converts_to_the_cpu_core_type() {
        let seed = SegmentSeed::flat(0x1234);
        let real: cpu::x86jit_backend::RealSegments = seed.into();
        assert_eq!(real.ds, 0x1234);
        assert_eq!(real.ss, 0x1234);
        assert_eq!(real.sp, seed.sp);
    }
}
