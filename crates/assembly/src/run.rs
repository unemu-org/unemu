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
use video::Frame;

use crate::{DeviceId, Observation, ObservationError};

/// Policy validation failures and hardware execution errors while driving a run.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Hardware execution failed.
    #[error(transparent)]
    Machine(#[from] MachineError),
    /// The console route is unbound or its previously validated scanout is lost.
    /// The source distinguishes static configuration from a capture failure.
    #[error("console text scanout is unavailable for route {device:?}: {source}")]
    ConsoleUnavailable {
        /// Requested display route.
        device: DeviceId,
        /// The facade's original route or scanout error.
        #[source]
        source: ObservationError,
    },
    /// The prompt left too little room for the complete scripted command.
    #[error("insufficient command budget: need {required} steps, only {remaining} remain")]
    InsufficientCommandBudget {
        /// Requested post-prompt steps.
        required: u64,
        /// Steps remaining before the overall cap.
        remaining: u64,
    },
}

/// The text-scanout source used to detect a prompt (§9.3).
///
/// The observation facade resolves the card's route and distributes its frame
/// through the same capture path as final capture. Plain data keeps [`RunPolicy`] comparable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleProbe {
    /// Bound display route whose text scanout is searched.
    pub device: DeviceId,
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
        /// Which card supplies the console text.
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
/// This driver uses the assembler's concrete CPU, the production implementation
/// of the step-bounded CPU contract.
pub fn run(
    machine: &mut Machine<X86JitCpu>,
    observation: &Observation,
    policy: &RunPolicy,
) -> Result<RunOutcome, RunError> {
    machine.set_idle_limit(policy.idle_limit);

    match &policy.stop {
        StopCondition::Natural => machine.run_bounded(policy.retired_cap).map_err(Into::into),
        StopCondition::PromptThen {
            console,
            needle,
            poll_step,
            then_budget,
        } => {
            let (outcome, prompt_reached) = poll_to_prompt(
                machine,
                observation,
                console,
                needle,
                *poll_step,
                policy.retired_cap,
            )?;

            // Act on the prompt only if we actually reached it (not an error, not a
            // spin to the cap) and something is scheduled to fire there. With no
            // stimuli the run stops at the prompt — the plain FreeDOS boot.
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
            Ok(outcome)
        }
    }
}

/// Step the machine in `poll_step` virtual-time chunks until `needle` appears on
/// the console, the guest halts, an error occurs, or the overall cap is reached.
///
/// This is the "boot to the shell prompt" loop: a headless FreeCOM busy-polls the
/// keyboard at its date prompt forever, so rather than run to the cap the driver
/// grows the cap one chunk at a time and stops the moment the prompt shows. The
/// returned outcome is that of the last [`Machine::run_bounded`], paired with
/// whether that boundary's captured frame contains the prompt. Reusing the
/// result avoids presenting the same boundary again before injecting input.
fn poll_to_prompt(
    machine: &mut Machine<X86JitCpu>,
    observation: &Observation,
    console: &ConsoleProbe,
    needle: &str,
    poll_step: u64,
    retired_cap: Option<u64>,
) -> Result<(RunOutcome, bool), RunError> {
    observation
        .text_route(console.device)
        .map_err(|source| RunError::ConsoleUnavailable {
            device: console.device,
            source,
        })?;
    loop {
        let target = machine.virtual_time().saturating_add(poll_step);
        let cap = retired_cap.map_or(target, |hard| target.min(hard));
        let outcome = machine.run_bounded(Some(cap)).map_err(RunError::from)?;
        let found = console_contains(machine, observation, console, needle)?;
        let hit_cap = retired_cap.is_some_and(|hard| cap >= hard);
        if outcome == RunOutcome::Halted || found || hit_cap {
            return Ok((outcome, found));
        }
    }
}

/// Search the rendered card state at a completed machine service boundary.
fn console_contains(
    machine: &Machine<X86JitCpu>,
    observation: &Observation,
    probe: &ConsoleProbe,
    needle: &str,
) -> Result<bool, RunError> {
    let Frame::Text(grid) = observation
        .capture_text(machine.bus(), probe.device)
        .map_err(|source| RunError::ConsoleUnavailable {
            device: probe.device,
            source,
        })?;
    Ok(grid.rows_as_strings().join("\n").contains(needle))
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

    fn prompt_observation() -> Observation {
        Observation::bind(
            &crate::ObservationPolicy {
                routes: vec![crate::ObservationRequest {
                    device: DeviceId::Mda,
                    output: crate::OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..Default::default()
            },
            &std::collections::HashMap::from([(DeviceId::Mda, 1)]),
            &mut [Box::new(I8042::new()), prompt_card()],
        )
        .unwrap()
    }

    fn prompt_card() -> Box<dyn device::Device> {
        let mut card = devices::MdaText::new();
        device::Device::mmio_write(&mut card, 0xB0000, 2, 0x073E).unwrap();
        Box::new(card)
    }

    #[test]
    fn prompt_poll_rejects_unbound_route_before_guest_execution() {
        let config = crate::at_boot(vec![0xEB, 0xFE]);
        let mut assembled = crate::assemble(&config).unwrap();
        let device = DeviceId::Keyboard;
        let console = ConsoleProbe { device };
        let policy = RunPolicy {
            retired_cap: Some(20),
            idle_limit: None,
            stop: StopCondition::PromptThen {
                console,
                needle: "prompt".into(),
                poll_step: 5,
                then_budget: 0,
            },
            stimuli: Vec::new(),
        };
        assert!(
            matches!(run(&mut assembled.machine, &assembled.observation, &policy),
            Err(RunError::ConsoleUnavailable { device: actual, source: ObservationError::RouteNotBound { .. } }) if actual == device)
        );
        assert_eq!(assembled.machine.virtual_time(), 0);
        assert_eq!(assembled.machine.cpu().retired(), 0);
    }

    #[test]
    fn prompt_poll_preserves_scanout_failure_after_execution() {
        use std::error::Error;

        for (program, retired) in [(vec![0xEB, 0xFE], 5), (vec![0xFA, 0xF4], 2)] {
            let mut assembled = crate::assemble_with_observation(
                &crate::at_vga_text(program),
                &crate::ObservationPolicy {
                    routes: vec![crate::ObservationRequest {
                        device: DeviceId::Vga,
                        output: crate::OutputKind::TextScanout,
                        byte_limit: 0,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
            let index = assembled.device_index[&DeviceId::Vga];
            *assembled.machine.bus_mut().device_mut(index).unwrap() = Box::new(I8042::new());
            let policy = RunPolicy {
                retired_cap: Some(20),
                idle_limit: None,
                stop: StopCondition::PromptThen {
                    console: ConsoleProbe {
                        device: DeviceId::Vga,
                    },
                    needle: "prompt".into(),
                    poll_step: 5,
                    then_budget: 0,
                },
                stimuli: Vec::new(),
            };
            let error = run(&mut assembled.machine, &assembled.observation, &policy).unwrap_err();
            assert!(error
                .to_string()
                .starts_with("console text scanout is unavailable"));
            assert!(error.to_string().contains("bound device Vga"));
            assert_eq!(
                error.source().unwrap().downcast_ref::<ObservationError>(),
                Some(&ObservationError::ScanoutUnavailable(DeviceId::Vga))
            );
            assert!(matches!(
                error,
                RunError::ConsoleUnavailable {
                    device: DeviceId::Vga,
                    source: ObservationError::ScanoutUnavailable(DeviceId::Vga)
                }
            ));
            assert_eq!(assembled.machine.cpu().retired(), retired);
            assert_eq!(assembled.machine.virtual_time(), retired);
        }
    }

    #[test]
    fn console_probe_searches_card_scanout_with_its_start_address() {
        let mut assembled = crate::assemble_with_observation(
            &crate::bios(vec![0; 0x8000], None),
            &crate::ObservationPolicy {
                routes: vec![crate::ObservationRequest {
                    device: DeviceId::Mda,
                    output: crate::OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..Default::default()
            },
        )
        .unwrap();
        let probe = ConsoleProbe {
            device: DeviceId::Mda,
        };
        let bus = assembled.machine.bus_mut();
        bus.mmio_write(0xB0000, 4, 0x07420741).unwrap();
        assert!(
            console_contains(&assembled.machine, &assembled.observation, &probe, "AB").unwrap()
        );
        let bus = assembled.machine.bus_mut();
        bus.port_write(0x3B4, 1, 13).unwrap();
        bus.port_write(0x3B5, 1, 1).unwrap();
        assert!(
            !console_contains(&assembled.machine, &assembled.observation, &probe, "AB").unwrap()
        );
        assert!(console_contains(&assembled.machine, &assembled.observation, &probe, "B").unwrap());
        let missing = ConsoleProbe {
            device: DeviceId::Post,
        };
        assert!(matches!(
            console_contains(&assembled.machine, &assembled.observation, &missing, ""),
            Err(RunError::ConsoleUnavailable {
                device: DeviceId::Post,
                source: ObservationError::RouteNotBound { .. }
            })
        ));
    }

    #[test]
    fn late_prompt_rejects_partial_command_budget_before_injecting_keys() {
        for cap in [10, 15, 19] {
            let mut cpu =
                X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).unwrap();
            cpu.map_ram(0, 0x10000).unwrap();
            cpu.write_mem(0x7C00, &[0xEB, 0xFE]).unwrap();
            let bus = Bus::new(
                vec![Box::new(I8042::new()), prompt_card()],
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
                        device: DeviceId::Mda,
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
            let error = run(&mut machine, &prompt_observation(), &policy).unwrap_err();
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
                Bus::new(
                    vec![Box::new(I8042::new()), prompt_card()],
                    DecodeMap::default(),
                ),
                Box::new(chipset::NoInterruptController),
                Vec::new(),
                None,
            );
            let policy = RunPolicy {
                retired_cap,
                idle_limit: Some(100),
                stop: StopCondition::PromptThen {
                    console: ConsoleProbe {
                        device: DeviceId::Mda,
                    },
                    needle: "prompt".into(),
                    poll_step: 10,
                    then_budget: 0,
                },
                stimuli: Vec::new(),
            };
            assert_eq!(
                run(&mut machine, &prompt_observation(), &policy).expect("run"),
                RunOutcome::Halted
            );
            assert_eq!(machine.cpu().retired(), 2);
        }
    }

    #[test]
    fn prompt_and_stimulus_budgets_start_from_current_virtual_time() {
        let mut cpu =
            X86JitCpu::new_real16_bare(0x10000, 0, 0x7C00, RealSegments::flat(0)).expect("cpu");
        cpu.map_ram(0, 0x10000).expect("ram");
        cpu.write_mem(0x7C00, &[0xFB, 0xF4]).expect("STI; HLT");
        let mut machine = Machine::new(
            cpu,
            Bus::new(
                vec![Box::new(I8042::new()), prompt_card()],
                DecodeMap::default(),
            ),
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
            device: DeviceId::Mda,
        };
        assert_eq!(
            poll_to_prompt(&mut machine, &prompt_observation(), &console, ">", 10, None)
                .expect("poll"),
            (RunOutcome::RetiredCapReached, true)
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
            run(&mut machine, &prompt_observation(), &policy).expect("prompt and stimulus"),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(machine.virtual_time(), 35);
        assert_eq!(machine.cpu().retired(), 2);
    }
}
