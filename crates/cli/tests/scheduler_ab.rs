// SPDX-License-Identifier: AGPL-3.0-or-later
//! A/B gates for PC/AT scheduler migration; see the support module (§10).
//!
//! Port 61h bits follow the PIT since 2026-11-11; delay loops take real refresh time.
//!

#[path = "support/scheduler.rs"]
mod scheduler;
#[path = "support/session.rs"]
pub mod session;

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

/// Intel 8254 datasheet, Mode 2: OUT is low at count=1 and rises on reload.
/// With the Boot guest's divisor 20, the initial load at 18 precedes the rising edge by 20
/// clocks; the ready HLT boundary accepts IRQ0 at that same clock.
#[test]
fn fidelity_mode2_timer_trace_uses_count_one_low_phase() {
    for engine in [SchedulerEngine::PerClock, SchedulerEngine::Deadline] {
        let stream = scheduler::run_workload_engine(Workload::Boot, engine)
            .unwrap()
            .unwrap();
        let first_inta = stream.iter().find_map(|event| match event {
            Event::Pic(chipset::PicObservation::Acknowledge { at, vector: 8, .. }) => Some(at.0),
            _ => None,
        });
        let first_byte = stream.iter().find_map(|event| match event {
            Event::Access {
                access:
                    TraceEvent::Port {
                        at,
                        port: 0x3F8,
                        dir: AccessDir::Write,
                        ..
                    },
                ..
            } => Some(at.0),
            _ => None,
        });
        let stop = stream.last().and_then(|event| match event {
            Event::Stop { at, .. } => Some(at.0),
            _ => None,
        });
        assert_eq!(
            (first_inta, first_byte, stop),
            (Some(38), Some(43), Some(335))
        );
        assert!(matches!(
            stream.last(),
            Some(Event::Stop {
                at: VClock(335),
                reason: Stop::Halted,
                ..
            })
        ));
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

#[test]
fn settling_before_snapshot_clears_expired_dormant_keyboard_deadline() {
    use device::Device;
    let mut keyboard = devices::I8042::new();
    keyboard.push_scancode(0x1E);
    let mut bus = bus::Bus::new(
        vec![Box::new(keyboard)],
        bus::DecodeMap {
            ports: vec![bus::PortRange {
                width: bus::PortWidth::default(),
                ports: 0x60..=0x64,
                device: 0,
            }],
            mmio: vec![],
        },
    );
    bus.port_read(0x60, 1).unwrap();
    bus.tick_due(VClock(1));
    bus.tick_due(VClock(2000));
    bus.settle();
    let snapshot = bus.device(0).unwrap().save();
    let mut restored = devices::I8042::new();
    restored.restore(&snapshot).unwrap();
    restored.push_scancode(0x9E);
    assert_eq!(restored.port_read(0x64, 1).unwrap() & 1, 1);
    assert_eq!(restored.port_read(0x60, 1).unwrap(), 0x9E);
    let timestamp = restored.take_trace();
    assert!(timestamp.iter().all(|event| matches!(
        event,
        device::TraceEvent::Port {
            at: VClock(2000),
            ..
        }
    )));
}

/// Intel 8254 datasheet, Mode 2: OUT pulses low for one clock at terminal count.
/// A running count restored at the clock it was saved at continues exactly
/// as the original under both engines.
#[test]
fn pit_restore_at_clock_50_matches_both_engines() {
    use device::Device;

    let mut pit = devices::Pit8253::new();
    for (port, value) in [(0x43, 0x34), (0x40, 100), (0x40, 0)] {
        pit.port_write(port, 1, value).unwrap();
    }
    let mut reference = testkit::MockBus::new(Box::new(pit));
    reference.tick(50);
    let snapshot = reference.device().save();
    reference.device_mut().restore(&snapshot).unwrap();
    let mut expected = Vec::new();
    for _ in 51..=251 {
        reference.tick(1);
        expected.push(reference.device().irq_pending());
    }
    assert_eq!(expected.iter().position(|out| !out), Some(49));

    for engine in [SchedulerEngine::PerClock, SchedulerEngine::Deadline] {
        let mut bus = bus::Bus::new(
            vec![Box::new(devices::Pit8253::new())],
            bus::DecodeMap {
                ports: vec![bus::PortRange {
                    width: bus::PortWidth::default(),
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
        |machine, observation, policy| {
            machine
                .bus_mut()
                .port_write(0x3F8, 1, u32::from(b'!'))
                .unwrap();
            assembly::run(machine, observation, policy)
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
// The bare rig's 8042 selects 80286; the POP DS single-step trap changes only
// the open BIOS's CPU identification and warning row in the pinned screen.
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

/// After the boot sector halts and the timer wakes it, the BIOS-to-floppy run
/// reaches the end of code segment 0, where an instruction crossing offset
/// FFFFh raises the general-protection exception (interrupt 13; Intel 80286 and
/// 80287 Programmer's Reference Manual, real-address-mode exceptions) and the
/// guest loops through the BIOS handler. Both engines must agree through it.
#[test]
#[ignore = "requires BIOS and floppy fixtures; compares completed instruction engines"]
fn deadline_matches_per_clock_on_bios_floppy() {
    assert_optional_workload(Workload::BiosFloppy);
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
    let stream = scheduler::observe_with(prepared, |machine, _, _| {
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
    if !std::env::var("UNEMUPC_SCHEDULER_WALL_TIMES").is_ok_and(|value| value == "1") {
        eprintln!("skipping scheduler wall times: set UNEMUPC_SCHEDULER_WALL_TIMES=1");
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
                let outcome = assembly::run(
                    &mut prepared.machine,
                    &prepared.observation,
                    &prepared.policy,
                )
                .unwrap();
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
    let Ok(directory) = std::env::var("UNEMUPC_SCHEDULER_STREAMS") else {
        eprintln!("skipping scheduler streams: UNEMUPC_SCHEDULER_STREAMS is unset");
        return;
    };
    let record = std::env::var("UNEMUPC_RECORD_STREAMS").is_ok_and(|value| value == "1");
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

#[test]
fn scheduler_post_rig_rejects_one_byte_changed_bios() {
    let Some(mut rom) = scheduler::fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    assert!(
        scheduler::prepare_with_bios(Workload::BiosPost, Some(rom.clone()))
            .unwrap()
            .is_some()
    );
    rom[0] ^= 1;
    let error = match scheduler::prepare_with_bios(Workload::BiosPost, Some(rom)) {
        Err(error) => error,
        Ok(_) => panic!("scheduler POST rig must reject the changed BIOS"),
    };
    let expected = assembly::ConfigError::FirmwareNotListed {
        sha256: "95e2791c710fdce0ebfc116efe2a187a178f6e33a072e7b75b05861ffdc4f402".into(),
        board: "ibm-5170",
        known: "kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh), kiselev-xi8088-1.0.2-atreset (open, vendor/kiselev-bios/fetch.sh), unemu-at-bios-1.0.2-unemu.3 (open, vendor/unemu-at-bios/fetch.sh), unemu-at-bios-1.0.2-unemu.2 (open, vendor/unemu-at-bios/fetch.sh at unemupc bcb375e6410b18266bd5f99533624b768d067867), ibm-5170-bios-1984-01-10 (user-supplied: U27 6181028 even, U47 6181029 odd or U17 6181024 even@0, U37 6181026 odd@0, U27 6181025 even@32768, U47 6181027 odd@32768; see vendor/user-roms/README.md), ibm-5170-bios-1985-06-10 (user-supplied: U27 6480090 even, U47 6480091 odd; see vendor/user-roms/README.md), ibm-5170-bios-1985-11-15 (user-supplied: U27 61X9266 even, U47 61X9265 odd; see vendor/user-roms/README.md), landmark-rom-post-at-1.2.0 (user-supplied: U27 27256 even, U47 27256 odd; see vendor/user-roms/README.md)".into(),
    };
    assert_eq!(
        error.downcast_ref::<assembly::ConfigError>(),
        Some(&expected)
    );
    assert_eq!(error.to_string(), expected.to_string());
}

#[test]
fn scheduler_freedos_rig_rejects_an_overridden_unlisted_bios() {
    let Some(mut rom) = scheduler::fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    rom[0] ^= 1;
    let error = scheduler::prepare_with_bios(Workload::FreeDosPrompt, Some(rom))
        .err()
        .expect("FreeDOS preparation must honour the BIOS override");
    assert!(
        error.to_string().contains("is not a known image"),
        "{error}"
    );
}
