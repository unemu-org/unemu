// SPDX-License-Identifier: AGPL-3.0-or-later
//! ATA-2 §8.3 PIO data-in blocks through Intel 8259A Edge Triggered Mode.
//! See `backlog/docs/design/spec.md` §7.1 for fixed-disk wiring.

use bus::{Bus, DecodeMap, PortRange, PortWidth};
use chipset::CascadedPic;
use cpu::x86jit_backend::{RealSegments, X86JitCpu};
use devices::{Ata, AtaDrive, AtaGeometry};
use machine::{IrqLine, Machine, RunOutcome};

#[test]
fn unacknowledged_read_interrupt_is_not_merged_with_the_next_block() {
    let mut cpu = X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).unwrap();
    cpu.map_ram(0, 0x10000).unwrap();
    let mut program = vec![0xFA, 0xFC, 0xBC, 0x00, 0x70]; // CLI; CLD; MOV SP,7000h
    for (port, value) in [
        (0x20u16, 0x11u8),
        (0x21, 0x08),
        (0x21, 0x04),
        (0x21, 0x01),
        (0xA0, 0x11),
        (0xA1, 0x70),
        (0xA1, 0x02),
        (0xA1, 0x01),
        (0x21, 0xFB),
        (0xA1, 0xBF),
        (0x1F6, 0xA0),
        (0x1F2, 2),
        (0x1F3, 1),
        (0x1F4, 0),
        (0x1F5, 0),
        (0x1F7, 0x20),
    ] {
        program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
    }
    program.push(0xFB); // STI
    let mut timeout_branches = Vec::new();
    for block in 1..=2 {
        program.extend([0xB9, 0xFF, 0xFF]); // MOV CX,FFFFh
        let wait = program.len();
        program.extend([0x83, 0x3E, 0x00, 0x05, block, 0x74, 0x04]); // CMP [0500h],block; JE ready
        program.extend([0xE2, (wait as isize - (program.len() + 2) as isize) as u8]); // LOOP wait
        program.extend([0xEB, 0]); // JMP timeout
        timeout_branches.push(program.len() - 1);
        program.extend([
            0xBA, 0xF0, 0x01, 0xBF, 0x00, 0x06, 0xB9, 0x00, 0x01, 0xF3, 0x6D, // REP INSW
            0xBA, 0xF7, 0x01,
            0xEC, // read Status after data, leaving IRQ high during transfer
        ]);
    }
    let halt = program.len();
    program.extend([0xFA, 0xF4]);
    for branch in timeout_branches {
        program[branch] = (halt - (branch + 1)).try_into().unwrap();
    }
    cpu.write_mem(0x7C00, &program).unwrap();
    cpu.write_mem(0x76 * 4, &[0, 0x80, 0, 0]).unwrap();
    cpu.write_mem(
        0x8000,
        &[
            0x50, // PUSH AX
            0xFF, 0x06, 0x00, 0x05, // INC word [0500h]
            0xB0, 0x20, 0xE6, 0xA0, 0xE6, 0x20, // slave and master EOI
            0x58, 0xCF, // POP AX; IRET
        ],
    )
    .unwrap();
    let geometry = AtaGeometry::new(1, 1, 3).unwrap();
    let ata = Ata::with_drives([
        Some(AtaDrive::new(geometry, vec![0xA5; 3 * 512]).unwrap()),
        None,
    ])
    .unwrap();
    let pic = CascadedPic::new();
    let bus = Bus::new(
        vec![
            Box::new(pic.master_device()),
            Box::new(pic.slave_device()),
            Box::new(ata),
        ],
        DecodeMap {
            ports: [
                (0x20..=0x21, 0, PortWidth::Byte),
                (0xA0..=0xA1, 1, PortWidth::Byte),
                (0x1F0..=0x1F0, 2, PortWidth::Word),
                (0x1F1..=0x1F7, 2, PortWidth::Byte),
            ]
            .into_iter()
            .map(|(ports, device, width)| PortRange {
                ports,
                device,
                width,
            })
            .collect(),
            mmio: Vec::new(),
        },
    );
    let mut machine = Machine::new(
        cpu,
        bus,
        Box::new(pic),
        vec![IrqLine {
            device: 2,
            output: 0,
            irq: 14,
        }],
        None,
    );
    assert_eq!(
        machine.run_bounded(Some(300_000)).unwrap(),
        RunOutcome::Halted
    );
    let mut count = [0; 2];
    machine.cpu().read_mem(0x500, &mut count).unwrap();
    assert_eq!(count, [2, 0]);
    let mut sector = [0; 512];
    machine.cpu().read_mem(0x600, &mut sector).unwrap();
    assert_eq!(sector, [0xA5; 512]);
}
