// SPDX-License-Identifier: AGPL-3.0-or-later
//! Run policy: *how* a machine is driven, separated from *what* it is (§3.1).
//!
//! [`MachineConfig`](crate::MachineConfig) describes the machine — its CPU, clock,
//! memory, and devices. It says nothing about the run: how long to let it go,
//! when to stop, and what to feed it. That is a [`RunPolicy`], applied by the one
//! [`run`] driver here. Splitting the two keeps machine identity ("what an AT is")
//! apart from run identity ("boot to the shell prompt, then type `DIR`").
//!
//! ## What belongs to the machine vs. the run
//!
//! The clock ratio belongs to the machine. The idle limit, step cap, stop
//! condition, and stimuli belong to the run. By contract, cooperation size does
//! not affect instruction boundaries or interrupt acceptance.
//!
//! ## The cadence is part of the golden identity
//!
//! A [`RunPolicy`] is deliberately deterministic and derives [`PartialEq`]/[`Eq`]:
//! the poll step at which [`StopCondition::PromptThen`] checks the console, and
//! the schedule on which [`KeyboardStimulus`]s are injected, decide *which*
//! interrupts land and thus the observable output. A golden trace is valid only
//! against a fixed policy cadence, exactly as it is valid only against a pinned
//! CPU revision. This is not a free tuning knob.

use cpu::x86jit_backend::X86JitCpu;
use devices::I8042;
use machine::{Machine, MachineError, RunOutcome};
use video::{render_text_mode, TextModeState};

/// Policy validation failures and hardware execution errors while driving a run.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Hardware execution failed.
    #[error(transparent)]
    Machine(#[from] MachineError),
    /// The prompt left too little room for the complete scripted command.
    #[error("insufficient command budget: need {required} steps, only {remaining} remain")]
    InsufficientCommandBudget {
        /// Requested post-prompt steps.
        required: u64,
        /// Steps remaining before the overall cap.
        remaining: u64,
    },
}

/// How to read a text console back out of guest memory to detect a prompt (§9.2).
///
/// The prompt-detection path reads a `cols × rows` two-bytes-per-cell text buffer
/// starting at guest linear address `base` and renders it exactly as the frontend
/// renders that console, so a needle match here is identical to one the caller
/// would see. Plain data (no device handle), so a [`RunPolicy`] stays comparable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleProbe {
    /// Guest linear address of the text buffer (e.g. the MDA window at `0xB0000`).
    pub base: u64,
    /// Visible columns of the console (80 for the BIOS MDA console).
    pub cols: u16,
    /// Visible rows of the console (25 for the BIOS MDA console).
    pub rows: u16,
}

/// When a [`KeyboardStimulus`] is delivered relative to the run (§7.5).
///
/// One variant today: keys wait until the guest owns the keyboard. Injected at
/// power-on they are eaten by the BIOS keyboard-reset handshake and the POST
/// buffer flush; delivered once the shell blocks at its prompt they reach INT 09h
/// cleanly. Kept an enum so a future schedule (a fixed retired count, say) is a
/// variant, not a re-architecture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InjectionSchedule {
    /// Deliver once [`StopCondition::PromptThen`]'s needle appears on the console.
    AfterPrompt,
}

/// A keyboard script fed into the machine on a schedule (§7.5).
///
/// This is the mechanism the interactive FreeDOS run drives the shell with: a set
/// of set-1 (XT) make/break scancodes queued into the 8042 at `keyboard_index`
/// (the caller resolves the controller's [`DeviceId`](crate::DeviceId) to its bus
/// index through the assembler's device-index map). The 8042 delivers them
/// serially, one clean IRQ1 edge per byte. Plain-data stimuli keep the policy
/// comparable as part of the golden identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardStimulus {
    /// Bus index of the 8042 keyboard controller (resolved from its `DeviceId`).
    pub keyboard_index: usize,
    /// The set-1 make/break scancode bytes to queue, in order.
    pub scancodes: Vec<u8>,
    /// When to deliver them.
    pub schedule: InjectionSchedule,
}

/// Why a policy-driven run stops (§3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopCondition {
    /// Run until the guest halts and its interrupt source falls quiescent (or the
    /// retired cap, whichever comes first). The plain machines — a POST/BIOS boot,
    /// the demos — stop this way.
    Natural,
    /// Boot to a shell prompt, then optionally act. Step the machine in
    /// `poll_step` virtual-time chunks, growing a cumulative cap, until
    /// `needle` appears on the `console` (or a halt, error, or the overall cap). This
    /// is the FreeCOM date-prompt shape: a headless shell busy-polls the keyboard
    /// forever, so running to the cap would spin billions of idle instructions
    /// producing no new output — polling to the prompt captures the same screen in
    /// a fraction of the time.
    ///
    /// If the policy carries [`InjectionSchedule::AfterPrompt`] stimuli, they are
    /// queued once the prompt is reached and the machine then runs `then_budget`
    /// more instruction-equivalent units so the shell processes them; with no
    /// stimuli the run stops at the prompt and `then_budget` is unused. If the
    /// overall cap cannot accommodate the full budget, return an error before
    /// injecting any keys.
    PromptThen {
        /// Where to read the console text back from guest memory.
        console: ConsoleProbe,
        /// The substring that marks the shell has reached its prompt.
        needle: String,
        /// Virtual-time step between prompt-detection polls (instruction equivalents).
        poll_step: u64,
        /// Virtual-time budget granted after stimuli fire (unused when no
        /// stimuli are scheduled).
        then_budget: u64,
    },
}

/// How to run a machine, apart from what the machine is (§3.1).
///
/// Every field describes how to drive the machine. `retired_cap` is the total
/// virtual-time safety net; `idle_limit` is the halted PIT-clock quiescence bound
/// (stamped onto the machine by [`run`],
/// not baked in at construction); `stop` chooses the stop shape; `stimuli` feeds
/// the guest on a schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPolicy {
    /// Total virtual-time safety cap in instruction equivalents (`None` = no cap).
    pub retired_cap: Option<u64>,
    /// Consecutive halted PIT clocks with no injection before stopping; resets
    /// on each wake-up. `None` waits no clocks and resumes HLT only for an
    /// immediately deliverable interrupt. IF clear always stops immediately.
    pub idle_limit: Option<u64>,
    /// When the run stops.
    pub stop: StopCondition,
    /// External stimuli injected on their schedule (empty for a passive run).
    pub stimuli: Vec<KeyboardStimulus>,
}

impl RunPolicy {
    /// A plain run to a natural stop, capped and idle-bounded — the policy the
    /// POST/BIOS boots and the demos use.
    #[must_use]
    pub fn natural(retired_cap: Option<u64>, idle_limit: Option<u64>) -> Self {
        Self {
            retired_cap,
            idle_limit,
            stop: StopCondition::Natural,
            stimuli: Vec::new(),
        }
    }
}

/// Drive `machine` under `policy`: apply the idle limit, then run to the policy's
/// stop condition, injecting any scheduled stimuli (§3.1).
///
/// The single run-loop driver above [`Machine::run_bounded`]. It stamps the
/// policy's `idle_limit` onto the machine, then either runs straight to the
/// virtual-time cap ([`StopCondition::Natural`]) or
/// polls to a prompt and optionally feeds the guest ([`StopCondition::PromptThen`]).
/// Returns the [`RunOutcome`] of the final [`Machine::run_bounded`] so the caller
/// classifies the stop.
///
/// This driver uses the assembler's concrete CPU because its console probe reads
/// guest memory through `X86JitCpu::read_mem`. That backend is also the only
/// production implementation of the step-bounded CPU contract.
pub fn run(machine: &mut Machine<X86JitCpu>, policy: &RunPolicy) -> Result<RunOutcome, RunError> {
    machine.set_idle_limit(policy.idle_limit);

    match &policy.stop {
        StopCondition::Natural => machine.run_bounded(policy.retired_cap).map_err(Into::into),
        StopCondition::PromptThen {
            console,
            needle,
            poll_step,
            then_budget,
        } => {
            let result = poll_to_prompt(machine, console, needle, *poll_step, policy.retired_cap);

            // Act on the prompt only if we actually reached it (not an error, not a
            // spin to the cap) and something is scheduled to fire there. With no
            // stimuli the run stops at the prompt — the plain FreeDOS boot.
            let prompt_reached = result.is_ok() && console_contains(machine, console, needle);
            let after_prompt: Vec<&KeyboardStimulus> = policy
                .stimuli
                .iter()
                .filter(|s| matches!(s.schedule, InjectionSchedule::AfterPrompt))
                .collect();
            if prompt_reached && !after_prompt.is_empty() {
                let now = machine.virtual_time();
                let remaining = policy
                    .retired_cap
                    .map_or(u64::MAX - now, |cap| cap.saturating_sub(now));
                if remaining < *then_budget {
                    return Err(RunError::InsufficientCommandBudget {
                        required: *then_budget,
                        remaining,
                    });
                }
                for stim in after_prompt {
                    inject_scancodes(machine, stim.keyboard_index, &stim.scancodes);
                }
                return machine
                    .run_bounded(Some(now + *then_budget))
                    .map_err(Into::into);
            }
            result.map_err(Into::into)
        }
    }
}

/// Step the machine in `poll_step` virtual-time chunks until `needle` appears on
/// the console, the guest halts, an error occurs, or the overall cap is reached.
///
/// This is the "boot to the shell prompt" loop: a headless FreeCOM busy-polls the
/// keyboard at its date prompt forever, so rather than run to the cap the driver
/// grows the cap one chunk at a time and stops the moment the prompt shows. The
/// returned outcome is that of the last [`Machine::run_bounded`] — typically
/// [`RunOutcome::RetiredCapReached`], since the busy-polling guest never halts.
fn poll_to_prompt(
    machine: &mut Machine<X86JitCpu>,
    console: &ConsoleProbe,
    needle: &str,
    poll_step: u64,
    retired_cap: Option<u64>,
) -> Result<RunOutcome, MachineError> {
    loop {
        let target = machine.virtual_time().saturating_add(poll_step);
        let cap = retired_cap.map_or(target, |hard| target.min(hard));
        let r = machine.run_bounded(Some(cap));
        let hit_cap = retired_cap.is_some_and(|hard| cap >= hard);
        if matches!(r, Err(_) | Ok(RunOutcome::Halted))
            || console_contains(machine, console, needle)
            || hit_cap
        {
            break r;
        }
    }
}

/// Whether the text console described by `probe` currently holds `needle`.
///
/// Reads the `cols × rows` two-bytes-per-cell buffer at `probe.base` out of guest
/// memory and renders it through the same [`render_text_mode`] path the frontend
/// uses, then searches the newline-joined rows — so a match here is identical to
/// one the frontend's console render would report. A short read leaves the tail
/// blank rather than failing.
fn console_contains(machine: &Machine<X86JitCpu>, probe: &ConsoleProbe, needle: &str) -> bool {
    let bytes = usize::from(probe.cols) * usize::from(probe.rows) * 2;
    let mut vram = vec![0u8; bytes];
    let _ = machine.cpu().read_mem(probe.base, &mut vram);
    let grid = render_text_mode(&TextModeState {
        vram: &vram,
        cols: probe.cols,
        rows: probe.rows,
        start_cell: 0,
        cursor_cell: 0,
        cursor_visible: false,
        attributes: video::TextAttributeSet::Colour {
            blink_enabled: true,
        },
    });
    grid.rows_as_strings().join("\n").contains(needle)
}

/// Queue a keyboard `scancodes` script into the 8042 at bus index `kbc_index`,
/// reaching the concrete controller through the `Device::as_any_mut` downcast
/// seam — the machine core still sees only `&mut dyn Device`.
pub fn inject_scancodes(machine: &mut Machine<X86JitCpu>, kbc_index: usize, scancodes: &[u8]) {
    if let Some(dev) = machine.bus_mut().device_mut(kbc_index) {
        if let Some(kbc) = dev.as_any_mut().and_then(|a| a.downcast_mut::<I8042>()) {
            for &scancode in scancodes {
                kbc.push_scancode(scancode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::{Bus, DecodeMap};
    use cpu::x86jit_backend::RealSegments;
    use cpu::Cpu;

    #[test]
    fn late_prompt_rejects_partial_command_budget_before_injecting_keys() {
        for cap in [10, 15, 19] {
            let mut cpu =
                X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).unwrap();
            cpu.map_ram(0, 0x10000).unwrap();
            cpu.write_mem(0x7C00, &[0xEB, 0xFE]).unwrap();
            cpu.write_mem(0x1000, b">\x07").unwrap();
            let bus = Bus::new(
                vec![Box::new(I8042::new())],
                DecodeMap {
                    ports: vec![bus::PortRange {
                        ports: 0x60..=0x60,
                        device: 0,
                    }],
                    mmio: Vec::new(),
                },
            );
            let mut machine = Machine::new(
                cpu,
                bus,
                Box::new(chipset::NoInterruptController),
                Vec::new(),
                None,
            );
            let policy = RunPolicy {
                retired_cap: Some(cap),
                idle_limit: None,
                stop: StopCondition::PromptThen {
                    console: ConsoleProbe {
                        base: 0x1000,
                        cols: 1,
                        rows: 1,
                    },
                    needle: ">".into(),
                    poll_step: 10,
                    then_budget: 10,
                },
                stimuli: vec![KeyboardStimulus {
                    keyboard_index: 0,
                    scancodes: vec![0x1C],
                    schedule: InjectionSchedule::AfterPrompt,
                }],
            };
            let error = run(&mut machine, &policy).unwrap_err();
            assert!(error.to_string().contains("insufficient command budget"));
            assert_eq!(machine.virtual_time(), 10);
            assert_eq!(machine.bus_mut().port_read(0x60, 1).unwrap(), 0);
        }
    }

    #[test]
    fn prompt_poll_stops_on_if_clear_halt_with_or_without_cap() {
        for retired_cap in [Some(1000), None] {
            let mut cpu =
                X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).expect("cpu");
            cpu.map_ram(0, 0x10000).expect("ram");
            cpu.write_mem(0x7C00, &[0xFA, 0xF4]).expect("CLI; HLT");
            let mut machine = Machine::new(
                cpu,
                Bus::new(Vec::new(), DecodeMap::default()),
                Box::new(chipset::NoInterruptController),
                Vec::new(),
                None,
            );
            let policy = RunPolicy {
                retired_cap,
                idle_limit: Some(100),
                stop: StopCondition::PromptThen {
                    console: ConsoleProbe {
                        base: 0,
                        cols: 1,
                        rows: 1,
                    },
                    needle: "prompt".into(),
                    poll_step: 10,
                    then_budget: 0,
                },
                stimuli: Vec::new(),
            };
            assert_eq!(run(&mut machine, &policy).expect("run"), RunOutcome::Halted);
            assert_eq!(machine.cpu().retired(), 2);
        }
    }

    #[test]
    fn prompt_and_stimulus_budgets_start_from_current_virtual_time() {
        let mut cpu =
            X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).expect("cpu");
        cpu.map_ram(0, 0x10000).expect("ram");
        cpu.write_mem(0x7C00, &[0xFB, 0xF4]).expect("STI; HLT");
        cpu.write_mem(0x1000, b">\x07").expect("console prompt");
        let mut machine = Machine::new(
            cpu,
            Bus::new(vec![Box::new(I8042::new())], DecodeMap::default()),
            Box::new(chipset::NoInterruptController),
            Vec::new(),
            Some(100),
        );
        assert_eq!(
            machine.run_bounded(Some(5)).expect("initial chunk"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.cpu().retired(), 2);
        assert_eq!(machine.virtual_time(), 5);
        let console = ConsoleProbe {
            base: 0x1000,
            cols: 1,
            rows: 1,
        };
        assert_eq!(
            poll_to_prompt(&mut machine, &console, ">", 10, None).expect("poll"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), 15);
        let policy = RunPolicy {
            retired_cap: Some(35),
            idle_limit: Some(100),
            stop: StopCondition::PromptThen {
                console,
                needle: ">".into(),
                poll_step: 10,
                then_budget: 10,
            },
            stimuli: vec![KeyboardStimulus {
                keyboard_index: 0,
                scancodes: vec![0x1C],
                schedule: InjectionSchedule::AfterPrompt,
            }],
        };
        assert_eq!(
            run(&mut machine, &policy).expect("prompt and stimulus"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), 35);
        assert_eq!(machine.cpu().retired(), 2);
    }
}
