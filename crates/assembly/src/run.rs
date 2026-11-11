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

/// Policy, execution and host persistence failures while driving a run.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Host persistence failed at a completed service boundary.
    #[error(transparent)]
    Persistence(#[from] crate::PersistenceError),
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
    run_with_persistence(machine, observation, policy, &[])
}

/// Drive a session while checking journal errors before and after every existing
/// run boundary. This does not close a generation; callers explicitly finish.
pub fn run_with_persistence(
    machine: &mut Machine<X86JitCpu>,
    observation: &Observation,
    policy: &RunPolicy,
    persistence: &[crate::PersistenceHandle],
) -> Result<RunOutcome, RunError> {
    check_persistence(persistence)?;
    machine.set_idle_limit(policy.idle_limit);

    match &policy.stop {
        StopCondition::Natural => run_past_reset_requests(machine, policy.retired_cap, persistence),
        StopCondition::PromptThen {
            console,
            needle,
            poll_step,
            then_budget,
        } => {
            let (outcome, prompt_reached) = poll_to_prompt_with_persistence(
                machine,
                observation,
                console,
                needle,
                *poll_step,
                policy.retired_cap,
                persistence,
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
                return run_past_reset_requests(machine, Some(now + *then_budget), persistence);
            }
            Ok(outcome)
        }
    }
}

// A reported reset must not consume policy budgets or repeat prompt stimuli.
// Reset is a CPU boundary, not a new media session.
fn run_past_reset_requests(
    machine: &mut Machine<X86JitCpu>,
    cap: Option<u64>,
    persistence: &[crate::PersistenceHandle],
) -> Result<RunOutcome, RunError> {
    loop {
        let outcome = run_boundary(machine, cap, persistence)?;
        if outcome != RunOutcome::ResetRequested {
            return Ok(outcome);
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
#[cfg(test)]
fn poll_to_prompt(
    machine: &mut Machine<X86JitCpu>,
    observation: &Observation,
    console: &ConsoleProbe,
    needle: &str,
    poll_step: u64,
    retired_cap: Option<u64>,
) -> Result<(RunOutcome, bool), RunError> {
    poll_to_prompt_with_persistence(
        machine,
        observation,
        console,
        needle,
        poll_step,
        retired_cap,
        &[],
    )
}

fn poll_to_prompt_with_persistence(
    machine: &mut Machine<X86JitCpu>,
    observation: &Observation,
    console: &ConsoleProbe,
    needle: &str,
    poll_step: u64,
    retired_cap: Option<u64>,
    persistence: &[crate::PersistenceHandle],
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
        let outcome = run_past_reset_requests(machine, Some(cap), persistence)?;
        let found = console_contains(machine, observation, console, needle)?;
        let hit_cap = retired_cap.is_some_and(|hard| cap >= hard);
        if outcome == RunOutcome::Halted || found || hit_cap {
            return Ok((outcome, found));
        }
    }
}

fn check_persistence(handles: &[crate::PersistenceHandle]) -> Result<(), crate::PersistenceError> {
    for handle in handles {
        handle.check_running()?;
    }
    Ok(())
}

fn run_boundary(
    machine: &mut Machine<X86JitCpu>,
    cap: Option<u64>,
    handles: &[crate::PersistenceHandle],
) -> Result<RunOutcome, RunError> {
    check_persistence(handles)?;
    let outcome = machine.run_bounded(cap);
    check_persistence(handles)?;
    if matches!(outcome, Ok(RunOutcome::ResetRequested)) {
        machine.reset_cpu()?;
    }
    outcome.map_err(Into::into)
}

impl crate::AssembledMachine {
    /// Run to the selected boundary without ending the media session.
    pub fn run_session(&mut self, policy: &RunPolicy) -> Result<RunOutcome, RunError> {
        run_with_persistence(
            &mut self.machine,
            &self.observation,
            policy,
            &self.persistence,
        )
    }

    /// Poll all drive latches before advancing or accepting more host input.
    pub fn check_persistence(&self) -> Result<(), crate::PersistenceError> {
        check_persistence(&self.persistence)
    }

    /// End a successful session, including policy-limited runs. Never call this
    /// after an execution error. There is no atomic multi-drive close promise.
    pub fn finish_persistence(&mut self) -> Result<(), crate::PersistenceError> {
        let mut first = None;
        for handle in &mut self.persistence {
            if let Err(error) = handle.finish() {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        first.map_or(Ok(()), Err)
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

/// Guest keyboard LEDs at a boundary where the controller can accept input.
/// Waiting for readiness also lets an in-flight LED command finish before the
/// frontend encodes the next key. No port transaction or device mutation occurs.
pub fn keyboard_leds_if_ready(machine: &mut Machine<X86JitCpu>, kbc_index: usize) -> Option<u8> {
    let kbc = machine
        .bus_mut()
        .device_mut(kbc_index)?
        .as_any_mut()?
        .downcast_mut::<I8042>()?;
    kbc.host_input_ready().then(|| kbc.keyboard_leds())
}

/// Try one host byte at a completed run boundary, leaving ownership with the
/// caller when the controller cannot deliver it yet or the route is absent.
/// [`I8042::host_input_ready`] observes readiness without a guest port access.
/// This conservative frontend policy keeps
/// backpressure outside the device's unbounded script queue; scripted golden
/// injection through [`inject_scancodes`] retains its existing behaviour.
pub fn try_inject_scancode(
    machine: &mut Machine<X86JitCpu>,
    kbc_index: usize,
    scancode: u8,
) -> bool {
    let Some(kbc) = machine
        .bus_mut()
        .device_mut(kbc_index)
        .and_then(|dev| dev.as_any_mut())
        .and_then(|dev| dev.downcast_mut::<I8042>())
    else {
        return false;
    };
    if !kbc.host_input_ready() {
        return false;
    }
    kbc.push_scancode(scancode);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::{Bus, DecodeMap};
    use cpu::x86jit_backend::RealSegments;
    use cpu::Cpu;

    #[test]
    fn reset_boundaries_check_journals_before_continuing_and_seal_once() {
        for stop in ["natural", "prompt", "post-prompt"] {
            for fail in [false, true] {
                let temp = crate::test_support::TempDir::new("run-reset-journal");
                let path = temp.path();
                let base = path.join("base.img");
                let journal = path.join("disk.upfj");
                std::fs::write(&base, vec![0; 1_474_560]).unwrap();
                let mut program = vec![0x90, 0xFA, 0xFC];
                let out = |program: &mut Vec<u8>, port: u16, value: u8| {
                    program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
                };
                out(&mut program, 0x3F2, 0x1C);
                for (port, value) in [
                    (0x0A, 6),
                    (0x0C, 0),
                    (0x0B, 0x4A),
                    (4, 0),
                    (4, 6),
                    (0x81, 0),
                    (5, 0xFF),
                    (5, 1),
                    (0x0A, 2),
                ] {
                    out(&mut program, port, value);
                }
                for value in [5, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
                    out(&mut program, 0x3F5, value);
                }
                program.extend([0xBF, 0, 0x10]);
                for _ in 0..7 {
                    program.extend([0xEC, 0xAA]);
                }
                program.extend([
                    0xB0, 0xFE, 0xE6, 0x64, // Request reset after the completed disk write.
                    0xB0, 0x55, 0xF4,
                ]);
                let demo = crate::at_fdc_write(program, vec![0xA7; 512], 0x600);
                let mut config = crate::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
                config.board = crate::BoardModel::Bare;
                config.cpu = demo.cpu;
                config.images = demo.images;
                config.images.push(crate::LoadImage {
                    base: 0xFFFF0,
                    source: crate::ImageSource::Inline(vec![0xB0, 0x66, 0xF4]),
                    kind: crate::ImageKind::Rom,
                    provides: Vec::new(),
                });
                config.drives[0].image = crate::ImageSource::File(base.clone());
                let mut reference = crate::assemble(&config).unwrap();
                assert_eq!(
                    reference.machine.run_bounded(Some(10_000)).unwrap(),
                    RunOutcome::ResetRequested
                );
                let reset_time = reference.machine.virtual_time();
                config.drives[0].write = crate::WriteMode::journal(&journal);
                let mut assembled = crate::assemble_with_observation(
                    &config,
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
                if fail {
                    assembled.persistence[0].fail_output();
                }
                let mut policy = RunPolicy::natural(Some(10_000), None);
                if stop != "natural" {
                    policy.stop = StopCondition::PromptThen {
                        console: ConsoleProbe {
                            device: DeviceId::Vga,
                        },
                        needle: String::new(),
                        poll_step: if stop == "post-prompt" { 1 } else { 10_000 },
                        then_budget: 9_999,
                    };
                    if stop == "post-prompt" {
                        policy.stimuli.push(KeyboardStimulus {
                            keyboard_index: assembled.device_index[&DeviceId::Keyboard],
                            scancodes: Vec::new(),
                            schedule: InjectionSchedule::AfterPrompt,
                        });
                    }
                }
                let result = assembled.run_session(&policy);
                if fail {
                    assert!(matches!(result, Err(RunError::Persistence(_))));
                    assert_eq!(assembled.machine.virtual_time(), reset_time);
                    assert_eq!(assembled.machine.cpu().code_segment(), 0);
                    assert_eq!(assembled.machine.cpu().reg(cpu::CpuReg::Rax) & 0xFF, 0xFE);
                    assert!(assembled.run_session(&policy).is_err());
                    assert_eq!(assembled.machine.virtual_time(), reset_time);
                    // Bypass the failed host handle only to inspect the retained
                    // machine obligation, without executing another instruction.
                    assert_eq!(
                        assembled.machine.run_bounded(Some(reset_time)).unwrap(),
                        RunOutcome::RetiredCapReached
                    );
                    assert_eq!(assembled.machine.cpu().code_segment(), 0xF000);
                    assert_eq!(assembled.machine.cpu().reg(cpu::CpuReg::Rip), 0xFFF0);
                    assert_eq!(assembled.machine.virtual_time(), reset_time);
                } else {
                    assert_eq!(result.unwrap(), RunOutcome::Halted);
                    assert_eq!(assembled.machine.virtual_time(), reset_time + 2);
                    assert_eq!(assembled.machine.cpu().reg(cpu::CpuReg::Rax) & 0xFF, 0x66);
                }
                let bytes = std::fs::read(&journal).unwrap();
                let image = std::fs::read(&base).unwrap();
                let report = media::scan(&bytes, &image, image.len() as u64).report;
                assert!(
                    report.generations.is_empty(),
                    "reset must not seal the session"
                );
                if !fail {
                    assembled.finish_persistence().unwrap();
                }
                drop(assembled);
                let bytes = std::fs::read(&journal).unwrap();
                let report = media::scan(&bytes, &image, image.len() as u64).report;
                assert_eq!(report.generations.len(), usize::from(!fail));
                if !fail {
                    assert_eq!(
                        report.generations[0].reason,
                        media::ClosingReason::SessionEnd
                    );
                }
            }
        }
    }

    #[test]
    fn host_injection_defers_full_buffer_and_rejects_missing_controller() {
        let mut assembled = crate::assemble(&crate::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        let index = assembled.device_index[&DeviceId::Keyboard];
        assert!(try_inject_scancode(&mut assembled.machine, index, 0x1E));
        assert!(!try_inject_scancode(&mut assembled.machine, index, 0x9E));
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
            0x1E
        );
        assert!(!try_inject_scancode(&mut assembled.machine, index, 0x9E));
        assembled.machine.run_bounded(Some(100_000)).unwrap();
        assert!(try_inject_scancode(&mut assembled.machine, index, 0x9E));
        assert!(!try_inject_scancode(
            &mut assembled.machine,
            usize::MAX,
            0x30
        ));
        assert!(!try_inject_scancode(
            &mut assembled.machine,
            assembled.device_index[&DeviceId::Pit],
            0x30
        ));
        assert_eq!(assembled.machine.virtual_time(), 100_000);
    }

    #[test]
    fn keyboard_led_boundary_read_is_side_effect_free_and_waits_for_readiness() {
        let mut assembled = crate::assemble(&crate::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        let index = assembled.device_index[&DeviceId::Keyboard];
        let dev = assembled.machine.bus_mut().device_mut(index).unwrap();
        dev.set_trace_policy(device::TracePolicy::Ring { capacity: 16 });
        let before = dev.save();
        assert_eq!(
            keyboard_leds_if_ready(&mut assembled.machine, index),
            Some(0)
        );
        assert_eq!(
            assembled.machine.bus().device(index).unwrap().save(),
            before
        );
        assert!(assembled
            .machine
            .bus_mut()
            .device_mut(index)
            .unwrap()
            .take_trace()
            .is_empty());
        assembled
            .machine
            .bus_mut()
            .port_write(0x60, 1, 0xED)
            .unwrap();
        assembled.machine.bus_mut().port_read(0x60, 1).unwrap();
        assembled.machine.run_bounded(Some(100_000)).unwrap();
        assert_eq!(keyboard_leds_if_ready(&mut assembled.machine, index), None);
        assembled.machine.bus_mut().port_write(0x60, 1, 2).unwrap();
        assembled.machine.run_bounded(Some(200_000)).unwrap();
        assembled.machine.bus_mut().port_read(0x60, 1).unwrap();
        assembled.machine.run_bounded(Some(300_000)).unwrap();
        assert_eq!(
            keyboard_leds_if_ready(&mut assembled.machine, index),
            Some(2)
        );
    }

    #[test]
    fn host_injection_never_reads_a_port_or_stages_behind_inhibited_input() {
        let mut assembled = crate::assemble(&crate::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        let index = assembled.device_index[&DeviceId::Keyboard];
        assembled
            .machine
            .bus_mut()
            .device_mut(index)
            .unwrap()
            .set_trace_policy(device::TracePolicy::Ring { capacity: 16 });
        assert!(try_inject_scancode(&mut assembled.machine, index, 0x1E));
        assert!(!try_inject_scancode(&mut assembled.machine, index, 0x9E));
        assert!(assembled
            .machine
            .bus_mut()
            .device_mut(index)
            .unwrap()
            .take_trace()
            .is_empty());
        assembled.machine.bus_mut().port_read(0x60, 1).unwrap();
        assembled.machine.run_bounded(Some(100_000)).unwrap();
        assembled
            .machine
            .bus_mut()
            .port_write(0x64, 1, 0xAD)
            .unwrap();
        let before = assembled.machine.bus().device(index).unwrap().save();
        assert!(!try_inject_scancode(&mut assembled.machine, index, 0x9E));
        assert_eq!(
            assembled.machine.bus().device(index).unwrap().save(),
            before
        );
    }

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
                        width: bus::PortWidth::default(),
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
