// SPDX-License-Identifier: AGPL-3.0-or-later
//! Scheduler migration observations for the PC/AT goldens (§10).
//!
//! See `backlog/docs/design/scheduler.md` § Migration and § Review outcome.
//! This stays in CLI test support: it can reuse frontend programs and assembly
//! presets without making either a dependency of the lower-level testkit crate.
//! Device traces are drained at each transaction, preserving equal-time order
//! across devices and Intel 8259A INTA/EOI (datasheet, "Interrupt Sequence" and
//! "End of Interrupt"). Sorting completed per-device traces would lose that order.

use std::{cell::RefCell, collections::HashMap, fmt, rc::Rc};

use anyhow::Result;
use assembly::{AssembledMachine, ConsoleProbe, DeviceId, RunPolicy, StopCondition};
use bus::{Bus, DecodeMap, PortRange};
use chipset::{NoInterruptController, PicObservation, PicObservationLog, SharedPic};
use cpu::X86JitCpu;
use device::{
    Device, DeviceError, DmaMemory, DmaOutcome, DmaRequest, NextEvent, TraceEvent, VClock,
};
use devices::{Uart16550, I8042};
use machine::{Machine, MachineError, RunOutcome};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    Halted,
    CapReached,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Access {
        device: DeviceId,
        access: TraceEvent,
    },
    Pic(PicObservation),
    Stop {
        at: VClock,
        virtual_time: u64,
        reason: Stop,
    },
}

pub type Stream = Vec<Event>;

/// Both final cap times must be in this inclusive, explicitly reviewed window.
#[derive(Clone, Copy, Debug)]
pub struct CapEndpoint {
    pub cap: u64,
    pub max_overshoot: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Normalisations {
    /// Add this constant to right-hand access times only. No shift is inferred.
    pub right_access_shift: i64,
    /// Never removes trailing accesses or normalises a halt/error into a cap.
    pub cap_endpoint: Option<CapEndpoint>,
}

fn access_time(event: &Event) -> Option<u64> {
    match event {
        Event::Access {
            access: TraceEvent::Port { at, .. } | TraceEvent::Mmio { at, .. },
            ..
        } => Some(at.0),
        _ => None,
    }
}

fn equivalent(left: &Event, right: &Event, options: Normalisations, final_pair: bool) -> bool {
    if let (Some(left_time), Some(right_time)) = (access_time(left), access_time(right)) {
        let mut shifted = right.clone();
        if let Event::Access {
            access: TraceEvent::Port { at, .. } | TraceEvent::Mmio { at, .. },
            ..
        } = &mut shifted
        {
            *at = VClock(left_time);
        }
        return i128::from(left_time)
            == i128::from(right_time) + i128::from(options.right_access_shift)
            && left == &shifted;
    }
    if final_pair {
        if let (
            Some(window),
            Event::Stop {
                at: la,
                virtual_time: lt,
                reason: Stop::CapReached,
            },
            Event::Stop {
                at: ra,
                virtual_time: rt,
                reason: Stop::CapReached,
            },
        ) = (options.cap_endpoint, left, right)
        {
            let in_window =
                |time: u64| time >= window.cap && time - window.cap <= window.max_overshoot;
            return left == right
                || (in_window(la.0) && in_window(ra.0) && in_window(*lt) && in_window(*rt));
        }
    }
    left == right
}

#[derive(Debug)]
pub struct Difference {
    pub index: usize,
    pub left: Option<Event>,
    pub right: Option<Event>,
    context: String,
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "first difference at event {}\nleft: {:?}\nright: {:?}\n{}",
            self.index, self.left, self.right, self.context
        )
    }
}

/// `Ok(())` means identical under exactly the requested normalisations.
pub fn compare(
    left: &[Event],
    right: &[Event],
    options: Normalisations,
) -> Result<(), Box<Difference>> {
    for index in 0..left.len().max(right.len()) {
        if let (Some(l), Some(r)) = (left.get(index), right.get(index)) {
            if equivalent(
                l,
                r,
                options,
                index + 1 == left.len() && index + 1 == right.len(),
            ) {
                continue;
            }
        }
        let mut context = String::new();
        for i in index.saturating_sub(2)..(index + 3).min(left.len().max(right.len())) {
            use fmt::Write;
            writeln!(
                context,
                "[{i}] left={:?} | right={:?}",
                left.get(i),
                right.get(i)
            )
            .expect("writing a string");
        }
        return Err(Box::new(Difference {
            index,
            left: left.get(index).cloned(),
            right: right.get(index).cloned(),
            context,
        }));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub enum Workload {
    Serial,
    Boot,
    Keyboard,
    Rtc,
    VgaText,
    BiosPost,
    FreeDosPrompt,
}

pub const FAST_WORKLOADS: [Workload; 5] = [
    Workload::Serial,
    Workload::Boot,
    Workload::Keyboard,
    Workload::Rtc,
    Workload::VgaText,
];

pub struct Prepared {
    pub machine: Machine<X86JitCpu>,
    pub device_index: HashMap<DeviceId, usize>,
    pub policy: RunPolicy,
}

#[path = "fixtures.rs"]
mod fixtures;

/// Uses the same presets, programs, quantum, inputs and policies as CLI goldens.
/// Missing optional firmware/media returns `None`; other assembly/I/O errors fail.
pub fn prepare(workload: Workload) -> Result<Option<Prepared>> {
    let mut policy = RunPolicy::natural(None, None);
    let assembled = match workload {
        Workload::Serial => {
            let cpu = X86JitCpu::new(cli::RAM_BYTES, cli::ENTRY, cli::HELLO_PROGRAM)?;
            let bus = Bus::new(
                vec![Box::new(Uart16550::new(chipset::COM1_BASE))],
                DecodeMap {
                    ports: vec![PortRange {
                        ports: chipset::COM1_BASE..=chipset::COM1_TOP,
                        device: 0,
                    }],
                    mmio: Vec::new(),
                },
            );
            AssembledMachine {
                machine: Machine::new(
                    cpu,
                    bus,
                    Box::new(NoInterruptController),
                    cli::QUANTUM,
                    Vec::new(),
                    None,
                ),
                device_index: HashMap::from([(DeviceId::Uart, 0)]),
                observation: assembly::Observation::default(),
            }
        }
        Workload::Boot => {
            policy.idle_limit = Some(cli::BOOT_IDLE_LIMIT);
            assembly::assemble(&assembly::at_boot(cli::BOOT_PROGRAM.to_vec()))?
        }
        Workload::Keyboard => {
            policy.idle_limit = Some(cli::KBD_IDLE_LIMIT);
            let mut assembled =
                assembly::assemble(&assembly::at_keyboard(cli::KBD_PROGRAM.to_vec()))?;
            assembled
                .machine
                .bus_mut()
                .device_mut(assembled.device_index[&DeviceId::Keyboard])
                .expect("keyboard")
                .as_any_mut()
                .and_then(|any| any.downcast_mut::<I8042>())
                .expect("8042")
                .push_scancode(cli::KBD_SCANCODE);
            assembled
        }
        Workload::Rtc => {
            policy.idle_limit = Some(cli::RTC_IDLE_LIMIT);
            assembly::assemble(&assembly::at_rtc(
                cli::RTC_PROGRAM.to_vec(),
                cli::RTC_SECOND_TICKS,
                cli::RTC_UIP_TICKS,
            ))?
        }
        Workload::VgaText => {
            assembly::assemble(&assembly::at_vga_text(cli::VGA_TEXT_PROGRAM.to_vec()))?
        }
        Workload::BiosPost | Workload::FreeDosPrompt => {
            let Some(rom) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
                return Ok(None);
            };
            anyhow::ensure!(rom.len() == 32768, "Xi8088 BIOS must be 32 KiB");
            let floppy = if matches!(workload, Workload::FreeDosPrompt) {
                let Some(image) = fixtures::fixture("freedos/freedos.img") else {
                    return Ok(None);
                };
                anyhow::ensure!(image.len() == 1_474_560, "FreeDOS floppy must be 1.44 MB");
                Some(image)
            } else {
                None
            };
            policy = RunPolicy::natural(
                Some(cli::BIOS_GOLDEN_MAX_RETIRED),
                Some(cli::BIOS_IDLE_LIMIT),
            );
            if floppy.is_some() {
                policy.retired_cap = Some(cli::BIOS_MAX_RETIRED);
                policy.stop = StopCondition::PromptThen {
                    console: ConsoleProbe {
                        base: cli::MDA_BUFFER_BASE,
                        cols: 80,
                        rows: 25,
                    },
                    needle: cli::DOS_DATE_PROMPT.to_string(),
                    poll_step: cli::DOS_INTERACTIVE_POLL_STEP,
                    then_budget: 0,
                };
            }
            assembly::assemble(&assembly::bios(rom, floppy, cli::BIOS_QUANTUM))?
        }
    };
    Ok(Some(Prepared {
        machine: assembled.machine,
        device_index: assembled.device_index,
        policy,
    }))
}

type SharedStream = Rc<RefCell<Stream>>;

fn drain_pic(logs: &[PicObservationLog], stream: &SharedStream) {
    for log in logs {
        stream
            .borrow_mut()
            .extend(log.take().into_iter().map(Event::Pic));
    }
}

struct ObservedDevice {
    inner: Box<dyn Device>,
    id: DeviceId,
    stream: SharedStream,
    pic: Vec<PicObservationLog>,
}

impl ObservedDevice {
    fn before(&self) {
        drain_pic(&self.pic, &self.stream);
    }
    fn after(&mut self) {
        self.stream
            .borrow_mut()
            .extend(
                self.inner
                    .take_trace()
                    .into_iter()
                    .map(|access| Event::Access {
                        device: self.id,
                        access,
                    }),
            );
        drain_pic(&self.pic, &self.stream);
    }
}

impl Device for ObservedDevice {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.before();
        let result = self.inner.port_read(port, size);
        self.after();
        result
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        self.before();
        let result = self.inner.port_write(port, size, value);
        self.after();
        result
    }
    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        self.before();
        let result = self.inner.mmio_read(addr, size);
        self.after();
        result
    }
    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        self.before();
        let result = self.inner.mmio_write(addr, size, value);
        self.after();
        result
    }
    fn tick(&mut self, now: VClock) {
        self.inner.tick(now);
    }
    fn sync(&mut self, now: VClock) {
        self.inner.sync(now);
    }
    fn next_event(&self, now: VClock) -> NextEvent {
        self.inner.next_event(now)
    }
    fn irq_pending(&self) -> bool {
        self.inner.irq_pending()
    }
    fn irq_vector(&self) -> Option<u8> {
        self.inner.irq_vector()
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn save(&self) -> Vec<u8> {
        self.inner.save()
    }
    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        self.inner.restore(snapshot)
    }
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.inner.take_trace()
    }
    fn take_dma_request(&mut self) -> Option<DmaRequest> {
        self.inner.take_dma_request()
    }
    fn dma_complete(&mut self, outcome: DmaOutcome) {
        self.inner.dma_complete(outcome);
    }
    fn dma_service_write(
        &mut self,
        channel: u8,
        mem: &mut dyn DmaMemory,
        data: &[u8],
    ) -> device::DmaTransfer {
        self.inner.dma_service_write(channel, mem, data)
    }
    fn dma_service_read(
        &mut self,
        channel: u8,
        mem: &mut dyn DmaMemory,
        out: &mut [u8],
    ) -> device::DmaTransfer {
        self.inner.dma_service_read(channel, mem, out)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        self.inner.as_any_mut()
    }
}

/// Engine selection stays at the call site; the default calls the legacy driver.
/// The observer wraps the existing bus devices without changing the run quantum.
pub fn observe_with(
    mut prepared: Prepared,
    engine: impl FnOnce(&mut Machine<X86JitCpu>, &RunPolicy) -> Result<RunOutcome, MachineError>,
) -> Stream {
    let stream = SharedStream::default();
    let mut pic = Vec::new();
    let mut devices: Vec<_> = prepared
        .device_index
        .iter()
        .map(|(&id, &index)| (id, index))
        .collect();
    devices.sort_unstable_by_key(|&(_, index)| index);
    for &(_, index) in &devices {
        if let Some(shared) = prepared
            .machine
            .bus_mut()
            .device_mut(index)
            .expect("device")
            .as_any_mut()
            .and_then(|any| any.downcast_mut::<SharedPic>())
        {
            pic.push(shared.observe());
        }
    }
    for (id, index) in devices {
        let slot = prepared
            .machine
            .bus_mut()
            .device_mut(index)
            .expect("device");
        // A placeholder permits moving the box out without a bus API change.
        let mut inner = std::mem::replace(slot, Box::new(Uart16550::new(0)));
        inner.take_trace();
        *slot = Box::new(ObservedDevice {
            inner,
            id,
            stream: stream.clone(),
            pic: pic.clone(),
        });
    }
    let result = engine(&mut prepared.machine, &prepared.policy);
    drain_pic(&pic, &stream);
    let reason = match result {
        Ok(RunOutcome::Halted) => Stop::Halted,
        Ok(RunOutcome::RetiredCapReached) => Stop::CapReached,
        Err(error) => Stop::Error(error.to_string()),
    };
    stream.borrow_mut().push(Event::Stop {
        at: prepared.machine.clock(),
        virtual_time: prepared.machine.virtual_time(),
        reason,
    });
    drop(prepared);
    Rc::try_unwrap(stream)
        .expect("observers released with the machine")
        .into_inner()
}

pub fn run_workload(workload: Workload) -> Result<Option<Stream>> {
    Ok(prepare(workload)?.map(|prepared| observe_with(prepared, assembly::run)))
}

/// Run the same prepared workload and policy with an explicit device engine.
pub fn run_workload_engine(
    workload: Workload,
    engine: machine::SchedulerEngine,
) -> Result<Option<Stream>> {
    Ok(prepare(workload)?.map(|mut prepared| {
        prepared.machine = prepared.machine.with_scheduler_engine(engine);
        observe_with(prepared, assembly::run)
    }))
}
