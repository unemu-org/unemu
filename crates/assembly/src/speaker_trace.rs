// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in host diagnostic: PC-speaker starts, tone changes and CPU resets,
//! each stamped with the guest virtual time, retired count and CS:IP.
//!
//! IBM Personal Computer AT Technical Reference, System Board, Speaker: the
//! speaker sounds counter 2 while port 61h bit 0 (GATE) and bit 1 (data) are
//! both set; Keyboard Controller Output Port: bit 0 resets the CPU, and CMOS
//! byte 0Fh tells the BIOS how to resume. Intel 8254 datasheet, Mode 3: the
//! count register divides the timer input into the tone.
//!
//! The trace reads timer, CMOS and CPU state only after a guest port write
//! and around a frontend CPU reset. It never writes guest state, so execution,
//! timing and PCM are the same with it on or off. Assembly builds it only when
//! [`crate::ObservationPolicy::speaker_trace`] asks for it. See
//! `backlog/docs/design/audio.md` §PC speaker.

use std::{
    cell::{Cell, RefCell},
    fmt,
    rc::Rc,
};

use cpu::{x86jit_backend::X86JitCpu, Cpu, CpuReg};
use device::{
    clock::{PIT_HZ, TICKS_PER_PIT_CLOCK},
    VClock,
};
use devices::{
    cmos_rtc::NvramStore,
    pit8253::{SharedPit8253, SpeakerControls},
};

/// Lines of one kind printed per virtual second before the rest are counted.
/// Digitised speaker sound and memory moves through CPU resets would
/// otherwise write thousands of lines a second.
const LINE_BUDGET: u32 = 32;

/// Where the guest was when a traced event happened.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GuestStamp {
    /// Virtual time in board ticks.
    pub at: VClock,
    /// Instructions the CPU had retired.
    pub retired: u64,
    /// Code segment register (a selector in protected mode).
    pub cs: u16,
    /// Instruction pointer: after a port write, the instruction following it.
    pub ip: u16,
}

impl GuestStamp {
    fn read(cpu: &X86JitCpu, at: VClock) -> Self {
        Self {
            at,
            retired: cpu.retired(),
            cs: cpu.code_segment() as u16,
            ip: cpu.reg(CpuReg::Rip) as u16,
        }
    }
}

/// What started a CPU reset.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResetSource {
    /// A guest port write pulsed the reset line: the 8042 output port or
    /// system control port A.
    Pulse,
    /// The CPU shut down without a pulse, for example after a triple fault.
    Shutdown,
    /// The frontend restarted a machine that halted with interrupts off.
    Host,
}

/// Why a frontend resets the CPU through [`crate::AssembledMachine::reset_cpu`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResetCause {
    /// The run stopped with `RunOutcome::ResetRequested`.
    Requested,
    /// The host restarts a machine that halted with interrupts off.
    Host,
}

/// One traced event; [`fmt::Display`] gives its log line.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SpeakerTraceEvent {
    /// GATE and speaker data became both set: the speaker sounds counter 2.
    On {
        /// The port write that completed the enable.
        stamp: GuestStamp,
        /// Counter 2's mode.
        mode: u8,
        /// Counter 2's divisor, 1 to 65536.
        divisor: u32,
    },
    /// Counter 2 was reprogrammed while the speaker was on.
    Tone {
        /// The port write that completed the new count or mode.
        stamp: GuestStamp,
        /// Counter 2's mode.
        mode: u8,
        /// Counter 2's divisor, 1 to 65536.
        divisor: u32,
    },
    /// GATE or speaker data went low again.
    Off {
        /// The port write that disabled the speaker.
        stamp: GuestStamp,
        /// How long the speaker was on, in board ticks.
        after: u64,
    },
    /// The CPU was reset.
    Reset {
        /// Where the guest stopped before the reset.
        stamp: GuestStamp,
        /// What started it.
        source: ResetSource,
        /// CMOS byte 0Fh at the reset; absent without a CMOS RTC.
        shutdown: Option<u8>,
        /// Code segment the CPU restarts from.
        restart_cs: u16,
        /// Instruction pointer the CPU restarts from.
        restart_ip: u16,
    },
    /// Lines of one virtual second left out to bound the log.
    Suppressed {
        /// The virtual second, counted from power-on.
        second: u64,
        /// Speaker lines left out.
        speaker: u64,
        /// Resets with a resume shutdown byte left out.
        resets: u64,
    },
}

/// Microseconds of virtual time; display only, never fed back to the machine.
fn micros(ticks: u64) -> u128 {
    u128::from(ticks) * u128::from(PIT_HZ.denominator()) * 1_000_000
        / (u128::from(PIT_HZ.numerator()) * u128::from(TICKS_PER_PIT_CLOCK))
}

struct Seconds(u64);

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let micros = micros(self.0);
        write!(f, "{}.{:06}s", micros / 1_000_000, micros % 1_000_000)
    }
}

impl fmt::Display for GuestStamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "t={} clock={} retired={} cs:ip={:04X}:{:04X}",
            Seconds(self.at.0),
            self.at.0,
            self.retired,
            self.cs,
            self.ip
        )
    }
}

struct Tone {
    mode: u8,
    divisor: u32,
}

impl fmt::Display for Tone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let decihertz = u128::from(PIT_HZ.numerator()) * 10
            / (u128::from(PIT_HZ.denominator()) * u128::from(self.divisor));
        write!(
            f,
            "mode {} divisor {} ({}.{} Hz)",
            self.mode,
            self.divisor,
            decihertz / 10,
            decihertz % 10
        )
    }
}

/// The resume codes the open AT BIOS honours after a reset; any other value
/// makes it run POST.
fn resumes(shutdown: Option<u8>) -> bool {
    matches!(shutdown, Some(0x05 | 0x09 | 0x0A))
}

impl fmt::Display for SpeakerTraceEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::On {
                stamp,
                mode,
                divisor,
            } => write!(f, "speaker on    {stamp} {}", Tone { mode, divisor }),
            Self::Tone {
                stamp,
                mode,
                divisor,
            } => write!(f, "speaker tone  {stamp} {}", Tone { mode, divisor }),
            Self::Off { stamp, after } => {
                write!(f, "speaker off   {stamp} after {}", Seconds(after))
            }
            Self::Reset {
                stamp,
                source,
                shutdown,
                restart_cs,
                restart_ip,
            } => {
                let source = match source {
                    ResetSource::Pulse => "reset-pulse",
                    ResetSource::Shutdown => "cpu-shutdown",
                    ResetSource::Host => "host",
                };
                write!(f, "cpu reset     {stamp} source={source} shutdown=")?;
                match shutdown {
                    Some(byte) => write!(f, "{byte:02X}h")?,
                    None => f.write_str("none")?,
                }
                write!(f, " restart={restart_cs:04X}:{restart_ip:04X}")
            }
            Self::Suppressed {
                second,
                speaker,
                resets,
            } => write!(
                f,
                "speaker trace second {second}: left out {speaker} speaker lines and {resets} resume resets"
            ),
        }
    }
}

struct State {
    pit: SharedPit8253,
    reset_line: Option<Rc<Cell<bool>>>,
    cmos: Option<NvramStore>,
    last: SpeakerControls,
    on_since: VClock,
    pulsed: bool,
    events: Vec<SpeakerTraceEvent>,
    second: u64,
    printed: [u32; 2],
    left_out: [u64; 2],
}

impl State {
    fn push(&mut self, event: SpeakerTraceEvent, budget: Option<usize>, at: VClock) {
        self.roll(at);
        if let Some(kind) = budget {
            if self.printed[kind] >= LINE_BUDGET {
                self.left_out[kind] += 1;
                return;
            }
            self.printed[kind] += 1;
        }
        self.events.push(event);
    }

    /// Report the previous second's left-out lines once time has moved on.
    fn roll(&mut self, at: VClock) {
        let second = (micros(at.0) / 1_000_000) as u64;
        if second == self.second {
            return;
        }
        if self.left_out != [0; 2] {
            self.events.push(SpeakerTraceEvent::Suppressed {
                second: self.second,
                speaker: self.left_out[0],
                resets: self.left_out[1],
            });
        }
        self.second = second;
        self.printed = [0; 2];
        self.left_out = [0; 2];
    }
}

/// Shared trace state: the CPU-memory hook records events, the frontend drains them.
#[derive(Clone)]
pub(crate) struct SpeakerTrace(Rc<RefCell<State>>);

impl SpeakerTrace {
    pub(crate) fn new(
        pit: SharedPit8253,
        reset_line: Option<Rc<Cell<bool>>>,
        cmos: Option<NvramStore>,
    ) -> Self {
        let last = pit.speaker_controls();
        Self(Rc::new(RefCell::new(State {
            pit,
            reset_line,
            cmos,
            last,
            on_since: last.at,
            pulsed: false,
            events: Vec::new(),
            second: 0,
            printed: [0; 2],
            left_out: [0; 2],
        })))
    }

    /// Compare the speaker inputs after a guest port write; the CPU stands on
    /// the instruction following that write.
    pub(crate) fn after_port_write(&self, cpu: &X86JitCpu) {
        let mut state = self.0.borrow_mut();
        // The machine consumes the line before the next instruction, so a
        // pulse is still visible right after the write that caused it.
        if state.reset_line.as_ref().is_some_and(|line| line.get()) {
            state.pulsed = true;
        }
        let now = state.pit.speaker_controls();
        let last = std::mem::replace(&mut state.last, now);
        let was_on = last.gate && last.data;
        let is_on = now.gate && now.data;
        let divisor = if now.divisor == 0 {
            65536
        } else {
            u32::from(now.divisor)
        };
        let stamp = || GuestStamp::read(cpu, now.at);
        let event = match (was_on, is_on) {
            (false, true) => {
                state.on_since = now.at;
                SpeakerTraceEvent::On {
                    stamp: stamp(),
                    mode: now.mode,
                    divisor,
                }
            }
            (true, true) if (last.mode, last.divisor) != (now.mode, now.divisor) => {
                SpeakerTraceEvent::Tone {
                    stamp: stamp(),
                    mode: now.mode,
                    divisor,
                }
            }
            (true, false) => SpeakerTraceEvent::Off {
                stamp: stamp(),
                after: now.at.0.saturating_sub(state.on_since.0),
            },
            _ => return,
        };
        state.push(event, Some(0), now.at);
    }

    /// Record a CPU reset; `before` is where the guest stopped and the CPU now
    /// stands at its restart address.
    pub(crate) fn reset(&self, cause: ResetCause, before: GuestStamp, cpu: &X86JitCpu) {
        let mut state = self.0.borrow_mut();
        let source = match cause {
            ResetCause::Host => ResetSource::Host,
            ResetCause::Requested if state.pulsed => ResetSource::Pulse,
            ResetCause::Requested => ResetSource::Shutdown,
        };
        state.pulsed = false;
        // General RAM starts at CMOS byte 0Eh, so 0Fh is its second byte.
        let shutdown = state.cmos.as_ref().map(|cmos| cmos.snapshot()[1]);
        let event = SpeakerTraceEvent::Reset {
            stamp: before,
            source,
            shutdown,
            restart_cs: cpu.code_segment() as u16,
            restart_ip: cpu.reg(CpuReg::Rip) as u16,
        };
        let budget = (source != ResetSource::Host && resumes(shutdown)).then_some(1);
        state.push(event, budget, before.at);
    }

    /// Take the recorded events, closing a finished second's summary first.
    pub(crate) fn take(&self, now: VClock) -> Vec<SpeakerTraceEvent> {
        let mut state = self.0.borrow_mut();
        state.roll(now);
        std::mem::take(&mut state.events)
    }

    pub(crate) fn stamp(cpu: &X86JitCpu, at: VClock) -> GuestStamp {
        GuestStamp::read(cpu, at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(at: u64, retired: u64, cs: u16, ip: u16) -> GuestStamp {
        GuestStamp {
            at: VClock(at),
            retired,
            cs,
            ip,
        }
    }

    /// Intel 8254 datasheet, Mode 3: the tone is the timer input divided by
    /// the count; 1193 is the BIOS beep and 2153 the power-on melody's first note.
    #[test]
    fn trace_lines_name_time_place_tone_and_reset_source() {
        let lines = [
            SpeakerTraceEvent::On {
                stamp: stamp(1_193_182, 4_000_000, 0xF000, 0xE1A2),
                mode: 3,
                divisor: 2153,
            },
            SpeakerTraceEvent::Tone {
                stamp: stamp(1_193_200, 4_000_010, 0x1234, 0x0100),
                mode: 3,
                divisor: 65536,
            },
            SpeakerTraceEvent::Off {
                stamp: stamp(1_412_000, 4_100_000, 0xF000, 0xE1B0),
                after: 219_546,
            },
            SpeakerTraceEvent::Reset {
                stamp: stamp(23, 9, 0xF000, 0x0F2E),
                source: ResetSource::Pulse,
                shutdown: Some(0x09),
                restart_cs: 0xF000,
                restart_ip: 0xFFF0,
            },
            SpeakerTraceEvent::Reset {
                stamp: stamp(0, 0, 0, 0),
                source: ResetSource::Shutdown,
                shutdown: None,
                restart_cs: 0xF000,
                restart_ip: 0xFFF0,
            },
            SpeakerTraceEvent::Reset {
                stamp: stamp(5, 2, 0xF000, 0x1000),
                source: ResetSource::Host,
                shutdown: Some(0),
                restart_cs: 0xF000,
                restart_ip: 0xFFF0,
            },
            SpeakerTraceEvent::Suppressed {
                second: 12,
                speaker: 340,
                resets: 25,
            },
        ]
        .map(|event| event.to_string());
        assert_eq!(
            lines,
            [
                "speaker on    t=1.000000s clock=1193182 retired=4000000 cs:ip=F000:E1A2 mode 3 divisor 2153 (554.1 Hz)",
                "speaker tone  t=1.000015s clock=1193200 retired=4000010 cs:ip=1234:0100 mode 3 divisor 65536 (18.2 Hz)",
                "speaker off   t=1.183390s clock=1412000 retired=4100000 cs:ip=F000:E1B0 after 0.184000s",
                "cpu reset     t=0.000019s clock=23 retired=9 cs:ip=F000:0F2E source=reset-pulse shutdown=09h restart=F000:FFF0",
                "cpu reset     t=0.000000s clock=0 retired=0 cs:ip=0000:0000 source=cpu-shutdown shutdown=none restart=F000:FFF0",
                "cpu reset     t=0.000004s clock=5 retired=2 cs:ip=F000:1000 source=host shutdown=00h restart=F000:FFF0",
                "speaker trace second 12: left out 340 speaker lines and 25 resume resets",
            ]
        );
    }

    /// A tone (counter 2 mode 3), a new count while it sounds, the speaker
    /// off, CMOS shutdown byte 09h, then the 8042 FEh reset pulse.
    fn speaker_machine(trace: bool) -> crate::AssembledMachine {
        let program = [
            0xFA, 0xB0, 0xB6, 0xE6, 0x43, 0xB0, 0xA9, 0xE6, 0x42, 0xB0, 0x04, 0xE6, 0x42, 0xB0,
            0x03, 0xE6, 0x61, 0xB0, 0x2E, 0xE6, 0x42, 0xB0, 0x08, 0xE6, 0x42, 0xB0, 0x00, 0xE6,
            0x61, 0xB0, 0x8F, 0xE6, 0x70, 0xB0, 0x09, 0xE6, 0x71, 0xB0, 0xFE, 0xE6, 0x64, 0xEB,
            0xFE,
        ];
        let mut rom = vec![0u8; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        let mut config = crate::bios(rom, None);
        config.cpu.reset = crate::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut policy = crate::ObservationPolicy {
            speaker_trace: trace,
            ..crate::ObservationPolicy::default()
        };
        policy.output.audio_sink = crate::AudioSinkKind::Capture;
        policy.output.audio_capture_frame_limit = 4096;
        let mut assembled = crate::assemble_with_observation_and_options(
            &config,
            &policy,
            crate::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        assert_eq!(
            assembled.machine.run_bounded(Some(10_000)).unwrap(),
            ::machine::RunOutcome::ResetRequested
        );
        assembled.capture_audio().unwrap();
        assembled
    }

    #[test]
    fn trace_is_off_by_default_and_never_changes_the_run() {
        assert!(!crate::ObservationPolicy::default().speaker_trace);
        let mut off = speaker_machine(false);
        let mut on = speaker_machine(true);
        assert!(!off.observation.speaker_trace_enabled());
        assert!(on.observation.speaker_trace_enabled());
        for host in [&mut off, &mut on] {
            host.reset_cpu(ResetCause::Requested).unwrap();
            host.machine.run_bounded(Some(20_000)).unwrap();
            host.capture_audio().unwrap();
        }
        assert!(off
            .observation
            .take_speaker_trace(off.machine.clock())
            .is_empty());
        assert_eq!(off.machine.clock(), on.machine.clock());
        assert_eq!(off.machine.cpu().retired(), on.machine.cpu().retired());
        let pcm = |host: &crate::AssembledMachine| {
            let audio = host.observation.captured_audio().unwrap();
            (audio.frame_count(), audio.hash())
        };
        assert_eq!(pcm(&off), pcm(&on));
        let audio = on.observation.captured_audio().unwrap();
        assert!(
            audio.frames().iter().any(|frame| frame[0] > 0),
            "the tone was rendered"
        );
    }

    /// IBM Personal Computer AT Technical Reference, System Board, Speaker and
    /// Keyboard Controller Output Port: each line carries the CS:IP after the
    /// OUT that caused it and the reset carries CMOS byte 0Fh.
    #[test]
    fn trace_stamps_speaker_writes_and_classifies_resets() {
        let mut assembled = speaker_machine(true);
        assembled.reset_cpu(ResetCause::Requested).unwrap();
        let restart = (
            assembled.machine.cpu().code_segment() as u16,
            assembled.machine.cpu().reg(CpuReg::Rip) as u16,
        );
        // No pulse since the last reset: the CPU shut down on its own.
        assembled.reset_cpu(ResetCause::Requested).unwrap();
        assembled.reset_cpu(ResetCause::Host).unwrap();
        let events = assembled
            .observation
            .take_speaker_trace(assembled.machine.clock());
        let place = |event: &SpeakerTraceEvent| match *event {
            SpeakerTraceEvent::On { stamp, .. }
            | SpeakerTraceEvent::Tone { stamp, .. }
            | SpeakerTraceEvent::Off { stamp, .. }
            | SpeakerTraceEvent::Reset { stamp, .. } => (stamp.cs, stamp.ip),
            SpeakerTraceEvent::Suppressed { .. } => (0, 0),
        };
        assert_eq!(
            events.iter().map(place).collect::<Vec<_>>(),
            [
                (0xF800, 0x11),
                (0xF800, 0x19),
                (0xF800, 0x1D),
                (0xF800, 0x29),
                restart,
                restart
            ]
        );
        assert!(matches!(
            events[0],
            SpeakerTraceEvent::On {
                mode: 3,
                divisor: 0x04A9,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            SpeakerTraceEvent::Tone {
                mode: 3,
                divisor: 0x082E,
                ..
            }
        ));
        let SpeakerTraceEvent::Off { stamp, after } = events[2] else {
            panic!("{:?}", events[2]);
        };
        let SpeakerTraceEvent::On { stamp: on, .. } = events[0] else {
            unreachable!()
        };
        assert_eq!(after, stamp.at.0 - on.at.0);
        assert!(stamp.retired > on.retired);
        let resets: Vec<_> = events[3..]
            .iter()
            .map(|event| match *event {
                SpeakerTraceEvent::Reset {
                    source,
                    shutdown,
                    restart_cs,
                    restart_ip,
                    ..
                } => (source, shutdown, (restart_cs, restart_ip)),
                _ => panic!("{event:?}"),
            })
            .collect();
        assert_eq!(
            resets,
            [
                (ResetSource::Pulse, Some(0x09), restart),
                (ResetSource::Shutdown, Some(0x09), restart),
                (ResetSource::Host, Some(0x09), restart),
            ]
        );
    }

    #[test]
    fn budget_counts_left_out_lines_and_reports_them_once_the_second_ends() {
        let pit = SharedPit8253::default();
        let trace = SpeakerTrace::new(pit, None, None);
        let off = |at| SpeakerTraceEvent::Off {
            stamp: stamp(at, 0, 0, 0),
            after: 1,
        };
        {
            let mut state = trace.0.borrow_mut();
            for at in 0..40 {
                state.push(off(at), Some(0), VClock(at));
            }
            // Non-resume resets always print.
            state.push(off(41), None, VClock(41));
        }
        let first = trace.take(VClock(50));
        assert_eq!(first.len(), LINE_BUDGET as usize + 1);
        let next_second = 1_193_182;
        assert_eq!(
            trace.take(VClock(next_second)),
            [SpeakerTraceEvent::Suppressed {
                second: 0,
                speaker: 8,
                resets: 0,
            }]
        );
        assert!(trace.take(VClock(next_second + 1)).is_empty());
    }
}
