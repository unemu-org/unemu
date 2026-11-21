// SPDX-License-Identifier: AGPL-3.0-or-later
//! Journal checks at reset boundaries; persistent-media.md § Session lifecycle.
mod tests {
    use assembly::*;
    use cpu::Cpu;
    use machine::RunOutcome;
    #[test]
    fn reset_boundaries_check_journals_before_continuing_and_seal_once() {
        for stop in ["natural", "prompt", "post-prompt"] {
            for fail in [false, true] {
                let temp = assembly::test_support::TempDir::new("run-reset-journal");
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
                let demo = assembly::at_fdc_write(program, vec![0xA7; 512], 0x600);
                let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
                config.board = assembly::BoardModel::Bare;
                config.cpu = demo.cpu;
                config.images = demo.images;
                config.images.push(assembly::LoadImage {
                    base: 0xFFFF0,
                    source: assembly::ImageSource::Inline(vec![0xB0, 0x66, 0xF4]),
                    kind: assembly::ImageKind::Rom,
                    provides: Vec::new(),
                });
                config.drives[0].image = assembly::ImageSource::File(base.clone());
                let mut reference = crate::assemble(&config).unwrap();
                assert_eq!(
                    reference.machine.run_bounded(Some(10_000)).unwrap(),
                    RunOutcome::ResetRequested
                );
                let reset_time = reference.machine.virtual_time();
                config.drives[0].write = assembly::WriteMode::journal(&journal);
                let mut assembled = crate::assemble_with_observation(
                    &config,
                    &assembly::ObservationPolicy {
                        routes: vec![assembly::ObservationRequest {
                            device: DeviceId::Vga,
                            output: assembly::OutputKind::TextScanout,
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
                let report = journal::scan(&bytes, &image, image.len() as u64).report;
                assert!(
                    report.generations.is_empty(),
                    "reset must not seal the session"
                );
                if !fail {
                    assembled.finish_persistence().unwrap();
                }
                drop(assembled);
                let bytes = std::fs::read(&journal).unwrap();
                let report = journal::scan(&bytes, &image, image.len() as u64).report;
                assert_eq!(report.generations.len(), usize::from(!fail));
                if !fail {
                    assert_eq!(
                        report.generations[0].reason,
                        journal::ClosingReason::SessionEnd
                    );
                }
            }
        }
    }
}
