// SPDX-License-Identifier: AGPL-3.0-or-later
//! The exceptions and interrupts the guest took, read between run slices
//! (§3.1).
//!
//! The CPU delivers exceptions and interrupts in the guest and keeps no
//! record of them unless asked ([`cpu::delivery`]). When a host diagnostic
//! asks ([`crate::Machine::enable_delivery_log`] for the diagnostic report,
//! [`crate::Machine::enable_delivery_trace`] for the trace), the machine
//! enables the CPU's delivery log and, after every run slice, moves its
//! records into a [`DeliveryTap`]: the last [`RECENT_DELIVERIES`] for the
//! report, and the [`DeliveryTrace`], a bounded text the front end appends
//! to its file or stream whenever it likes. A slice ends at every port
//! access before the bus performs it, so an IRQ's controller input is
//! named from the 8259's vector bases as they were at its delivery.
//!
//! The log lives outside the CPU state the guest executes on and the tap
//! outside the CPU: retired counts, virtual time and every guest effect are
//! the same with either on or off. Off, the cost is one branch per slice.
//! The formats are described in `backlog/docs/design/observation.md`
//! §Exception and interrupt deliveries.

use cpu::{Delivery, DeliveryCounts, DeliveryName};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;

/// How many deliveries the diagnostic report shows.
pub const RECENT_DELIVERIES: usize = 64;

/// How many deliveries the trace writes a line for, from the first.
pub const TRACE_LINES: u64 = 10_000;

/// After [`TRACE_LINES`], the trace writes a count line every this many
/// deliveries.
pub const TRACE_COUNT_EVERY: u64 = 1_000_000;

/// The last deliveries and the log's counts, for the diagnostic report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentDeliveries {
    /// The last deliveries, oldest first, at most [`RECENT_DELIVERIES`].
    pub records: Vec<Delivery>,
    /// Every delivery since the log was enabled.
    pub total: u64,
    /// Deliveries the CPU's log overwrote before the machine read them.
    pub dropped: u64,
}

/// The bounded text trace of every delivery: a line for each of the first
/// [`TRACE_LINES`], then a count line every [`TRACE_COUNT_EVERY`]
/// deliveries, and on request a summary of the counts per vector.
#[derive(Clone, Debug)]
pub struct DeliveryTrace {
    line_limit: u64,
    count_every: u64,
    /// Text not taken yet.
    pending: String,
    /// The log's total when the trace started: the trace numbers the
    /// deliveries after it from 1.
    base: u64,
    /// The number of the last delivery seen, dropped ones included.
    last: u64,
    /// Whether the line saying that lines stop was written.
    limit_noted: bool,
    lines: u64,
    dropped: u64,
    per_name: BTreeMap<DeliveryName, u64>,
}

impl Default for DeliveryTrace {
    fn default() -> Self {
        Self::with_bounds(TRACE_LINES, TRACE_COUNT_EVERY)
    }
}

impl DeliveryTrace {
    /// A trace with other bounds than [`TRACE_LINES`] and
    /// [`TRACE_COUNT_EVERY`], for tests; `count_every` is at least 1.
    pub fn with_bounds(line_limit: u64, count_every: u64) -> Self {
        Self {
            line_limit,
            count_every: count_every.max(1),
            pending: String::new(),
            base: 0,
            last: 0,
            limit_noted: false,
            lines: 0,
            dropped: 0,
            per_name: BTreeMap::new(),
        }
    }

    /// The same trace for a log that already counted `base` deliveries,
    /// which the trace leaves out.
    fn starting_at(mut self, base: u64) -> Self {
        self.base = base;
        self
    }

    /// Add one read of the log: `records`, the deliveries numbered
    /// `counts.total - records.len() + 1` to `counts.total` of the log,
    /// after `counts.dropped` the log lost.
    fn record(&mut self, records: &[Delivery], counts: DeliveryCounts) {
        let total = counts.total.saturating_sub(self.base);
        let first = total.saturating_sub(records.len() as u64);
        if counts.dropped != 0 {
            self.dropped += counts.dropped;
            let _ = writeln!(
                self.pending,
                "dropped  {} deliveries (the CPU's log overflowed before the machine read it)",
                counts.dropped
            );
        }
        // Count marks that fell among dropped deliveries.
        while self.next_mark() <= first {
            let mark = self.next_mark();
            let _ = writeln!(
                self.pending,
                "count    {mark} deliveries (the delivery itself was dropped)"
            );
            self.last = mark;
        }
        self.last = first;
        for record in records {
            self.last += 1;
            *self.per_name.entry(record.name()).or_default() += 1;
            if self.last <= self.line_limit {
                let _ = writeln!(self.pending, "delivery {record}");
                self.lines += 1;
            } else if !self.limit_noted {
                self.limit_noted = true;
                let _ = writeln!(
                    self.pending,
                    "trace    {} delivery lines written; from here a count line every {} deliveries",
                    self.lines, self.count_every
                );
            }
            if self.last.is_multiple_of(self.count_every) {
                let _ = writeln!(
                    self.pending,
                    "count    {} deliveries, the last at retired {}",
                    self.last, record.retired
                );
            }
        }
    }

    fn next_mark(&self) -> u64 {
        (self.last / self.count_every + 1) * self.count_every
    }

    /// The text written since the last take.
    pub fn take(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }

    /// The summary: the total, the lines written, the deliveries the log
    /// dropped before they were read, then one line per name with its count,
    /// exceptions first, then NMI, IRQs and software interrupts, each by
    /// number. Dropped deliveries are in the total only.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "summary  {} deliveries, {} lines written, {} dropped before they were read\n",
            self.last, self.lines, self.dropped
        );
        for (name, count) in &self.per_name {
            let _ = writeln!(text, "summary  {name}: {count}");
        }
        text
    }
}

/// What the machine collects from the CPU's delivery log; see the module
/// documentation.
pub struct DeliveryTap {
    recent: Option<VecDeque<Delivery>>,
    trace: Option<DeliveryTrace>,
    /// The capacity the CPU's log was enabled with.
    capacity: usize,
    /// Deliveries counted by earlier logs, which a larger one replaced.
    earlier: u64,
    total: u64,
    dropped: u64,
    /// Records read from the CPU after the last slice.
    buffer: Vec<Delivery>,
}

impl DeliveryTap {
    fn new(capacity: usize) -> Self {
        Self {
            recent: None,
            trace: None,
            capacity,
            earlier: 0,
            total: 0,
            dropped: 0,
            buffer: Vec::new(),
        }
    }

    /// Add the records in the buffer, read with `counts` from the current
    /// log.
    fn absorb(&mut self, counts: DeliveryCounts) {
        let counts = DeliveryCounts {
            dropped: counts.dropped,
            total: self.earlier + counts.total,
        };
        self.total = counts.total;
        self.dropped += counts.dropped;
        if let Some(recent) = &mut self.recent {
            for record in &self.buffer {
                if recent.len() == RECENT_DELIVERIES {
                    recent.pop_front();
                }
                recent.push_back(*record);
            }
        }
        if let Some(trace) = &mut self.trace {
            trace.record(&self.buffer, counts);
        }
    }
}

impl<C: cpu::Cpu> crate::Machine<C> {
    /// The tap with the CPU's log at `capacity` or more: a smaller log is
    /// read out first and replaced.
    fn delivery_tap(&mut self, capacity: usize) -> Result<&mut DeliveryTap, cpu::DeliveryLogError> {
        let enabled = self.deliveries.as_ref().map(|tap| tap.capacity);
        if enabled.is_none_or(|enabled| enabled < capacity) {
            self.read_deliveries();
            self.cpu.enable_delivery_log(capacity)?;
            let tap = self
                .deliveries
                .get_or_insert_with(|| Box::new(DeliveryTap::new(capacity)));
            tap.capacity = capacity;
            tap.earlier = tap.total;
        }
        Ok(self
            .deliveries
            .as_deref_mut()
            .expect("the tap exists once the log is enabled"))
    }

    /// Start keeping the last [`RECENT_DELIVERIES`] exceptions and
    /// interrupts the guest takes, for a diagnostic report; see
    /// [`crate::delivery_tap`]. The CPU's log stays outside the state the
    /// guest executes on, so retired counts and every guest effect are
    /// unchanged. Calling it again keeps what was kept. A CPU without a
    /// delivery log refuses.
    pub fn enable_delivery_log(&mut self) -> Result<(), cpu::DeliveryLogError> {
        let tap = self.delivery_tap(RECENT_DELIVERIES)?;
        tap.recent.get_or_insert_with(VecDeque::new);
        Ok(())
    }

    /// Start the bounded trace of every exception and interrupt the guest
    /// takes from now on ([`DeliveryTrace`]), with the CPU's log at its
    /// largest: a slice with at most one delivery per step loses none, and
    /// any loss is counted as dropped. Take its text with
    /// [`Self::take_delivery_trace`]. As [`Self::enable_delivery_log`], it
    /// leaves the run unchanged. Calling it again keeps the trace.
    pub fn enable_delivery_trace(&mut self) -> Result<(), cpu::DeliveryLogError> {
        let tap = self.delivery_tap(cpu::delivery::DELIVERY_LOG_MAX)?;
        let total = tap.total;
        tap.trace
            .get_or_insert_with(|| DeliveryTrace::default().starting_at(total));
        Ok(())
    }

    /// Name the controller input of an IRQ delivery from its vector, with
    /// the bus as it stands when a slice ends; for the delivery records.
    pub fn set_irq_input_of(&mut self, input_of: crate::IrqInputOf) {
        self.irq_input_of = Some(input_of);
    }

    /// Move the CPU's delivery records into the tap; nothing without one.
    pub(crate) fn read_deliveries(&mut self) {
        let Some(tap) = self.deliveries.as_deref_mut() else {
            return;
        };
        tap.buffer.clear();
        let Some(counts) = self.cpu.drain_deliveries(&mut tap.buffer) else {
            return;
        };
        if let Some(input_of) = &self.irq_input_of {
            for record in &mut tap.buffer {
                if record.kind == cpu::DeliveryKind::Irq {
                    record.irq = input_of(&self.bus, record.vector);
                }
            }
        }
        tap.absorb(counts);
    }

    /// The last deliveries and the counts, when
    /// [`Self::enable_delivery_log`] enabled them.
    pub fn recent_deliveries(&self) -> Option<RecentDeliveries> {
        let tap = self.deliveries.as_deref()?;
        Some(RecentDeliveries {
            records: tap.recent.as_ref()?.iter().copied().collect(),
            total: tap.total,
            dropped: tap.dropped,
        })
    }

    /// The trace text written since the last take; `None` while the trace
    /// is off.
    pub fn take_delivery_trace(&mut self) -> Option<String> {
        Some(self.deliveries.as_deref_mut()?.trace.as_mut()?.take())
    }

    /// The trace's summary so far; `None` while the trace is off.
    pub fn delivery_trace_summary(&self) -> Option<String> {
        Some(self.deliveries.as_deref()?.trace.as_ref()?.summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpu::{diagnostic::Peek, DeliveryKind};

    fn record(retired: u64, kind: DeliveryKind, vector: u8) -> Delivery {
        Delivery {
            retired,
            kind,
            vector,
            irq: None,
            cs: 0xF000,
            ip: 0x10,
            linear: 0xF0010,
            error_code: None,
            cpl_before: 0,
            cpl_after: 0,
            bytes: [Peek::Value(0x90); cpu::delivery::DELIVERY_BYTES],
        }
    }

    /// A trace started after the report's log numbers its deliveries from
    /// its own start: the first three INT 21h go to the report only, the
    /// trace writes a line for each of the next three and counts three in
    /// its summary, while the report keeps all six; the run is the same as
    /// a twin's without either.
    #[test]
    fn a_trace_enabled_after_the_report_log_counts_from_its_own_start() {
        use crate::{Bus, Machine, RunOutcome};
        use cpu::{x86jit_backend::RealSegments, Cpu, X86JitCpu};
        let program = [
            0xCD, 0x21, 0xCD, 0x21, 0xCD, 0x21, // 1000 INT 21h x3
            0xCD, 0x21, 0xCD, 0x21, 0xCD, 0x21, // 1006 INT 21h x3
            0xFA, 0xF4, // 100C CLI; HLT
        ];
        let run = |diagnose: bool| {
            let mut cpu =
                X86JitCpu::new_i286_bare(0x100_0000, 0, 0x1000, RealSegments::flat(0)).unwrap();
            cpu.map_ram(0, 0x10000).unwrap();
            cpu.write_mem(0x1000, &program).unwrap();
            cpu.write_mem(0x21 * 4, &[0x00, 0x18, 0, 0]).unwrap();
            cpu.write_mem(0x1800, &[0xCF]).unwrap(); // IRET
            let bus = Bus::new(Vec::new(), bus::DecodeMap::default());
            let mut machine =
                Machine::new(cpu, bus, Box::new(crate::tests::NoIc), Vec::new(), None);
            if diagnose {
                machine.enable_delivery_log().unwrap();
            }
            // Three INT 21h and their IRETs.
            assert_eq!(
                machine.run_bounded(Some(6)).unwrap(),
                RunOutcome::RetiredCapReached
            );
            if diagnose {
                machine.enable_delivery_trace().unwrap();
            }
            assert_eq!(machine.run_bounded(Some(1000)).unwrap(), RunOutcome::Halted);
            (machine.cpu().retired(), machine.virtual_time(), machine)
        };
        let (retired, time, mut machine) = run(true);
        let (plain_retired, plain_time, _) = run(false);
        assert_eq!((retired, time), (plain_retired, plain_time));
        let trace = machine.take_delivery_trace().unwrap();
        let lines: Vec<&str> = trace.lines().collect();
        assert_eq!(lines.len(), 3, "{trace}");
        assert!(lines[0].starts_with("delivery 7 software INT 21h vector 21 at 0000:1008 "));
        assert!(lines[2].starts_with("delivery 11 software INT 21h vector 21 at 0000:100c "));
        assert_eq!(
            machine.delivery_trace_summary().unwrap(),
            "summary  3 deliveries, 3 lines written, 0 dropped before they were read\n\
             summary  INT 21h: 3\n"
        );
        let recent = machine.recent_deliveries().unwrap();
        assert_eq!(
            (recent.records.len(), recent.total, recent.dropped),
            (6, 6, 0)
        );
    }

    /// Lines for the first deliveries only, then a note, a count line at
    /// every mark, marks among dropped deliveries too, and a summary that
    /// counts what was read per name.
    #[test]
    fn the_trace_writes_the_first_lines_then_counts() {
        let mut trace = DeliveryTrace::with_bounds(3, 4);
        let batch: Vec<Delivery> = (1..=5)
            .map(|n| record(n * 10, DeliveryKind::Fault, 6))
            .collect();
        trace.record(
            &batch,
            DeliveryCounts {
                dropped: 0,
                total: 5,
            },
        );
        let text = trace.take();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert!(lines[0].starts_with("delivery 10 fault #UD vector 06 at f000:0010 "));
        assert!(lines[2].starts_with("delivery 30 fault #UD "));
        assert_eq!(
            lines[3],
            "trace    3 delivery lines written; from here a count line every 4 deliveries"
        );
        assert_eq!(lines[4], "count    4 deliveries, the last at retired 40");
        assert!(trace.take().is_empty());
        // Deliveries 6 to 9 were dropped, so mark 8 has no record; 10 to 12
        // are read, 12 is a mark.
        let batch: Vec<Delivery> = (10..=12)
            .map(|n| record(n * 10, DeliveryKind::Software, 0x21))
            .collect();
        trace.record(
            &batch,
            DeliveryCounts {
                dropped: 4,
                total: 12,
            },
        );
        assert_eq!(
            trace.take(),
            "dropped  4 deliveries (the CPU's log overflowed before the machine read it)\n\
             count    8 deliveries (the delivery itself was dropped)\n\
             count    12 deliveries, the last at retired 120\n"
        );
        assert_eq!(
            trace.summary(),
            "summary  12 deliveries, 3 lines written, 4 dropped before they were read\n\
             summary  #UD: 5\n\
             summary  INT 21h: 3\n"
        );
    }
}
