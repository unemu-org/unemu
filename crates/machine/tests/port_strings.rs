// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real-mode string port transfers across the CPU, bus and IRQ seams (§5–§6).
//!
//! Intel 80286 Programmer's Reference Manual, INS, OUTS, REP and IRET
//! instruction descriptions; see `backlog/docs/design/machine-time-profile.md`
//! §String instruction accounting for the engine's per-element time convention.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use bus::{Bus, DecodeMap, MmioRange, PortRange, PortWidth};
use chipset::SharedPic;
use cpu::x86jit_backend::{RealSegments, X86JitCpu};
use cpu::{Cpu, CpuReg};
use device::{Device, DeviceError, TraceEvent, VClock};
use machine::{IrqLine, Machine, RunOutcome};
use testkit::MockBus;

const DATA: u16 = 0x300;
const MARKER: u16 = 0x80;
const ENTRY: u64 = 0x7C00;
const MMIO: u64 = 0xB8000;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Transfer {
    port: u16,
    size: u8,
    value: u32,
    at: VClock,
}

#[derive(Debug, PartialEq, Eq)]
enum Access {
    Port(Transfer),
    MmioRead { addr: u64, size: u8, value: u64 },
    MmioWrite { addr: u64, size: u8, value: u64 },
}

#[derive(Default)]
struct PortState {
    input: VecDeque<u32>,
    transfers: Vec<Transfer>,
    accesses: Vec<Access>,
    mmio_input: VecDeque<u64>,
    data_count: usize,
    irq_after: Option<usize>,
    now: VClock,
}

struct ScriptPort(Rc<RefCell<PortState>>);

impl ScriptPort {
    fn record(&mut self, port: u16, size: u8, value: u32) {
        let mut state = self.0.borrow_mut();
        let at = state.now;
        let transfer = Transfer {
            port,
            size,
            value,
            at,
        };
        state.transfers.push(transfer);
        state.accesses.push(Access::Port(transfer));
        if port == DATA {
            state.data_count += 1;
        }
    }
}

impl Device for ScriptPort {
    fn name(&self) -> &'static str {
        "script-port"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = self
            .0
            .borrow_mut()
            .input
            .pop_front()
            .expect("unexpected port read");
        self.record(port, size, value);
        Ok(value)
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.record(port, size, value);
        Ok(())
    }
    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        let mut state = self.0.borrow_mut();
        let value = state.mmio_input.pop_front().expect("unexpected MMIO read");
        state.accesses.push(Access::MmioRead { addr, size, value });
        Ok(value)
    }
    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        self.0
            .borrow_mut()
            .accesses
            .push(Access::MmioWrite { addr, size, value });
        Ok(())
    }
    fn tick(&mut self, now: VClock) {
        self.0.borrow_mut().now = now;
    }
    fn irq_pending(&self) -> bool {
        let state = self.0.borrow();
        state
            .irq_after
            .is_some_and(|count| state.data_count >= count)
    }
    fn reset(&mut self) {
        *self.0.borrow_mut() = PortState::default();
    }
    fn save(&self) -> Vec<u8> {
        panic!("test port has no snapshot interface")
    }
    fn restore(&mut self, _snapshot: &[u8]) -> Result<(), DeviceError> {
        panic!("test port has no snapshot interface")
    }
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        Vec::new()
    }
}

fn cpu(code: &[u8]) -> X86JitCpu {
    let segments = RealSegments {
        ds: 0x100,
        es: 0x200,
        ..RealSegments::flat(0)
    };
    let mut cpu = X86JitCpu::new_real16_bare(0xC0000, 0, ENTRY as u16, segments).unwrap();
    cpu.map_ram(0, 0x20000).unwrap();
    cpu.write_mem(ENTRY, code).unwrap();
    cpu.set_reg(CpuReg::Rdx, DATA.into());
    cpu.set_reg(CpuReg::Rcx, 3);
    cpu.set_reg(CpuReg::Rsi, 0x600);
    cpu.set_reg(CpuReg::Rdi, 0x700);
    cpu.write_mem(0x1600, &[0x12, 0x34, 0x56]).unwrap();
    cpu
}

fn machine(cpu: X86JitCpu, state: Rc<RefCell<PortState>>) -> Machine<X86JitCpu> {
    let mut pic = SharedPic::new();
    // Intel 8259A datasheet, Initialization Command Words: single, edge-triggered,
    // 8086 mode, vector base 20h, IRQ0 unmasked.
    for (port, value) in [(0x20, 0x13), (0x21, 0x20), (0x21, 1), (0x21, 0xFE)] {
        pic.port_write(port, 1, value).unwrap();
    }
    let bus = Bus::new(
        vec![Box::new(ScriptPort(state))],
        DecodeMap {
            ports: [DATA, MARKER]
                .into_iter()
                .map(|port| PortRange {
                    ports: port..=port + 1,
                    device: 0,
                    width: PortWidth::Word,
                })
                .collect(),
            mmio: vec![MmioRange {
                addrs: MMIO..=MMIO + 0x7FFF,
                device: 0,
            }],
        },
    );
    Machine::new(
        cpu,
        bus,
        Box::new(pic),
        vec![IrqLine { device: 0, irq: 0 }],
        None,
    )
}

fn state() -> Rc<RefCell<PortState>> {
    Rc::new(RefCell::new(PortState {
        input: [0x1234, 0xABCD, 0x5678].into(),
        ..PortState::default()
    }))
}

fn memory(machine: &Machine<X86JitCpu>, addr: u64, len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    machine.cpu().read_mem(addr, &mut bytes).unwrap();
    bytes
}

#[test]
fn scripted_port_isolated_on_mock_bus() {
    let state = state();
    state.borrow_mut().irq_after = Some(2);
    let mut bus = MockBus::new(Box::new(ScriptPort(state.clone())));
    assert!(!bus.device().irq_pending());
    bus.tick(7);
    assert_eq!(bus.port_read(DATA, 2).unwrap(), 0x1234);
    assert!(!bus.device().irq_pending());
    bus.port_write(DATA, 1, 0x56).unwrap();
    assert!(bus.device().irq_pending());
    assert_eq!(
        state.borrow().transfers,
        [
            Transfer {
                port: DATA,
                size: 2,
                value: 0x1234,
                at: VClock(7)
            },
            Transfer {
                port: DATA,
                size: 1,
                value: 0x56,
                at: VClock(7)
            },
        ]
    );
    state.borrow_mut().mmio_input.push_back(0xABCD);
    assert_eq!(bus.mmio_read(MMIO, 2).unwrap(), 0xABCD);
    bus.mmio_write(MMIO + 2, 2, 0x1234).unwrap();
    assert_eq!(
        state.borrow().accesses,
        [
            Access::Port(Transfer {
                port: DATA,
                size: 2,
                value: 0x1234,
                at: VClock(7)
            }),
            Access::Port(Transfer {
                port: DATA,
                size: 1,
                value: 0x56,
                at: VClock(7)
            }),
            Access::MmioRead {
                addr: MMIO,
                size: 2,
                value: 0xABCD
            },
            Access::MmioWrite {
                addr: MMIO + 2,
                size: 2,
                value: 0x1234
            },
        ]
    );
    bus.device_mut().reset();
    assert!(!bus.device().irq_pending());
    assert!(state.borrow().transfers.is_empty());
}

#[test]
fn rep_outsb_preserves_order_registers_and_per_element_time() {
    let state = state();
    // CLD; REP OUTSB; HLT.
    let mut machine = machine(cpu(&[0xFC, 0xF3, 0x6E, 0xF4]), state.clone());
    for count in 1..=3 {
        let cap = 1 + count;
        assert_eq!(
            machine.run_bounded(Some(cap)).unwrap(),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), cap);
        assert_eq!(machine.clock(), VClock(cap));
        assert_eq!(machine.cpu().completed_steps(), cap);
        assert_eq!(machine.cpu().reg(CpuReg::Rcx), 3 - count);
        assert_eq!(machine.cpu().reg(CpuReg::Rsi), 0x600 + count);
        assert_eq!(
            machine.cpu().reg(CpuReg::Rip),
            ENTRY + if count == 3 { 3 } else { 1 }
        );
        assert_eq!(machine.cpu().retired(), 1 + u64::from(count == 3));
        assert_eq!(state.borrow().transfers.len() as u64, count);
    }
    assert_eq!(machine.run_bounded(Some(20)).unwrap(), RunOutcome::Halted);
    assert_eq!(machine.virtual_time(), 5);
    assert_eq!(machine.cpu().retired(), 3);
    assert_eq!(
        state.borrow().transfers,
        [0x12, 0x34, 0x56]
            .into_iter()
            .enumerate()
            .map(|(i, value)| Transfer {
                port: DATA,
                size: 1,
                value,
                at: VClock(1 + i as u64)
            })
            .collect::<Vec<_>>()
    );
}

#[test]
fn rep_insw_stores_words_in_es_and_wraps_di() {
    let state = state();
    // CLD; DS override REP INSW; HLT. INS must still use ES.
    let mut cpu = cpu(&[0xFC, 0x3E, 0xF3, 0x6D, 0xF4]);
    cpu.set_reg(CpuReg::Rdi, 0xFFFE);
    let mut machine = machine(cpu, state.clone());
    assert_eq!(machine.run_bounded(Some(20)).unwrap(), RunOutcome::Halted);
    assert_eq!(memory(&machine, 0x11FFE, 2), [0x34, 0x12]);
    assert_eq!(memory(&machine, 0x2000, 4), [0xCD, 0xAB, 0x78, 0x56]);
    assert_eq!(memory(&machine, 0x10FFE, 2), [0, 0]);
    assert_eq!(machine.cpu().reg(CpuReg::Rdi), 4);
    assert_eq!(machine.cpu().reg(CpuReg::Rcx), 0);
    assert_eq!(machine.cpu().reg(CpuReg::Rip), ENTRY + 5);
    assert_eq!(machine.virtual_time(), 5);
    assert_eq!(machine.cpu().completed_steps(), 5);
    assert_eq!(machine.cpu().retired(), 3);
    assert!(state.borrow().input.is_empty());
    assert_eq!(
        state
            .borrow()
            .transfers
            .iter()
            .map(|t| (t.size, t.at))
            .collect::<Vec<_>>(),
        [(2, VClock(1)), (2, VClock(2)), (2, VClock(3))]
    );
}

#[test]
fn es_override_rep_outsb_reads_es_instead_of_ds() {
    let state = state();
    let mut cpu = cpu(&[0xFC, 0x26, 0xF3, 0x6E, 0xF4]);
    cpu.write_mem(0x2600, &[0x87, 0x65, 0x43]).unwrap();
    let mut machine = machine(cpu, state.clone());
    assert_eq!(machine.run_bounded(Some(20)).unwrap(), RunOutcome::Halted);
    assert_eq!(
        state
            .borrow()
            .transfers
            .iter()
            .map(|t| t.value)
            .collect::<Vec<_>>(),
        [0x87, 0x65, 0x43]
    );
    assert_eq!(machine.cpu().reg(CpuReg::Rsi), 0x603);
    assert_eq!(machine.cpu().reg(CpuReg::Rcx), 0);
    assert_eq!(machine.virtual_time(), 5);
}

fn irq_during_rep(input: bool) {
    let state = state();
    state.borrow_mut().irq_after = Some(1);
    // CLD; STI; NOP; REP INSW/OUTSB; CLI; HLT.
    let mut cpu = cpu(&[
        0xFC,
        0xFB,
        0x90,
        0xF3,
        if input { 0x6D } else { 0x6E },
        0xFA,
        0xF4,
    ]);
    cpu.write_mem(0x80, &[0, 0x80, 0, 0]).unwrap();
    // Save the interrupted count and indices, and inspect the just-written INS
    // word before emitting the marker. None of these instructions changes CX/SI/DI.
    cpu.write_mem(
        0x8000,
        &[
            0x89, 0x0E, 0, 9, // MOV [0900], CX
            0x89, 0x36, 2, 9, // MOV [0902], SI
            0x89, 0x3E, 4, 9, // MOV [0904], DI
            0x26, 0x8B, 0x45, 0xFE, // MOV AX, ES:[DI-2]
            0xA3, 6, 9, // MOV [0906], AX
            0xB0, 0x99, 0xE6, 0x80, 0xCF, // MOV AL,99; OUT 80,AL; IRET
        ],
    )
    .unwrap();
    let mut machine = machine(cpu, state.clone());
    assert_eq!(machine.run_bounded(Some(100)).unwrap(), RunOutcome::Halted);
    assert_eq!(
        memory(&machine, 0x1900, 6),
        if input {
            [2, 0, 0, 6, 2, 7]
        } else {
            [2, 0, 1, 6, 0, 7]
        }
    );
    if input {
        assert_eq!(memory(&machine, 0x1906, 2), [0x34, 0x12]);
        assert_eq!(
            memory(&machine, 0x2700, 6),
            [0x34, 0x12, 0xCD, 0xAB, 0x78, 0x56]
        );
    }
    let state = state.borrow();
    assert_eq!(
        state.transfers.iter().map(|t| t.port).collect::<Vec<_>>(),
        [DATA, MARKER, DATA, DATA]
    );
    assert_eq!(
        state.transfers.iter().map(|t| t.value).collect::<Vec<_>>(),
        if input {
            [0x1234, 0x99, 0xABCD, 0x5678]
        } else {
            [0x12, 0x99, 0x34, 0x56]
        }
    );
    assert_eq!(machine.cpu().reg(CpuReg::Rcx), 0);
    assert_eq!(
        machine.cpu().reg(CpuReg::Rsi),
        if input { 0x600 } else { 0x603 }
    );
    assert_eq!(
        machine.cpu().reg(CpuReg::Rdi),
        if input { 0x706 } else { 0x700 }
    );
    assert_eq!(
        machine.cpu().reg(CpuReg::Rsp),
        RealSegments::DEFAULT_SP.into()
    );
    assert!(!machine.cpu().has_pending_irq());
    // Three prelude instructions, three elements, eight ISR instructions, CLI/HLT.
    assert_eq!(machine.virtual_time(), 16);
    assert_eq!(machine.cpu().completed_steps(), 16);
    assert_eq!(machine.cpu().retired(), 14);
}

#[test]
fn irq_between_outsb_elements_resumes_after_iret() {
    irq_during_rep(false);
}

#[test]
fn irq_waits_for_insw_store_then_resumes_after_iret() {
    irq_during_rep(true);
}

#[test]
fn rep_insw_cap_retains_answer_until_next_run_stores_without_rereading() {
    let state = state();
    // REP INSW; NOP; HLT. One element isolates its store from another input.
    let mut cpu = cpu(&[0xF3, 0x6D, 0x90, 0xF4]);
    cpu.set_reg(CpuReg::Rcx, 1);
    cpu.write_mem(0x2700, &[0xAA, 0x55]).unwrap();
    let mut machine = machine(cpu, state.clone());
    for _ in 0..2 {
        assert_eq!(
            machine.run_bounded(Some(1)).unwrap(),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), 1);
        assert_eq!(machine.clock(), VClock(1));
        assert_eq!(machine.cpu().completed_steps(), 1);
        assert_eq!(machine.cpu().reg(CpuReg::Rip), ENTRY);
        assert_eq!(machine.cpu().reg(CpuReg::Rdi), 0x700);
        assert_eq!(machine.cpu().reg(CpuReg::Rcx), 1);
        assert_eq!(memory(&machine, 0x2700, 2), [0xAA, 0x55]);
        assert_eq!(
            state.borrow().transfers,
            [Transfer {
                port: DATA,
                size: 2,
                value: 0x1234,
                at: VClock(0),
            }]
        );
        assert_eq!(state.borrow().input, [0xABCD, 0x5678]);
    }
    assert_eq!(
        machine.run_bounded(Some(2)).unwrap(),
        RunOutcome::RetiredCapReached
    );
    assert_eq!(memory(&machine, 0x2700, 2), [0x34, 0x12]);
    assert_eq!(machine.cpu().reg(CpuReg::Rdi), 0x702);
    assert_eq!(machine.cpu().reg(CpuReg::Rcx), 0);
    assert_eq!(machine.cpu().reg(CpuReg::Rip), ENTRY + 3);
    assert_eq!(machine.virtual_time(), 2);
    assert_eq!(machine.cpu().completed_steps(), 2);
    assert_eq!(machine.cpu().retired(), 2);
    assert_eq!(state.borrow().transfers.len(), 1);
    assert_eq!(state.borrow().input, [0xABCD, 0x5678]);
    assert_eq!(
        machine.run_bounded(Some(3)).unwrap(),
        RunOutcome::RetiredCapReached
    );
    assert_eq!(machine.virtual_time(), 3);
    assert_eq!(machine.cpu().retired(), 3);
    assert_eq!(state.borrow().transfers.len(), 1);
}

#[test]
fn empty_rep_port_strings_count_one_step_without_accesses() {
    // MOVSB establishes the existing empty-REP convention for comparison.
    for opcode in [0xA4, 0x6C, 0x6D, 0x6E, 0x6F] {
        let state = state();
        let mut cpu = cpu(&[0xF3, opcode, 0xF4]);
        cpu.set_reg(CpuReg::Rcx, 0);
        let mut machine = machine(cpu, state.clone());
        assert_eq!(
            machine.run_bounded(Some(1)).unwrap(),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), 1);
        assert_eq!(machine.clock(), VClock(1));
        assert_eq!(machine.cpu().completed_steps(), 1);
        assert_eq!(machine.cpu().retired(), 1);
        assert_eq!(machine.cpu().reg(CpuReg::Rip), ENTRY + 2);
        assert_eq!(machine.cpu().reg(CpuReg::Rsi), 0x600);
        assert_eq!(machine.cpu().reg(CpuReg::Rdi), 0x700);
        assert_eq!(machine.cpu().reg(CpuReg::Rcx), 0);
        assert_eq!(memory(&machine, 0x2700, 2), [0, 0]);
        assert!(state.borrow().accesses.is_empty());
        assert_eq!(state.borrow().input.len(), 3);
        assert_eq!(
            machine.run_bounded(Some(2)).unwrap(),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), 2);
        assert_eq!(machine.cpu().retired(), 2);
        assert_eq!(machine.cpu().reg(CpuReg::Rip), ENTRY + 3);
        assert!(state.borrow().accesses.is_empty());
    }
}

fn rep_word_mmio(input: bool) {
    let state = state();
    state.borrow_mut().mmio_input = [0x1234, 0xABCD, 0x5678].into();
    // MOV AX,B800; MOV ES,AX; CLD; ES override REP INSW/OUTSW; HLT.
    let mut cpu = cpu(&[
        0xB8,
        0,
        0xB8,
        0x8E,
        0xC0,
        0xFC,
        0x26,
        0xF3,
        if input { 0x6D } else { 0x6F },
        0xF4,
    ]);
    cpu.map_mmio(MMIO, 0x8000).unwrap();
    let mut machine = machine(cpu, state.clone());
    assert_eq!(machine.run_bounded(Some(20)).unwrap(), RunOutcome::Halted);
    let mut expected = Vec::new();
    for (i, value) in [0x1234, 0xABCD, 0x5678].into_iter().enumerate() {
        let port = Access::Port(Transfer {
            port: DATA,
            size: 2,
            value,
            at: VClock(3 + i as u64),
        });
        let addr = MMIO + if input { 0x700 } else { 0x600 } + 2 * i as u64;
        if input {
            expected.push(port);
            expected.push(Access::MmioWrite {
                addr,
                size: 2,
                value: value.into(),
            });
        } else {
            expected.push(Access::MmioRead {
                addr,
                size: 2,
                value: value.into(),
            });
            expected.push(port);
        }
    }
    assert_eq!(state.borrow().accesses, expected);
    assert_eq!(state.borrow().transfers.len(), 3);
    if input {
        assert!(state.borrow().input.is_empty());
        assert_eq!(state.borrow().mmio_input.len(), 3);
    } else {
        assert!(state.borrow().mmio_input.is_empty());
        assert_eq!(state.borrow().input.len(), 3);
    }
    assert_eq!(machine.cpu().reg(CpuReg::Rcx), 0);
    assert_eq!(
        machine.cpu().reg(CpuReg::Rsi),
        if input { 0x600 } else { 0x606 }
    );
    assert_eq!(
        machine.cpu().reg(CpuReg::Rdi),
        if input { 0x706 } else { 0x700 }
    );
    assert_eq!(machine.cpu().reg(CpuReg::Rip), ENTRY + 10);
    assert_eq!(machine.virtual_time(), 7);
    assert_eq!(machine.cpu().completed_steps(), 7);
    assert_eq!(machine.cpu().retired(), 5);
}

#[test]
fn rep_insw_mmio_destination_writes_once_after_each_port_read() {
    rep_word_mmio(true);
}

#[test]
fn rep_outsw_mmio_source_reads_once_before_each_port_write() {
    rep_word_mmio(false);
}
