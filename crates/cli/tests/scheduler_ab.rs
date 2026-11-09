// SPDX-License-Identifier: AGPL-3.0-or-later
//! A/B gates for PC/AT scheduler migration; see the support module (§10).

#[path = "support/scheduler.rs"]
mod scheduler;

use device::{AccessDir, TraceEvent, VClock};
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
fn legacy_matches_itself_on_every_fast_workload() {
    for workload in scheduler::FAST_WORKLOADS {
        let left = scheduler::run_workload(workload).unwrap().unwrap();
        let right = scheduler::run_workload(workload).unwrap().unwrap();
        assert!(
            matches!(
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
fn injected_write_reports_the_first_real_difference_with_context() {
    let left = scheduler::run_workload(Workload::Serial).unwrap().unwrap();
    let right = scheduler::observe_with(
        scheduler::prepare(Workload::Serial).unwrap().unwrap(),
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
fn legacy_matches_itself_on_bios_post() {
    assert_optional_workload(Workload::BiosPost);
}

#[test]
#[ignore = "requires BIOS and FreeDOS fixtures and boots to the prompt twice"]
fn legacy_matches_itself_on_freedos_prompt() {
    assert_optional_workload(Workload::FreeDosPrompt);
}

fn assert_optional_workload(workload: Workload) {
    let Some(left) = scheduler::run_workload(workload).unwrap() else {
        return;
    };
    let right = scheduler::run_workload(workload).unwrap().unwrap();
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
