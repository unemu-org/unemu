// SPDX-License-Identifier: AGPL-3.0-or-later
//! A/B gates for PC/AT scheduler migration; see the support module (§10).

#[path = "support/scheduler.rs"]
mod scheduler;

use cpu::Cpu;
use device::{AccessDir, TraceEvent, VClock};
use machine::SchedulerEngine;
use scheduler::{compare, CapEndpoint, Event, Normalisations, Stop, Workload};

fn access(at: u64, value: u32) -> Event {
    Event::Access {
        device: assembly::DeviceId::Uart,
        access: TraceEvent::Port {
            at: VClock(at),
            port: 0x3F8,
            size: 1,
            dir: AccessDir::Write,
            value,
        },
    }
}

fn cap(at: u64) -> Event {
    Event::Stop {
        at: VClock(at),
        virtual_time: at,
        reason: Stop::CapReached,
    }
}

#[test]
fn deadline_matches_per_clock_on_every_fast_workload() {
    for workload in scheduler::FAST_WORKLOADS {
        let Some(left) =
            scheduler::run_workload_engine(workload, SchedulerEngine::PerClock).unwrap()
        else {
            continue;
        };
        let right = scheduler::run_workload_engine(workload, SchedulerEngine::Deadline)
            .unwrap()
            .unwrap();
        assert!(
            matches!(workload, Workload::FdcWrite | Workload::Test386)
                || matches!(
                    left.last(),
                    Some(Event::Stop {
                        reason: Stop::Halted,
                        ..
                    })
                ),
            "{workload:?}: {:?}",
            left.last()
        );
        assert!(left
            .iter()
            .any(|event| matches!(event, Event::Access { .. })));
        assert!(left
            .iter()
            .any(|event| matches!(event, Event::FinalState { .. })));
        compare(&left, &right, Normalisations::default())
            .unwrap_or_else(|diff| panic!("{workload:?}: {diff}"));
        match workload {
            Workload::Boot | Workload::Keyboard | Workload::Rtc => {
                let vector = if matches!(workload, Workload::Rtc) {
                    0x70
                } else if matches!(workload, Workload::Keyboard) {
                    9
                } else {
                    8
                };
                assert!(left.iter().any(|event| matches!(event, Event::Pic(chipset::PicObservation::Acknowledge { vector: v, .. }) if *v == vector)));
                assert!(left
                    .iter()
                    .any(|event| matches!(event, Event::Pic(chipset::PicObservation::Eoi { .. }))));
            }
            Workload::VgaText => assert!(left.iter().any(|event| matches!(
                event,
                Event::Access {
                    access: TraceEvent::Mmio {
                        addr: 0xB8000,
                        dir: AccessDir::Write,
                        ..
                    },
                    ..
                }
            ))),
            _ => {}
        }
    }
}

#[test]
fn serial_workload_has_no_per_clock_devices() {
    let prepared = scheduler::prepare(Workload::SerialReal16).unwrap().unwrap();
    for index in prepared.device_index.values() {
        let device = prepared.machine.bus().device(*index).unwrap();
        assert_ne!(
            device.next_event(prepared.machine.clock()),
            device::NextEvent::EveryPitClock,
            "{} still requests per-clock visits",
            device.name()
        );
    }
}

/// Intel 8254 datasheet, Mode 2: OUT pulses low for one clock at terminal count.
/// A restored running count must advance from the bus clock of the restore.
#[test]
fn pit_restore_at_clock_50_matches_both_engines() {
    use device::Device;

    let mut pit = devices::Pit8253::new();
    for (port, value) in [(0x43, 0x34), (0x40, 100), (0x40, 0)] {
        pit.port_write(port, 1, value).unwrap();
    }
    let mut reference = testkit::MockBus::new(Box::new(pit));
    reference.tick(1);
    let snapshot = reference.device().save();
    reference.tick(49);
    reference.device_mut().restore(&snapshot).unwrap();
    let mut expected = Vec::new();
    for _ in 51..=251 {
        reference.tick(1);
        expected.push(reference.device().irq_pending());
    }
    assert_eq!(expected.iter().position(|out| !out), Some(99));

    for engine in [SchedulerEngine::PerClock, SchedulerEngine::Deadline] {
        let mut bus = bus::Bus::new(
            vec![Box::new(devices::Pit8253::new())],
            bus::DecodeMap {
                ports: vec![bus::PortRange {
                    ports: 0x40..=0x43,
                    device: 0,
                }],
                mmio: Vec::new(),
            },
        );
        for (port, value) in [(0x43, 0x34), (0x40, 100), (0x40, 0)] {
            bus.port_write(port, 1, value).unwrap();
        }
        let tick = |bus: &mut bus::Bus, clock| match engine {
            SchedulerEngine::PerClock => bus.tick_all(VClock(clock)),
            SchedulerEngine::Deadline => {
                bus.tick_due(VClock(clock));
            }
        };
        for clock in 1..=50 {
            tick(&mut bus, clock);
        }
        bus.device_mut(0).unwrap().restore(&snapshot).unwrap();
        let mut actual = Vec::new();
        for clock in 51..=251 {
            tick(&mut bus, clock);
            actual.push(bus.device_irq_line(0).unwrap());
        }
        assert_eq!(actual, expected, "{engine:?}");
    }
}

#[test]
fn injected_write_reports_the_first_real_difference_with_context() {
    let left = scheduler::run_workload(Workload::SerialReal16)
        .unwrap()
        .unwrap();
    let right = scheduler::observe_with(
        scheduler::prepare(Workload::SerialReal16).unwrap().unwrap(),
        |machine, policy| {
            machine
                .bus_mut()
                .port_write(0x3F8, 1, u32::from(b'!'))
                .unwrap();
            assembly::run(machine, policy)
        },
    );
    let difference = compare(&left, &right, Normalisations::default()).unwrap_err();
    assert_eq!(difference.index, 0);
    assert_eq!(difference.left, left.first().cloned());
    assert_eq!(difference.right, Some(access(0, u32::from(b'!'))));
    let report = difference.to_string();
    for expected in [
        "first difference at event 0",
        "left:",
        "right:",
        "[0]",
        "[1]",
        "[2]",
    ] {
        assert!(report.contains(expected), "{report}");
    }
}

#[test]
fn constant_access_shift_is_explicit_and_does_not_shift_inta() {
    let irq = Event::Pic(chipset::PicObservation::Acknowledge {
        at: VClock(4),
        vector: 8,
    });
    let left = vec![access(2, 42), access(5, 43), irq.clone(), cap(9)];
    let right = vec![access(1, 42), access(4, 43), irq, cap(9)];
    assert_eq!(
        compare(&left, &right, Normalisations::default())
            .unwrap_err()
            .index,
        0
    );
    let options = Normalisations {
        right_access_shift: 1,
        ..Normalisations::default()
    };
    compare(&left, &right, options).unwrap();
    let mut variable_shift = right.clone();
    variable_shift[1] = access(3, 43);
    assert_eq!(
        compare(&left, &variable_shift, options).unwrap_err().index,
        1
    );
    let mut moved_irq = right;
    moved_irq[2] = Event::Pic(chipset::PicObservation::Acknowledge {
        at: VClock(3),
        vector: 8,
    });
    assert_eq!(compare(&left, &moved_irq, options).unwrap_err().index, 2);
    compare(
        &[access(0, 42)],
        &[access(1, 42)],
        Normalisations {
            right_access_shift: -1,
            ..options
        },
    )
    .unwrap();
    assert!(compare(
        &[access(u64::MAX, 42)],
        &[access(0, 42)],
        Normalisations {
            right_access_shift: -1,
            ..options
        }
    )
    .is_err());
}

#[test]
fn cap_endpoint_only_normalises_bounded_final_cap_times() {
    let left = vec![access(2, 42), cap(100)];
    let right = vec![access(2, 42), cap(108)];
    assert_eq!(
        compare(&left, &right, Normalisations::default())
            .unwrap_err()
            .index,
        1
    );
    let options = Normalisations {
        cap_endpoint: Some(CapEndpoint {
            cap: 100,
            max_overshoot: 8,
        }),
        ..Normalisations::default()
    };
    compare(&left, &right, options).unwrap();
    compare(&[cap(109)], &[cap(109)], options).unwrap();
    assert!(compare(&left, &[access(2, 42), cap(109)], options).is_err());
    assert!(compare(&left, &[access(2, 42), cap(99)], options).is_err());
    let halted = Event::Stop {
        at: VClock(108),
        virtual_time: 108,
        reason: Stop::Halted,
    };
    assert!(compare(&left, &[access(2, 42), halted], options).is_err());
    assert_eq!(
        compare(&left, &[access(2, 42), access(101, 99), cap(108)], options)
            .unwrap_err()
            .index,
        1
    );
    assert!(compare(
        &[cap(100), access(110, 42)],
        &[cap(108), access(110, 42)],
        options
    )
    .is_err());
}

#[test]
fn normalisations_compose_without_hiding_values_widths_or_order() {
    let left = vec![access(2, 42), access(2, 43), cap(100)];
    let right = vec![access(1, 42), access(1, 43), cap(108)];
    let options = Normalisations {
        right_access_shift: 1,
        right_mmio_shift: None,
        cap_endpoint: Some(CapEndpoint {
            cap: 100,
            max_overshoot: 8,
        }),
    };
    compare(&left, &right, options).unwrap();
    let mut swapped = right.clone();
    swapped.swap(0, 1);
    assert_eq!(compare(&left, &swapped, options).unwrap_err().index, 0);
    let mut width = right;
    if let Event::Access {
        access: TraceEvent::Port { size, .. },
        ..
    } = &mut width[1]
    {
        *size = 2;
    }
    assert_eq!(compare(&left, &width, options).unwrap_err().index, 1);
}

#[test]
fn unequal_lengths_report_the_missing_event() {
    let left = vec![access(1, 42), cap(100)];
    let diff = compare(&left, &left[..1], Normalisations::default()).unwrap_err();
    assert_eq!(diff.index, 1);
    assert_eq!(diff.left, Some(cap(100)));
    assert_eq!(diff.right, None);
    compare(&[], &[], Normalisations::default()).unwrap();
}

// Same opt-in boundary as the firmware goldens; absent fixtures still skip.
#[test]
#[ignore = "requires BIOS fixture and runs POST twice"]
fn deadline_matches_per_clock_on_bios_post() {
    assert_optional_workload(Workload::BiosPost);
}

#[test]
#[ignore = "requires BIOS and FreeDOS fixtures and boots to the prompt twice"]
fn deadline_matches_per_clock_on_freedos_prompt() {
    assert_optional_workload(Workload::FreeDosPrompt);
}

/// KNOWN CPU-CORE DEFECT: after the boot sector halts, the BIOS-to-floppy run
/// leaves the end of code segment 0, and the pinned core decodes a real-mode
/// block past the 16-bit IP wrap. The result then depends on the step budget,
/// so the two engines diverge. This test pins the divergence; once the core is
/// fixed and re-pinned it fails, and it must become an equality check through
/// `assert_optional_workload` like its neighbours.
#[test]
#[ignore = "requires BIOS and floppy fixtures; pins a known CPU-core divergence"]
fn deadline_diverges_from_per_clock_on_bios_floppy_until_the_core_is_fixed() {
    let Some(left) =
        scheduler::run_workload_engine(Workload::BiosFloppy, SchedulerEngine::PerClock).unwrap()
    else {
        return;
    };
    let right = scheduler::run_workload_engine(Workload::BiosFloppy, SchedulerEngine::Deadline)
        .unwrap()
        .unwrap();
    // Final snapshots follow the last access and precede Stop. Compare the
    // execution events here to pin the missing OUT against the actual stop;
    // equality gates elsewhere still compare the complete saved state.
    let execution = |stream: Vec<Event>| {
        stream
            .into_iter()
            .filter(|event| !matches!(event, Event::FinalState { .. }))
            .collect::<Vec<_>>()
    };
    let difference = compare(&execution(left), &execution(right), Normalisations::default())
        .expect_err("the engines now agree on BiosFloppy: the CPU-core defect is fixed, turn this back into an equality check");
    assert_eq!(difference.index, 349_290, "{difference}");
    assert_eq!(
        difference.left,
        Some(access(5_899_026, 0x42)),
        "{difference}"
    );
    assert!(
        matches!(&difference.right, Some(Event::Stop {
        at: VClock(5_899_248), reason: Stop::Error(error), ..
    }) if error.contains("unsupported instruction")),
        "{difference}"
    );
}

#[test]
#[ignore = "requires BIOS and FreeDOS fixtures; runs DIR with both engines"]
fn deadline_matches_per_clock_on_dos_dir() {
    assert_optional_workload(Workload::DosDir);
}

#[test]
#[ignore = "requires BIOS and FreeDOS fixtures; writes and lists a file with both engines"]
fn deadline_matches_per_clock_on_dos_write() {
    assert_optional_workload(Workload::DosWrite);
}

fn assert_optional_workload(workload: Workload) {
    let Some(left) = scheduler::run_workload_engine(workload, SchedulerEngine::PerClock).unwrap()
    else {
        return;
    };
    let right = scheduler::run_workload_engine(workload, SchedulerEngine::Deadline)
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            left.last(),
            Some(Event::Stop {
                reason: Stop::CapReached,
                ..
            })
        ),
        "{workload:?}: {:?}",
        left.last()
    );
    compare(&left, &right, Normalisations::default())
        .unwrap_or_else(|diff| panic!("{workload:?}: {diff}"));
}

#[test]
fn equal_time_events_keep_cross_device_and_inta_out_eoi_order() {
    use device::InterruptController;
    let prepared = scheduler::prepare(Workload::Boot).unwrap().unwrap();
    let uart = prepared.device_index[&assembly::DeviceId::Uart];
    let master = prepared.device_index[&assembly::DeviceId::MasterPic];
    let stream = scheduler::observe_with(prepared, |machine, _| {
        for (port, value) in [(0x20, 0x11), (0x21, 8), (0x21, 4), (0x21, 1)] {
            machine.bus_mut().port_write(port, 1, value).unwrap();
        }
        let mut controller = machine
            .bus_mut()
            .device_mut(master)
            .unwrap()
            .as_any_mut()
            .and_then(|any| any.downcast_mut::<chipset::SharedPic>())
            .unwrap()
            .clone();
        controller.set_irq_line(0, true);
        assert_eq!(controller.acknowledge(), Some(8));
        assert_eq!(machine.bus().device(uart).unwrap().name(), "16550-uart");
        machine.bus_mut().port_write(0x3F8, 1, 42).unwrap();
        machine.bus_mut().port_write(0x20, 1, 0x20).unwrap();
        Ok(machine::RunOutcome::Halted)
    });
    assert!(matches!(
        stream[4],
        Event::Pic(chipset::PicObservation::Acknowledge {
            at: VClock(0),
            vector: 8
        })
    ));
    assert_eq!(stream[5], access(0, 42));
    assert!(matches!(
        stream[6],
        Event::Access {
            device: assembly::DeviceId::MasterPic,
            access: TraceEvent::Port {
                at: VClock(0),
                port: 0x20,
                ..
            }
        }
    ));
    assert!(matches!(
        stream[7],
        Event::Pic(chipset::PicObservation::Eoi {
            at: VClock(0),
            command: 0x20,
            value: Some(0x20)
        })
    ));
}

#[test]
fn mmio_shift_preserves_device_address_direction_and_value() {
    let mmio = |at, addr, dir, value| Event::Access {
        device: assembly::DeviceId::Vga,
        access: TraceEvent::Mmio {
            at: VClock(at),
            addr,
            size: 2,
            dir,
            value,
        },
    };
    let left = [mmio(2, 0xB8000, AccessDir::Write, 0x1F48)];
    let right = [mmio(1, 0xB8000, AccessDir::Write, 0x1F48)];
    let options = Normalisations {
        right_access_shift: 1,
        ..Normalisations::default()
    };
    compare(&left, &right, options).unwrap();
    assert!(compare(&left, &right, Normalisations::default()).is_err());
    for changed in [
        mmio(1, 0xB8002, AccessDir::Write, 0x1F48),
        mmio(1, 0xB8000, AccessDir::Read, 0x1F48),
        mmio(1, 0xB8000, AccessDir::Write, 0x1F49),
    ] {
        assert!(compare(&left, &[changed], options).is_err());
    }
}

/// Informative host timing only; fixture construction and stream observers are
/// outside the measured interval. Use a release build for reported comparisons.
#[test]
#[ignore = "requires BIOS/FreeDOS fixtures; informative machine wall times"]
fn scheduler_engine_wall_times() {
    if !std::env::var("UNEMUPC98_SCHEDULER_WALL_TIMES").is_ok_and(|value| value == "1") {
        eprintln!("skipping scheduler wall times: set UNEMUPC98_SCHEDULER_WALL_TIMES=1");
        return;
    }
    for workload in [Workload::BiosPost, Workload::FreeDosPrompt] {
        let engines = [SchedulerEngine::PerClock, SchedulerEngine::Deadline];
        let mut samples = [Vec::new(), Vec::new()];
        let mut expected_counts = None;
        for sample in 1..=3 {
            for (index, engine) in engines.into_iter().enumerate() {
                let Some(mut prepared) = scheduler::prepare(workload).unwrap() else {
                    continue;
                };
                prepared.machine = prepared.machine.with_scheduler_engine(engine);
                let start = std::time::Instant::now();
                let outcome = assembly::run(&mut prepared.machine, &prepared.policy).unwrap();
                let elapsed = start.elapsed();
                assert_eq!(outcome, machine::RunOutcome::RetiredCapReached);
                let counts = (prepared.machine.clock().0, prepared.machine.cpu().retired());
                assert_eq!(*expected_counts.get_or_insert(counts), counts);
                samples[index].push(elapsed);
                eprintln!(
                    "{workload:?} {engine:?} sample {sample}: {:.6} s; clock={}, retired={}",
                    elapsed.as_secs_f64(),
                    counts.0,
                    counts.1
                );
            }
        }
        for (engine, mut times) in engines.into_iter().zip(samples) {
            if times.len() == 3 {
                times.sort_unstable();
                eprintln!(
                    "{workload:?} {engine:?} median of 3: {:.6} s",
                    times[1].as_secs_f64()
                );
            }
        }
    }
}

/// Keep complete, unnormalised streams across scheduler optimisations. Recording
/// uses create_new so a comparison cannot accidentally replace its reference.
#[test]
#[ignore = "requires firmware/media and an explicit stream reference directory"]
fn scheduler_streams_unchanged() {
    let Ok(directory) = std::env::var("UNEMUPC98_SCHEDULER_STREAMS") else {
        eprintln!("skipping scheduler streams: UNEMUPC98_SCHEDULER_STREAMS is unset");
        return;
    };
    let record = std::env::var("UNEMUPC98_RECORD_STREAMS").is_ok_and(|value| value == "1");
    for workload in scheduler::FAST_WORKLOADS.into_iter().chain([
        Workload::BiosPost,
        Workload::BiosFloppy,
        Workload::FreeDosPrompt,
        Workload::DosDir,
        Workload::DosWrite,
    ]) {
        for engine in [SchedulerEngine::PerClock, SchedulerEngine::Deadline] {
            let Some(stream) = scheduler::run_workload_engine(workload, engine).unwrap() else {
                continue;
            };
            let path =
                std::path::Path::new(&directory).join(format!("{workload:?}-{engine:?}.txt"));
            let contents = format!("{stream:#?}\n");
            if record {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .unwrap();
                file.write_all(contents.as_bytes()).unwrap();
            } else {
                let expected = std::fs::read_to_string(&path).unwrap();
                for (line, (left, right)) in expected.lines().zip(contents.lines()).enumerate() {
                    assert_eq!(left, right, "{}: line {}", path.display(), line + 1);
                }
                assert_eq!(
                    expected.lines().count(),
                    contents.lines().count(),
                    "{}: stream length",
                    path.display()
                );
            }
            eprintln!(
                "{workload:?} {engine:?}: {} events {}",
                stream.len(),
                if record { "recorded" } else { "unchanged" }
            );
        }
    }
}

#[test]
fn access_normalisation_moves_ocw2_observations_but_never_inta() {
    let eoi = |at| {
        Event::Pic(chipset::PicObservation::Eoi {
            at: VClock(at),
            command: 0x20,
            value: Some(0x20),
        })
    };
    let inta = |at| {
        Event::Pic(chipset::PicObservation::Acknowledge {
            at: VClock(at),
            vector: 8,
        })
    };
    let options = Normalisations {
        right_access_shift: 1,
        right_mmio_shift: Some(0),
        ..Normalisations::default()
    };
    compare(&[eoi(2), inta(3)], &[eoi(1), inta(3)], options).unwrap();
    assert_eq!(
        compare(&[eoi(2), inta(3)], &[eoi(1), inta(2)], options)
            .unwrap_err()
            .index,
        1
    );
    let mmio = Event::Access {
        device: assembly::DeviceId::Vga,
        access: TraceEvent::Mmio {
            at: VClock(1),
            addr: 0xB8000,
            size: 2,
            dir: AccessDir::Write,
            value: 0x0741,
        },
    };
    compare(
        &[access(2, 42), mmio.clone()],
        &[access(1, 42), mmio],
        options,
    )
    .unwrap();
}
