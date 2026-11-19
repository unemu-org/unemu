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

    /// The AT boot preset with a protected-mode guest whose last instruction
    /// the CPU core refuses: it enters protected mode with LMSW, loads SS, DS
    /// and TR (a 286 TSS at 7E00h with ring stacks 0038:0F00, 0030:5000 and
    /// 0030:4000), returns to ring 3 with RETF on the stack 0033:0400, then
    /// makes a far CALL at 002B:7C2B through the ring-3 call gate 0023h to
    /// 0008:7C23, copying 31 words. The GDT at 7D00h holds ring-0 code 08h and
    /// data 10h, the TSS 18h, the gate 20h, ring-3 code 28h, and ring-3 data
    /// 30h and ring-0 data 38h at the colour text buffer B8000h, which the
    /// card traps. With both stacks on a device, the core cannot record the
    /// frame copy and refuses the call before it touches either stack. For the
    /// tests of the CPU stop report.
    pub fn refused_gate_call() -> crate::MachineConfig {
        let mut blob = vec![
            0xFA, // CLI
            0x0F, 0x01, 0x16, 0xF0, 0x7C, // LGDT [7CF0h]
            0xB8, 0x01, 0x00, 0x0F, 0x01, 0xF0, // MOV AX,1; LMSW AX
            0xEA, 0x11, 0x7C, 0x08, 0x00, // JMP 0008:7C11
            0xB8, 0x10, 0x00, 0x8E, 0xD0, 0x8E, 0xD8, // MOV AX,10h; MOV SS,AX; MOV DS,AX
            0xB8, 0x18, 0x00, 0x0F, 0x00, 0xD8, // MOV AX,18h; LTR AX
            0x68, 0x33, 0x00, // PUSH 33h (ring-3 SS)
            0x68, 0x00, 0x04, // PUSH 0400h (ring-3 SP)
            0x68, 0x2B, 0x00, // PUSH 2Bh (ring-3 CS)
            0x68, 0x2B, 0x7C, // PUSH 7C2Bh (ring-3 IP)
            0xCB, // RETF
            0x9A, 0x00, 0x00, 0x23, 0x00, // CALL FAR 0023:0000
            0xF4, // HLT
        ];
        blob.resize(0x210, 0);
        blob[0xF0..0xF6].copy_from_slice(&[0x3F, 0x00, 0x00, 0x7D, 0x00, 0x00]);
        let gdt: [[u8; 8]; 7] = [
            [0xFF, 0xFF, 0, 0, 0, 0x9A, 0, 0],
            [0xFF, 0xFF, 0, 0, 0, 0x92, 0, 0],
            [0x2B, 0, 0x00, 0x7E, 0, 0x81, 0, 0],
            [0x23, 0x7C, 0x08, 0, 0x1F, 0xE4, 0, 0],
            [0xFF, 0xFF, 0, 0, 0, 0xFA, 0, 0],
            [0xFF, 0x7F, 0x00, 0x80, 0x0B, 0xF2, 0, 0],
            [0xFF, 0x7F, 0x00, 0x80, 0x0B, 0x92, 0, 0],
        ];
        for (index, descriptor) in gdt.iter().enumerate() {
            let at = 0x108 + 8 * index;
            blob[at..at + 8].copy_from_slice(descriptor);
        }
        blob[0x200..0x20E].copy_from_slice(&[
            0, 0, 0x00, 0x0F, 0x38, 0, 0x00, 0x50, 0x30, 0, 0x00, 0x40, 0x30, 0,
        ]);
        let mut config = crate::at_boot(blob);
        let text = devices::vga_text::TEXT_BUFFER_BASE;
        let length = devices::vga_text::TEXT_BUFFER_LEN as u64;
        config.memory = crate::MemoryConfig {
            address_space_bytes: text + length,
            ram: vec![crate::RamRegion::new(0, text)],
            mmio: vec![crate::MmioWindow::new(text, length, crate::DeviceId::Vga)],
        };
        config
    }

    /// A 32 KiB system ROM for F8000h whose guest gets stuck delivering
    /// exceptions: from the reset vector it enters protected mode with a GDT
    /// (ring-0 code 08h over the ROM, data 10h over the first 64 KiB of RAM)
    /// and an IDT whose 14 exception gates all lead to 0008:0040, loads SS
    /// with SP F000h, then loads DS with selector F8h, beyond the GDT limit:
    /// #GP(00F8h). The handler's first instruction loads DS from the same AX,
    /// so every delivery faults again and no instruction retires. On an
    /// 80286 each delivery pushes four words; from SP F000h, a multiple of
    /// the 8-byte frame, the frames wrap within the 64 KiB segment without
    /// a fault, so the loop never ends (Intel 80286 Programmer's Reference
    /// Manual, §9.5 and the MOV and PUSH descriptions). For the tests of the
    /// delivery-stall stop; [`delivery_overrun_rom`] is the case that shuts
    /// down.
    ///
    /// The bytes are this NASM listing (`nasm -f bin`, org 0, ROM = F8000h):
    ///
    /// ```text
    /// 0000 FA               cli
    /// 0001 2E0F01160001     lgdt [cs:0100h]
    /// 0007 2E0F011E0601     lidt [cs:0106h]
    /// 000D B80100           mov ax, 1
    /// 0010 0F01F0           lmsw ax
    /// 0013 EA18000800       jmp 0008:0018h
    /// 0018 B81000           mov ax, 10h
    /// 001B 8ED0             mov ss, ax
    /// 001D BC00F0           mov sp, 0F000h
    /// 0020 B8F800           mov ax, 0F8h
    /// 0023 8ED8             mov ds, ax          ; #GP(00F8h)
    /// 0025 F4               hlt
    /// 0040 8ED8             mov ds, ax          ; the handler: #GP again
    /// 0042 F4               hlt
    /// 0100 1700 1081 0F00   GDTR: limit 17h, base 0F8110h
    /// 0106 6F00 8081 0F00   IDTR: limit 6Fh, base 0F8180h
    /// 0110 00 x 8           null descriptor
    /// 0118 FFFF00800F9B0000 08h: code, base 0F8000h, limit FFFFh, accessed
    /// 0120 FFFF000000930000 10h: data, base 0, limit FFFFh, accessed
    /// 0180 4000080000860000 x 14: 286 interrupt gates to 0008:0040h
    /// 7FF0 EA0000 00F8      jmp F800:0000h      ; the reset vector
    /// ```
    pub fn delivery_stall_rom() -> Vec<u8> {
        let mut rom = vec![0; 0x8000];
        let code: &[u8] = &[
            0xFA, // CLI
            0x2E, 0x0F, 0x01, 0x16, 0x00, 0x01, // LGDT CS:[0100h]
            0x2E, 0x0F, 0x01, 0x1E, 0x06, 0x01, // LIDT CS:[0106h]
            0xB8, 0x01, 0x00, 0x0F, 0x01, 0xF0, // MOV AX,1; LMSW AX
            0xEA, 0x18, 0x00, 0x08, 0x00, // JMP 0008:0018
            0xB8, 0x10, 0x00, 0x8E, 0xD0, // MOV AX,10h; MOV SS,AX
            0xBC, 0x00, 0xF0, // MOV SP,0F000h
            0xB8, 0xF8, 0x00, 0x8E, 0xD8, // MOV AX,0F8h; MOV DS,AX
            0xF4, // HLT
        ];
        rom[..code.len()].copy_from_slice(code);
        rom[0x40..0x43].copy_from_slice(&[0x8E, 0xD8, 0xF4]); // MOV DS,AX; HLT
        rom[0x100..0x10C].copy_from_slice(&[
            0x17, 0x00, 0x10, 0x81, 0x0F, 0x00, 0x6F, 0x00, 0x80, 0x81, 0x0F, 0x00,
        ]);
        rom[0x118..0x120].copy_from_slice(&[0xFF, 0xFF, 0x00, 0x80, 0x0F, 0x9B, 0, 0]);
        rom[0x120..0x128].copy_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x93, 0, 0]);
        for vector in 0..14 {
            let at = 0x180 + 8 * vector;
            rom[at..at + 8].copy_from_slice(&[0x40, 0x00, 0x08, 0x00, 0x00, 0x86, 0, 0]);
        }
        rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0x00, 0x00, 0x00, 0xF8]);
        rom
    }

    /// The AT BIOS preset with [`delivery_stall_rom`] as its system ROM and
    /// no floppy controller.
    pub fn delivery_stall() -> crate::MachineConfig {
        crate::bios(delivery_stall_rom(), None)
    }

    /// A 32 KiB system ROM for F8000h whose real-mode guest faults at its
    /// #UD handler's first instruction on a stack that overruns: from the
    /// reset vector it counts its starts in the byte at 0000:0500h and halts
    /// at the second. On the first it points INT 6 at F800:0040h, sets
    /// SS:SP to 1000:FFFEh and executes 0F FF, an invalid opcode, as is the
    /// handler. Each delivery pushes a 6-byte frame and retires nothing;
    /// FFFEh is not a multiple of 6, so after 10,922 frames the next one
    /// crosses the bottom of the stack, its fault escalates to a double
    /// fault and then to shutdown, and the AT board resets the processor
    /// (Intel 80286 Programmer's Reference Manual, §9.6.2 and Appendix B,
    /// INT; IBM Personal Computer AT Technical Reference, System Board,
    /// shutdown decoding). RAM survives the CPU reset, so the second start
    /// halts. The bytes are this NASM listing (`nasm -f bin`, org 0):
    ///
    /// ```text
    /// 0000 FA               cli
    /// 0001 31C0             xor ax, ax
    /// 0003 8ED8             mov ds, ax
    /// 0005 FE060005         inc byte [0500h]
    /// 0009 803E000502       cmp byte [0500h], 2
    /// 000E 7416             je 0026h
    /// 0010 C70618004000     mov word [0018h], 0040h   ; INT 6 offset
    /// 0016 C7061A0000F8     mov word [001Ah], 0F800h  ; INT 6 segment
    /// 001C B80010           mov ax, 1000h
    /// 001F 8ED0             mov ss, ax
    /// 0021 BCFEFF           mov sp, 0FFFEh
    /// 0024 0FFF             (invalid opcode: #UD)
    /// 0026 F4               hlt
    /// 0040 0FFF             the handler: #UD again
    /// 7FF0 EA000000F8       jmp F800:0000h            ; the reset vector
    /// ```
    pub fn delivery_overrun_rom() -> Vec<u8> {
        let mut rom = vec![0; 0x8000];
        let code: &[u8] = &[
            0xFA, // CLI
            0x31, 0xC0, 0x8E, 0xD8, // XOR AX,AX; MOV DS,AX
            0xFE, 0x06, 0x00, 0x05, // INC BYTE [0500h]
            0x80, 0x3E, 0x00, 0x05, 0x02, // CMP BYTE [0500h],2
            0x74, 0x16, // JE 0026h
            0xC7, 0x06, 0x18, 0x00, 0x40, 0x00, // MOV WORD [0018h],0040h
            0xC7, 0x06, 0x1A, 0x00, 0x00, 0xF8, // MOV WORD [001Ah],0F800h
            0xB8, 0x00, 0x10, 0x8E, 0xD0, // MOV AX,1000h; MOV SS,AX
            0xBC, 0xFE, 0xFF, // MOV SP,0FFFEh
            0x0F, 0xFF, // #UD
            0xF4, // HLT
        ];
        rom[..code.len()].copy_from_slice(code);
        rom[0x40..0x42].copy_from_slice(&[0x0F, 0xFF]);
        rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0x00, 0x00, 0x00, 0xF8]);
        rom
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
    check_system_bios, AssemblyOptions, CmosLayout, FirmwareBase, FirmwareCapability, FirmwareCpu,
    FirmwareDistribution, FirmwareEntry, FirmwareKind, FirmwarePart, FirmwareStatus, KnownPart,
    Supersession, UnlistedFirmware, KISELEV_XI8088,
};
