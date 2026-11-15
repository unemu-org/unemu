// SPDX-License-Identifier: AGPL-3.0-or-later
//! Declarative machine configuration + validation (§3.1).
//!
//! A [`MachineConfig`] is a **pure-data description** of one emulated computer:
//! its CPU, virtual clock, memory map, device set, drives, and loadable images.
//! It names *what* the machine is made of, never *how* to wire it up.
//! Turning this description into a running machine (a [`bus::Bus`] with a
//! [`bus::DecodeMap`], the [`::machine::IrqLine`] / [`::machine::DmaWiring`] tables,
//! and a [`::machine::Machine`]) is the job of [`assemble`], the single data-driven
//! realiser in this crate. Keeping the description separate from the assembly step
//! is what makes a machine reproducible and snapshot-comparable: the whole config
//! derives [`PartialEq`]/[`Eq`], so two configs — or a config and a golden — can
//! be compared byte-for-byte.
//!
//! ## What is modelled
//!
//! - [`CpuConfig`] — the modelled chip, the reset entry point, and the seeded
//!   real-mode segment registers ([`::cpu::x86jit_backend::RealSegments`]).
//! - [`ClockConfig`] — the deterministic virtual-clock ratio.
//! - [`MemoryConfig`] — the RAM regions and MMIO windows of the address space.
//! - [`DeviceConfig`] — one entry per modelled chip ([`devices`]), carrying its
//!   role ([`DeviceId`]), decoded port windows, IRQ line, and DMA wiring.
//! - [`LoadImage`] — the single home for any blob loaded into the address space
//!   (a masked BIOS ROM, a RAM-loaded boot blob).
//! - [`DriveConfig`] — the removable/fixed media attached to a disk controller.
//!
//! [`ObservationPolicy`] carries [`OutputConfig`], the initial presentation
//! subscriber selection, separately from the hardware description.
//!
//! ## Validation
//!
//! [`MachineConfig::validate`] catches the ways a description can be internally
//! inconsistent before any hardware is built — overlapping decodes, an MMIO
//! window colliding with RAM (which the CPU core forbids), a region outside the
//! address space, two devices contending for one IRQ line, a drive bound to a
//! controller slot that no controller offers, and an image whose size does not
//! match its geometry. Every failure is a distinct [`ConfigError`], and the
//! validators are unit-tested here without assembling a machine.

mod assemble;
mod board;
pub mod capabilities;
mod clock;
mod cpu;
pub mod descriptors;
mod device;
mod drive;
mod ems_frame;
mod error;
mod firmware;
pub mod firmware_discovery;
mod floppy;
mod fw_cfg;
mod image;
mod machine;
mod memory;
mod observation;
mod output;
mod persistence;
mod presets;
mod run;
mod sb16_audio;
mod speaker_trace;
mod validate;

pub use assemble::{
    assemble, assemble_with_observation, assemble_with_observation_and_options,
    assemble_with_options, assemble_with_verified_bases, derive_cmos_config, derive_cmos_layout,
    AssembledMachine, AssemblyError,
};
pub use board::{BoardModel, DisplaySwitch, MachineClass, PrimaryDisplay};
pub use bus::PortWidth;
pub use clock::ClockConfig;
pub use cpu::{CpuChip, CpuConfig, FpuChip, ResetSpec, SegmentSeed};
pub use device::{
    CmosCfg, CmosRam, DebugConsoleCfg, DeviceConfig, DeviceId, DeviceKind, DmaCfg, DmaSpec, FdcCfg,
    FwCfgCfg, I8042Cfg, IdeCfg, LotechEmsCfg, MdaTextCfg, NvramSource, PicCfg, PitCfg, PortWindow,
    PostPortCfg, RtcStart, Sb16Cfg, SysControlCfg, UartCfg, VgaCfg, VgaTextCfg,
};
pub use devices::ata::AtaControllerModel;
pub use devices::FwCfgFile;
pub use drive::{
    DriveConfig, DriveKind, FloppyGeometry, FloppyMediaList, FloppyMedium, HardDiskGeometry,
    WriteMode,
};
pub use error::ConfigError;
pub use floppy::{FloppyAction, FloppyError, FloppyEvent};
pub use image::{
    interleave_parts, FirmwareService, ImageKind, ImagePart, ImageSource, Lane, LoadImage,
};
pub use machine::MachineConfig;
pub use memory::{MemoryConfig, MmioWindow, RamRegion, IBM5170_MAX_EXTENDED_KIB};
pub use observation::{
    Observation, ObservationError, ObservationPolicy, ObservationRequest, ObservationRoute,
    OutputKind,
};
pub use output::{AudioSinkKind, FrameSinkKind, OutputConfig};
pub use persistence::{PersistenceError, PersistenceHandle};
pub use presets::{
    at_boot, at_fdc_write, at_keyboard, at_rtc, at_vga_text, bios, floppy_boot, test386,
};
pub use run::{
    inject_mouse, inject_scancodes, keyboard_leds_if_ready, run, run_with_persistence,
    try_inject_scancode, ConsoleProbe, InjectionSchedule, KeyboardStimulus, RunError, RunPolicy,
    StopCondition,
};
pub use speaker_trace::{GuestStamp, ResetCause, ResetSource, SpeakerTraceEvent};

/// Shared host-side fixtures for image, diagnostic and persistence tests.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    pub use ::cpu::{Cpu, CpuStop, DmaAccess};
    mod user_firmware;
    pub use user_firmware::{
        find_user_firmware, required_user_firmware, FoundUserFirmware, UserFirmwareError,
        UserFirmwareLayout,
    };

    /// Record successful CPU mappings as (base, exclusive end, kind), where
    /// kind is ram/mmio/rom/empty-bus.
    /// Builds the CPU through the production mapper without running guest code.
    pub fn cpu_mapping_regions(
        config: &crate::MachineConfig,
        options: crate::AssemblyOptions,
    ) -> Result<Vec<(u64, u64, &'static str)>, crate::AssemblyError> {
        crate::assemble::cpu_mapping_regions(config, options)
    }

    /// Build a validated physical map for host inspection and byte DMA tests.
    /// Host `write_mem` is a loader and can overwrite ROM; use the `Cpu` DMA
    /// seam when checking guest immutability at addresses outside real mode.
    pub fn mapped_cpu(
        config: &crate::MachineConfig,
        options: crate::AssemblyOptions,
    ) -> Result<::cpu::X86JitCpu, crate::AssemblyError> {
        crate::assemble::mapped_cpu(config, options)
    }

    /// SHA-256 of visible row-major glyph/attribute pairs, excluding cursor and style.
    /// IBM Monochrome Display and Printer Adapter, Programming Considerations:
    /// each text cell occupies a glyph byte followed by an attribute byte.
    ///
    /// # Panics
    /// Panics if the supplied frame contains pixels instead of text cells.
    pub fn text_plane_sha256(frame: &video::Frame) -> String {
        use sha2::{Digest, Sha256};
        let video::Frame::Text(grid) = frame else {
            panic!("text_plane_sha256 requires a text frame, received pixels");
        };
        let mut digest = Sha256::new();
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                let cell = grid.cell(col, row).expect("cell in text grid");
                digest.update([cell.glyph, cell.attr]);
            }
        }
        format!("{:x}", digest.finalize())
    }

    /// SHA-256 for local binary fixtures, including non-sector-aligned drivers.
    pub fn fixture_sha256(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(bytes))
    }

    /// SHA-256 of a captured indexed frame: little-endian dimensions/repeat,
    /// row-major indices, then RGB palette bytes. No rendered text is included.
    /// IBM VGA/XGA Technical Reference, 256-Color Graphics Mode (13h).
    ///
    /// # Panics
    /// Panics if the frame contains text instead of indexed pixels.
    pub fn graphics_frame_sha256(frame: &video::Frame) -> String {
        use sha2::{Digest, Sha256};
        let video::Frame::Pixels(pixels) = frame else {
            panic!("graphics_frame_sha256 requires a pixel frame");
        };
        let mut digest = Sha256::new();
        digest.update(pixels.width.to_le_bytes());
        digest.update(pixels.height.to_le_bytes());
        digest.update([pixels.repeat.0, pixels.repeat.1]);
        digest.update(&pixels.indices);
        for colour in pixels.palette.iter() {
            digest.update(colour);
        }
        format!("{:x}", digest.finalize())
    }

    /// Exercise production image resolution without building a CPU or executing firmware.
    pub fn resolve_file_sources(
        config: &crate::MachineConfig,
        options: crate::AssemblyOptions,
    ) -> Result<crate::MachineConfig, crate::ConfigError> {
        crate::assemble::resolve_file_sources(config, options)
            .map(|(resolved, _)| resolved.into_owned())
    }

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A unique test directory removed on success and during panic unwinding.
    pub struct TempDir(PathBuf);

    impl TempDir {
        /// Create an empty directory with a process-local unique name.
        pub fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let path = std::env::temp_dir().join(format!(
                    "{label}-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("test directory: {error}"),
                }
            }
        }

        /// Borrow the directory for test inputs and output paths.
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::text_plane_sha256;

        #[test]
        fn text_plane_hash_uses_row_major_glyph_and_attribute_bytes() {
            let mut state = video::TextModeState {
                vram: &[b'A', 7, b'B', 0x1f, b'C', 0x70, b'D', 0x87],
                cols: 2,
                rows: 2,
                start_cell: 0,
                cursor_cell: 0,
                cursor_visible: false,
                attributes: video::TextAttributeSet::Colour {
                    blink_enabled: false,
                },
            };
            let hash = text_plane_sha256(&video::Frame::Text(video::render_text_mode(&state)));
            assert_eq!(
                hash,
                "7192f4315fc5be2195f757f77d25393c510d13863fc773c8a85043d3a2b8650e"
            );
            state.cursor_visible = true;
            state.cursor_cell = 3;
            state.attributes = video::TextAttributeSet::Colour {
                blink_enabled: true,
            };
            assert_eq!(
                text_plane_sha256(&video::Frame::Text(video::render_text_mode(&state))),
                hash
            );
        }

        #[test]
        #[should_panic(expected = "text_plane_sha256 requires a text frame, received pixels")]
        fn text_plane_hash_rejects_pixel_frames_explicitly() {
            let frame = video::Frame::Pixels(video::IndexedPixels {
                width: 1,
                height: 1,
                repeat: (1, 1),
                indices: vec![0],
                palette: vec![[0; 3]].into_boxed_slice(),
            });
            text_plane_sha256(&frame);
        }
    }
}
pub use firmware::{
    bios_display_switch, check_board_system_bios, check_firmware_parts, check_option_rom_catalogue,
    check_system_bios, AssemblyOptions, FirmwareBase, FirmwareCapability, FirmwareCpu,
    FirmwareDistribution, FirmwareEntry, FirmwareKind, FirmwarePart, FirmwareStatus, KnownPart,
    UnlistedFirmware, KISELEV_XI8088,
};
