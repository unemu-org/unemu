// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic AT rigs for the Lo-tech 2MB EMS Board schematic r02 page frame:
//! code fetched from mapped pages, remapping and stale code, unbacked and
//! duplicate latches, window-boundary operands, snapshots, ISA DMA in both
//! directions and run chunking. Guest code uses Intel 80286 Programmer's
//! Reference Manual Appendix B encodings.

use cpu::{Cpu, CpuError, CpuReg};
use device::TraceEvent;
use machine::{MachineError, RunOutcome};

use crate::{
    AssembledMachine, DeviceConfig, DeviceId, DeviceKind, ImageSource, LotechEmsCfg, MachineConfig,
    MmioWindow, ObservationPolicy,
};

const FRAME: u64 = 0xD0000;
const PAGE: usize = 0x4000;

const CX: u8 = 1;
const BX: u8 = 3;
const BP: u8 = 5;
const SI: u8 = 6;
const DI: u8 = 7;

/// `MOV AX, CS:[0]; ADD AX, 1230h; RETF`, as little-endian words. The routine
/// reads the first word of the page it runs from, so its result depends on
/// both the fetched code and the data around it.
const ROUTINE: [u16; 4] = [0xA12E, 0x0000, 0x3005, 0xCB12];

/// A tiny real-mode program builder. Every program starts with CLI, so its
/// final HLT ends the run at once.
struct Asm(Vec<u8>);

impl Asm {
    fn new() -> Self {
        Self(vec![0xFA, 0xFC])
    }

    /// `MOV DX, port; MOV AL, value; OUT DX, AL`.
    fn out(&mut self, port: u16, value: u8) -> &mut Self {
        self.0
            .extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
        self
    }

    /// Select a page for one of the four frame windows (port 260h + window).
    fn latch(&mut self, window: u16, value: u8) -> &mut Self {
        self.out(0x260 + window, value)
    }

    /// `MOV AX, segment; MOV ES, AX`.
    fn es(&mut self, segment: u16) -> &mut Self {
        self.0.push(0xB8);
        self.0.extend(segment.to_le_bytes());
        self.0.extend([0x8E, 0xC0]);
        self
    }

    /// Point ES at the D000h frame.
    fn frame_es(&mut self) -> &mut Self {
        self.es(0xD000)
    }

    /// `MOV WORD ES:[offset], value`.
    fn store(&mut self, offset: u16, value: u16) -> &mut Self {
        self.0.extend([0x26, 0xC7, 0x06]);
        self.0.extend(offset.to_le_bytes());
        self.0.extend(value.to_le_bytes());
        self
    }

    /// `MOV reg, ES:[offset]`.
    fn load(&mut self, reg: u8, offset: u16) -> &mut Self {
        self.0.extend([0x26, 0x8B, (reg << 3) | 6]);
        self.0.extend(offset.to_le_bytes());
        self
    }

    /// Store [`ROUTINE`] at `ES:offset`.
    fn routine(&mut self, offset: u16) -> &mut Self {
        for (index, word) in ROUTINE.into_iter().enumerate() {
            self.store(offset + 2 * index as u16, word);
        }
        self
    }

    /// `CALL FAR segment:offset`.
    fn call_far(&mut self, segment: u16, offset: u16) -> &mut Self {
        self.0.push(0x9A);
        self.0.extend(offset.to_le_bytes());
        self.0.extend(segment.to_le_bytes());
        self
    }

    /// `MOV reg, AX`.
    fn keep_ax(&mut self, reg: u8) -> &mut Self {
        self.0.extend([0x89, 0xC0 | reg]);
        self
    }

    /// `MOV CX, FFFFh; LOOP $`: a long, interruptible delay for host stops.
    fn delay(&mut self) -> &mut Self {
        self.0.extend([0xB9, 0xFF, 0xFF, 0xE2, 0xFE]);
        self
    }

    fn halt(&mut self) -> Vec<u8> {
        self.0.push(0xF4);
        std::mem::take(&mut self.0)
    }
}

fn add_board(config: &mut MachineConfig, size_kib: u32) {
    add_board_at(config, 0xD000, size_kib);
}

fn add_board_at(config: &mut MachineConfig, frame: u16, size_kib: u32) {
    config.memory.address_space_bytes = 0x100000;
    let cfg = LotechEmsCfg {
        frame,
        size_kib,
        ..Default::default()
    };
    config.devices.push(DeviceConfig::new(
        DeviceId::LotechEms,
        DeviceKind::LotechEms(cfg),
        cfg.port_windows(),
    ));
    config.memory.mmio.push(MmioWindow::new(
        u64::from(frame) << 4,
        0x10000,
        DeviceId::LotechEms,
    ));
}

fn boot_config(program: Vec<u8>, size_kib: u32) -> MachineConfig {
    let mut config = crate::at_boot(program);
    add_board(&mut config, size_kib);
    config
}

fn rig(program: Vec<u8>, size_kib: u32) -> AssembledMachine {
    crate::assemble(&boot_config(program, size_kib)).unwrap()
}

fn halt_within(rig: &mut AssembledMachine, cap: u64) {
    assert_eq!(
        rig.machine.run_bounded(Some(cap)).unwrap(),
        RunOutcome::Halted
    );
}

fn word(rig: &AssembledMachine, reg: CpuReg) -> u64 {
    rig.machine.cpu().reg(reg) & 0xFFFF
}

/// The board snapshot: version, port, frame, four latches, clock, then SRAM.
fn board_state(rig: &AssembledMachine) -> Vec<u8> {
    let index = rig.device_index[&DeviceId::LotechEms];
    rig.machine.bus().device(index).unwrap().save()
}

fn sram_word(state: &[u8], page: usize, offset: usize) -> u16 {
    let at = 17 + page * PAGE + offset;
    u16::from_le_bytes([state[at], state[at + 1]])
}

fn routine_program() -> Vec<u8> {
    Asm::new()
        .latch(0, 5)
        .frame_es()
        .store(0, 0x1234)
        .routine(0x100)
        .call_far(0xD000, 0x100)
        .keep_ax(BX)
        .latch(0, 6)
        .store(0, 0x5678)
        .latch(0, 5)
        .call_far(0xD000, 0x100)
        .keep_ax(CX)
        .latch(0, 6)
        .load(SI, 0)
        // The same page now runs through window 1 at its own segment.
        .latch(1, 5)
        .call_far(0xD400, 0x100)
        .keep_ax(DI)
        .halt()
}

#[test]
fn routine_in_a_mapped_page_runs_and_survives_remapping() {
    let mut rig = rig(routine_program(), 2048);
    halt_within(&mut rig, 10_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0x2464);
    assert_eq!(word(&rig, CpuReg::Rcx), 0x2464);
    assert_eq!(word(&rig, CpuReg::Rsi), 0x5678);
    assert_eq!(word(&rig, CpuReg::Rdi), 0x2464);
    let state = board_state(&rig);
    assert_eq!(&state[5..9], &[6, 5, 0xFF, 0xFF]);
    assert_eq!(sram_word(&state, 5, 0), 0x1234);
    assert_eq!(sram_word(&state, 5, 0x100), ROUTINE[0]);
    assert_eq!(sram_word(&state, 6, 0), 0x5678);
}

#[test]
fn new_page_contents_replace_code_at_the_same_address() {
    // Different code at D000:0100 in pages 5 and 6, so code left over from the
    // previous page would return the wrong value.
    let program = Asm::new()
        .latch(0, 5)
        .frame_es()
        .store(0x100, 0x11B8)
        .store(0x102, 0xCB11)
        .latch(0, 6)
        .store(0x100, 0x22B8)
        .store(0x102, 0xCB22)
        .latch(0, 5)
        .call_far(0xD000, 0x100)
        .keep_ax(BX)
        // RAM to RAM: same region, new contents.
        .latch(0, 6)
        .call_far(0xD000, 0x100)
        .keep_ax(CX)
        // RAM to ROM and back: two retiles.
        .latch(0, 0xFF)
        .latch(0, 5)
        .call_far(0xD000, 0x100)
        .keep_ax(SI)
        // Shared and trapped, then RAM again for both windows.
        .latch(1, 5)
        .latch(0, 6)
        .call_far(0xD000, 0x100)
        .keep_ax(DI)
        .call_far(0xD400, 0x100)
        .keep_ax(BP)
        .halt();
    let mut rig = rig(program, 2048);
    halt_within(&mut rig, 10_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0x1111);
    assert_eq!(word(&rig, CpuReg::Rcx), 0x2222);
    assert_eq!(word(&rig, CpuReg::Rsi), 0x1111);
    assert_eq!(word(&rig, CpuReg::Rdi), 0x2222);
    assert_eq!(word(&rig, CpuReg::Rbp), 0x1111);
}

#[test]
fn unbacked_latches_read_ff_and_drop_writes() {
    // 512 KiB populates pages 0-31; page 32 selects an empty chip socket.
    let program = Asm::new()
        .latch(0, 5)
        .frame_es()
        .store(0, 0x1111)
        .latch(0, 0x80)
        .store(0, 0x2222)
        .load(BX, 0)
        .latch(0, 32)
        .store(0, 0x3333)
        .load(CX, 0)
        // Windows 0 and 1 are both unbacked: one operand spans their latches.
        .load(SI, 0x3FFF)
        .latch(0, 5)
        .load(DI, 0)
        .halt();
    let mut rig = rig(program, 512);
    halt_within(&mut rig, 10_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0xFFFF);
    assert_eq!(word(&rig, CpuReg::Rcx), 0xFFFF);
    assert_eq!(word(&rig, CpuReg::Rsi), 0xFFFF);
    assert_eq!(word(&rig, CpuReg::Rdi), 0x1111);
    let state = board_state(&rig);
    assert_eq!(sram_word(&state, 5, 0), 0x1111);
    assert_eq!(state[17..].iter().filter(|&&byte| byte != 0).count(), 2);
    // ISA DMA follows the same map: CPU RAM for the held page, read-only FFh
    // for the unbacked window.
    let cpu = rig.machine.cpu();
    assert_eq!(
        cpu.guest_dma_read(FRAME as u32),
        cpu::DmaAccess::Complete(0x11)
    );
    assert_eq!(
        cpu.guest_dma_read(FRAME as u32 + 0x4000),
        cpu::DmaAccess::Complete(0xFF)
    );
}

#[test]
fn duplicate_latches_share_one_coherent_page() {
    let program = Asm::new()
        .latch(0, 7)
        .frame_es()
        .store(0, 0x1111)
        // Both windows now select page 7.
        .latch(1, 7)
        .load(BX, 0x4000)
        .store(0x4002, 0x2222)
        .load(CX, 0x0002)
        // Window 0 holds page 7 alone again.
        .latch(1, 8)
        .load(SI, 0x0002)
        .store(0x0004, 0x3333)
        .latch(0, 9)
        .latch(1, 7)
        .load(DI, 0x4004)
        .halt();
    let mut rig = rig(program, 2048);
    halt_within(&mut rig, 10_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0x1111);
    assert_eq!(word(&rig, CpuReg::Rcx), 0x2222);
    assert_eq!(word(&rig, CpuReg::Rsi), 0x2222);
    assert_eq!(word(&rig, CpuReg::Rdi), 0x3333);
}

#[test]
fn word_operands_and_code_straddle_ram_held_windows() {
    let program = Asm::new()
        .latch(0, 1)
        .latch(1, 2)
        .latch(2, 3)
        .frame_es()
        .store(0x3FFF, 0xBEEF)
        .load(BX, 0x3FFF)
        // MOV AX, 1234h; RETF, fetched across the window 1/2 boundary.
        .store(0x7FFE, 0x34B8)
        .store(0x8000, 0xCB12)
        .call_far(0xD000, 0x7FFE)
        .keep_ax(CX)
        .latch(1, 7)
        .latch(0, 2)
        .load(SI, 0)
        .latch(0, 1)
        .load(DI, 0x3FFE)
        .halt();
    let mut rig = rig(program, 2048);
    halt_within(&mut rig, 10_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0xBEEF);
    assert_eq!(word(&rig, CpuReg::Rcx), 0x1234);
    assert_eq!(word(&rig, CpuReg::Rsi), 0x00BE);
    assert_eq!(word(&rig, CpuReg::Rdi), 0xEF00);
}

#[test]
fn operands_and_code_join_a_held_window_and_an_unbacked_one() {
    // Window 1 keeps its power-on FFh latch.
    let program = Asm::new()
        .latch(0, 1)
        .frame_es()
        .store(0x3FFF, 0xBEEF)
        .load(BX, 0x3FFF)
        // JMP near at 3FFEh takes its high displacement byte from the
        // unbacked window: FFh lands at 3F01h, 00h would run on into FFh.
        .store(0x3FFE, 0x00E9)
        .store(0x3F01, 0x34B8)
        .store(0x3F03, 0xCB12)
        .call_far(0xD000, 0x3FFE)
        .keep_ax(CX)
        // The dropped high byte reached no page.
        .latch(1, 2)
        .load(SI, 0x4000)
        .halt();
    let mut rig = rig(program, 2048);
    halt_within(&mut rig, 10_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0xFFEF);
    assert_eq!(word(&rig, CpuReg::Rcx), 0x1234);
    assert_eq!(word(&rig, CpuReg::Rsi), 0);
}

#[test]
fn operand_from_a_held_window_into_a_shared_one_is_a_cpu_error() {
    // Known limit: the CPU core does not split one operand between a RAM
    // region and a trapped region, so the run stops instead.
    let program = Asm::new()
        .latch(0, 1)
        .latch(1, 2)
        .latch(2, 2)
        .frame_es()
        .store(0x3FFF, 0xBEEF)
        .halt();
    let mut rig = rig(program, 2048);
    let error = rig.machine.run_bounded(Some(10_000)).unwrap_err();
    assert!(
        matches!(
            error,
            MachineError::Cpu(CpuError::Unmapped { addr: 0xD3FFF })
        ),
        "{error}"
    );
    let cpu = rig.machine.cpu();
    assert_eq!(
        cpu.guest_dma_read(FRAME as u32),
        cpu::DmaAccess::Complete(0)
    );
    assert_eq!(
        cpu.guest_dma_read(FRAME as u32 + 0x4000),
        cpu::DmaAccess::Mmio
    );
}

#[test]
fn snapshots_capture_held_pages_and_restore_replaces_them() {
    let program = Asm::new()
        .latch(0, 5)
        .frame_es()
        .store(0, 0x1111)
        .routine(0x100)
        .latch(1, 6)
        .store(0x4000, 0x2222)
        .delay()
        .store(0, 0x9999)
        .store(0x4000, 0x8888)
        .delay()
        .load(BX, 0)
        .load(SI, 0x4000)
        .call_far(0xD000, 0x100)
        .halt();
    let mut rig = rig(program, 2048);
    let index = rig.device_index[&DeviceId::LotechEms];
    assert_eq!(
        rig.machine.run_bounded(Some(1_000)).unwrap(),
        RunOutcome::RetiredCapReached
    );
    let saved = board_state(&rig);
    assert_eq!(&saved[5..9], &[5, 6, 0xFF, 0xFF]);
    assert_eq!(sram_word(&saved, 5, 0), 0x1111);
    assert_eq!(sram_word(&saved, 5, 0x106), ROUTINE[3]);
    assert_eq!(sram_word(&saved, 6, 0), 0x2222);

    assert_eq!(
        rig.machine.run_bounded(Some(100_000)).unwrap(),
        RunOutcome::RetiredCapReached
    );
    let newer = board_state(&rig);
    assert_eq!(sram_word(&newer, 5, 0), 0x9999);
    assert_eq!(sram_word(&newer, 6, 0), 0x8888);

    rig.machine
        .bus_mut()
        .device_mut(index)
        .unwrap()
        .restore(&saved)
        .unwrap();
    halt_within(&mut rig, 1_000_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0x1111);
    assert_eq!(word(&rig, CpuReg::Rsi), 0x2222);
    assert_eq!(word(&rig, CpuReg::Rax), 0x2341);
    let restored = board_state(&rig);
    assert_eq!(&restored[5..9], &saved[5..9]);
    assert_eq!(&restored[17..], &saved[17..]);
}

#[test]
fn frame_beside_another_trapped_window_keeps_that_window_trapped() {
    // The colour text buffer ends at C0000h, where a C000h frame begins, so
    // assembly maps both as one trapped region.
    let program = Asm::new()
        .latch(0, 5)
        .es(0xC000)
        .store(0, 0x1234)
        .routine(0x100)
        .call_far(0xC000, 0x100)
        .keep_ax(BX)
        .es(0xB800)
        .store(0, 0x0741)
        // Windows 0 and 1 share page 5, so window 0 is trapped like the
        // colour buffer below it and one operand can span both devices.
        .latch(1, 5)
        .store(0x7FFF, 0xABCD)
        .halt();
    let mut config = crate::at_vga_text(program);
    add_board_at(&mut config, 0xC000, 2048);
    let mut rig = crate::assemble(&config).unwrap();
    halt_within(&mut rig, 10_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0x2464);
    rig.machine.bus_mut().settle();
    let index = rig.device_index[&DeviceId::Vga];
    let scanout = rig
        .machine
        .bus()
        .device(index)
        .unwrap()
        .text_scanout()
        .unwrap();
    assert_eq!(&scanout.vram[..2], &[0x41, 0x07]);
    assert_eq!(scanout.vram.last(), Some(&0xCD));
    assert_eq!(sram_word(&board_state(&rig), 5, 0), 0x12AB);
}

#[test]
fn neighbouring_windows_of_one_kind_share_a_cpu_region() {
    use super::{EmsFrameMap, Kind, Tile, Window};
    let board = devices::LotechEms::new(0x260, 0xD000, 2048).unwrap();
    let map = EmsFrameMap::new(board.frame_memory(), FRAME, (0xC8000, 0xE8000));
    let tile = |base, end, kind| Tile { base, end, kind };
    assert_eq!(
        map.tiles_for(&[
            Window::Held(0),
            Window::Held(1),
            Window::Unbacked,
            Window::Shared
        ]),
        [
            tile(0xC8000, 0xD0000, Kind::Trap),
            tile(0xD0000, 0xD8000, Kind::Ram),
            tile(0xD8000, 0xDC000, Kind::Rom),
            tile(0xDC000, 0xE8000, Kind::Trap),
        ]
    );
    assert_eq!(map.tiles_for(&[Window::Shared; 4]), map.tiles);
    let exact = EmsFrameMap::new(board.frame_memory(), FRAME, (FRAME, FRAME + 0x10000));
    assert_eq!(
        exact.tiles_for(&[Window::Unbacked; 4]),
        [tile(FRAME, FRAME + 0x10000, Kind::Rom)]
    );
}

/// NEC µPD765 READ DATA or WRITE DATA (N=2) of `sector` on cylinder 0, head
/// 0, through Intel 8237A channel 2 in single mode at physical
/// `page:address`. The seven result bytes are stored at ES:`result`.
fn floppy(asm: &mut Asm, write: bool, sector: u8, page: u8, address: u16, result: u16) {
    for (port, value) in [
        (0x0A, 6),
        (0x0C, 0),
        (0x0B, if write { 0x4A } else { 0x46 }),
        (4, address as u8),
        (4, (address >> 8) as u8),
        (0x81, page),
        (5, 0xFF),
        (5, 1),
        (0x0A, 2),
    ] {
        asm.out(port, value);
    }
    let command = if write { 5 } else { 6 };
    for value in [command, 0, 0, 0, sector, 2, sector, 0x1B, 0xFF] {
        asm.out(0x3F5, value);
    }
    // MOV DI, result; then seven IN AL, DX; STOSB.
    asm.0.extend([0xBF, result as u8, (result >> 8) as u8]);
    for _ in 0..7 {
        asm.0.extend([0xEC, 0xAA]);
    }
}

fn dma_config() -> MachineConfig {
    let mut asm = Asm::new();
    asm.latch(0, 3).out(0x3F2, 0x1C).out(0x3F7, 2);
    floppy(&mut asm, false, 1, 0x0D, 0x0000, 0x1010);
    floppy(&mut asm, false, 1, 0x0D, 0x4000, 0x1020);
    asm.frame_es()
        .call_far(0xD000, 0)
        .keep_ax(BX)
        .latch(0, 4)
        .latch(0, 3)
        .load(CX, 0x01FE)
        .load(SI, 0x4000)
        // A word only the CPU copy holds, then the page goes back to disk.
        .store(0x0010, 0xA55A)
        .es(0);
    floppy(&mut asm, true, 2, 0x0D, 0x0000, 0x1030);
    floppy(&mut asm, false, 2, 0x00, 0x2000, 0x1040);
    let program = asm.frame_es().latch(1, 5).load(DI, 0x4000).halt();
    let mut config = crate::at_fdc_write(program, Vec::new(), 0x600);
    add_board(&mut config, 2048);
    let mut disk = vec![0; config.drives[0].kind.image_bytes() as usize];
    // MOV AX, 4321h; RETF, then a recognisable tail.
    disk[..4].copy_from_slice(&[0xB8, 0x21, 0x43, 0xCB]);
    disk[510..512].copy_from_slice(&[0xC3, 0x5A]);
    config.drives[0].image = ImageSource::Inline(disk);
    config
}

#[test]
fn isa_dma_moves_bytes_between_the_disk_and_the_held_copy() {
    let mut rig = crate::assemble(&dma_config()).unwrap();
    halt_within(&mut rig, 20_000);
    assert_eq!(word(&rig, CpuReg::Rbx), 0x4321);
    assert_eq!(word(&rig, CpuReg::Rcx), 0x5AC3);
    assert_eq!(word(&rig, CpuReg::Rsi), 0xFFFF);
    assert_eq!(word(&rig, CpuReg::Rdi), 0);
    // Sector 2, written from the held window, read back into low RAM.
    let mut sector = vec![0; 512];
    rig.machine.cpu().read_mem(0x2000, &mut sector).unwrap();
    assert_eq!(&sector[..4], &[0xB8, 0x21, 0x43, 0xCB]);
    assert_eq!(&sector[0x10..0x12], &[0x5A, 0xA5]);
    assert_eq!(&sector[0x1FE..], &[0xC3, 0x5A]);
    let state = board_state(&rig);
    assert_eq!(sram_word(&state, 3, 0), 0x21B8);
    assert_eq!(sram_word(&state, 3, 0x10), 0xA55A);
    assert_eq!(sram_word(&state, 3, 0x1FE), 0x5AC3);
}

/// Everything a run leaves behind that a host could observe.
#[derive(Debug, PartialEq)]
struct Outcome {
    registers: Vec<u64>,
    code_segment: u64,
    retired: u64,
    virtual_time: u64,
    devices: Vec<(Vec<u8>, Vec<TraceEvent>, u64)>,
}

/// Run `config` to its halt in one run, or in runs capped every `step` units
/// of virtual time, and collect the [`Outcome`].
fn outcome(config: &MachineConfig, step: Option<u64>) -> Outcome {
    let policy = ObservationPolicy {
        trace: device::TracePolicy::Ring { capacity: 1 << 14 },
        ..Default::default()
    };
    let mut rig = crate::assemble_with_observation(config, &policy).unwrap();
    match step {
        None => halt_within(&mut rig, 100_000),
        Some(step) => {
            let mut cap = 0;
            loop {
                cap += step;
                assert!(cap <= 100_000, "the guest did not halt");
                match rig.machine.run_bounded(Some(cap)).unwrap() {
                    RunOutcome::Halted => break,
                    RunOutcome::RetiredCapReached => {}
                    other => panic!("unexpected stop {other:?}"),
                }
            }
        }
    }
    let cpu = rig.machine.cpu();
    let registers = [
        CpuReg::Rax,
        CpuReg::Rcx,
        CpuReg::Rdx,
        CpuReg::Rbx,
        CpuReg::Rsp,
        CpuReg::Rbp,
        CpuReg::Rsi,
        CpuReg::Rdi,
        CpuReg::Rip,
    ]
    .map(|reg| cpu.reg(reg))
    .to_vec();
    let (code_segment, retired) = (cpu.code_segment(), cpu.retired());
    let virtual_time = rig.machine.virtual_time();
    let mut devices = Vec::new();
    let bus = rig.machine.bus_mut();
    for index in 0.. {
        let Some(device) = bus.device_mut(index) else {
            break;
        };
        devices.push((device.save(), device.take_trace(), device.trace_dropped()));
    }
    Outcome {
        registers,
        code_segment,
        retired,
        virtual_time,
        devices,
    }
}

#[test]
fn run_chunking_changes_nothing_a_host_can_observe() {
    for (name, config) in [
        ("routine", boot_config(routine_program(), 2048)),
        ("dma", dma_config()),
    ] {
        let whole = outcome(&config, None);
        assert!(whole.devices.iter().all(|(_, _, dropped)| *dropped == 0));
        assert!(whole.devices.iter().any(|(_, trace, _)| !trace.is_empty()));
        for step in [1, 7] {
            assert_eq!(outcome(&config, Some(step)), whole, "{name}, step {step}");
        }
    }
}
