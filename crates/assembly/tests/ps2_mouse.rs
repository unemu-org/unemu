// SPDX-License-Identifier: AGPL-3.0-or-later
//! PS/2 mouse input through the AT's 8042 and cascaded interrupt controllers.
//!
//! IBM PS/2 Hardware Interface Technical Reference, Common Interfaces,
//! "Pointing Device", Commands and Data Reporting; Intel 8259A datasheet,
//! Cascading and End of Interrupt. See machine-config.md §PS/2 mouse.

use assembly::*;

fn out(code: &mut Vec<u8>, port: u8, byte: u8) {
    code.extend([0xB0, byte, 0xE6, port]);
}

fn command(code: &mut Vec<u8>, byte: u8) {
    out(code, 0x64, 0xD4);
    out(code, 0x60, byte);
}

fn config(mouse: bool, enable: bool) -> MachineConfig {
    let mut code = vec![
        0xFA, 0xFC, // cli; cld
        0x31, 0xC0, 0x8E, 0xD8, 0x8E, 0xC0, 0x8E, 0xD0, // zero DS, ES, SS
        0xBC, 0x00, 0x70, // mov sp,7000h
        0xBF, 0x00, 0x20, // mov di,2000h
        0xC7, 0x06, 0xD0, 0x01, 0x00, 0x02, // IVT[74h] offset 0200h
        0xC7, 0x06, 0xD2, 0x01, 0x00, 0xF8, // IVT[74h] segment F800h
    ];
    for (port, byte) in [
        (0x20, 0x11),
        (0x21, 0x20),
        (0x21, 4),
        (0x21, 1),
        (0xA0, 0x11),
        (0xA1, 0x70),
        (0xA1, 2),
        (0xA1, 1),
        (0x21, 0xFB),
        (0xA1, 0xEF),
        (0x64, 0x60),
        (0x60, 2),
    ] {
        out(&mut code, port, byte);
    }
    code.push(0xFB); // sti
    command(&mut code, 0xFF);
    // Let all three reset replies traverse the serial link before enabling.
    code.extend([0xB9, 0x88, 0x13, 0xE2, 0xFE]); // cx=5000; loop $
    if enable {
        command(&mut code, 0xF4);
    }
    code.extend([0xF4, 0xEB, 0xFD]); // hlt; jmp hlt
    let mut rom = vec![0xF4; 0x8000];
    rom[..code.len()].copy_from_slice(&code);
    rom[0x200..0x20C].copy_from_slice(&[
        0x50, 0xE4, 0x60, 0xAA, // push ax; in al,60h; stosb
        0xB0, 0x20, 0xE6, 0xA0, 0xE6, 0x20, // EOI slave, then master
        0x58, 0xCF, // pop ax; iret
    ]);
    rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
    let mut config = bios(rom, Some(vec![0; 1_474_560]));
    mouse_config(&mut config).mouse = mouse;
    config
}

fn mouse_config(config: &mut MachineConfig) -> &mut I8042Cfg {
    let device = config
        .devices
        .iter_mut()
        .find(|d| d.id == DeviceId::Keyboard)
        .unwrap();
    let DeviceKind::Keyboard(cfg) = &mut device.kind else {
        panic!("8042 required")
    };
    cfg
}

fn assembled(mouse: bool, enable: bool) -> AssembledMachine {
    assemble_with_options(
        &config(mouse, enable),
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap()
}

fn run_chunk(assembled: &mut AssembledMachine, budget: u64) {
    assembled.machine.set_idle_limit(Some(1_000_000));
    let cap = assembled.machine.virtual_time() + budget;
    assert_eq!(
        assembled.machine.run_bounded(Some(cap)).unwrap(),
        machine::RunOutcome::RetiredCapReached
    );
}

fn collected(assembled: &AssembledMachine, expected: &[u8]) {
    let mut actual = vec![0; expected.len() + 3];
    assembled
        .machine
        .cpu()
        .read_mem(0x2000, &mut actual)
        .unwrap();
    assert_eq!(&actual[..expected.len()], expected);
    assert_eq!(
        &actual[expected.len()..],
        &[0; 3],
        "no extra interrupt bytes"
    );
}

#[test]
fn mouse_irq12_guest_collects_reset_enable_motion_and_buttons() {
    let engines = [
        machine::SchedulerEngine::Deadline,
        #[cfg(feature = "reference-scheduler")]
        machine::SchedulerEngine::PerClock,
    ];
    for engine in engines {
        let mut assembled = assembled(true, true);
        #[cfg(feature = "reference-scheduler")]
        {
            assembled.machine = assembled.machine.with_scheduler_engine(engine);
        }
        assert_eq!(assembled.machine.scheduler_engine(), engine);
        let index = assembled.device_index[&DeviceId::Keyboard];
        run_chunk(&mut assembled, 30_000);
        collected(&assembled, &[0xFA, 0xAA, 0, 0xFA]);
        assert!(inject_mouse(&mut assembled.machine, index, 5, -7, 1));
        run_chunk(&mut assembled, 10_000);
        collected(&assembled, &[0xFA, 0xAA, 0, 0xFA, 0x29, 5, 249]);
        assert!(inject_mouse(&mut assembled.machine, index, -2, 3, 6));
        run_chunk(&mut assembled, 10_000);
        collected(
            &assembled,
            &[0xFA, 0xAA, 0, 0xFA, 0x29, 5, 249, 0x1E, 254, 3],
        );
        assert!(inject_mouse(&mut assembled.machine, index, 0, 0, 0));
        run_chunk(&mut assembled, 10_000);
        collected(
            &assembled,
            &[0xFA, 0xAA, 0, 0xFA, 0x29, 5, 249, 0x1E, 254, 3, 8, 0, 0],
        );
    }
}

#[test]
fn mouse_off_drops_aux_commands_and_has_no_irq12() {
    let mut assembled = assembled(false, true);
    let index = assembled.device_index[&DeviceId::Keyboard];
    let uart = assembled.device_index[&DeviceId::Uart];
    assert!(!inject_mouse(&mut assembled.machine, uart, 5, 7, 1));
    assert!(!inject_mouse(&mut assembled.machine, index, 5, 7, 1));
    assert!(!inject_mouse(&mut assembled.machine, usize::MAX, 5, 7, 1));
    run_chunk(&mut assembled, 30_000);
    collected(&assembled, &[]);
    assert_eq!(
        assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 0x21,
        0
    );
    assert_eq!(
        assembled.machine.bus().device_irq_output(index, 1),
        Some(false)
    );
}

#[test]
fn disabled_reporting_accumulates_host_motion_until_guest_reads_data() {
    let mut assembled = assembled(true, false);
    let index = assembled.device_index[&DeviceId::Keyboard];
    run_chunk(&mut assembled, 30_000);
    assert!(inject_mouse(&mut assembled.machine, index, 5, -7, 1));
    assert!(inject_mouse(&mut assembled.machine, index, -2, 3, 4));
    run_chunk(&mut assembled, 10_000);
    collected(&assembled, &[0xFA, 0xAA, 0]);
    let bus = assembled.machine.bus_mut();
    bus.port_write(0x64, 1, 0xD4).unwrap();
    bus.port_write(0x60, 1, 0xEB).unwrap();
    run_chunk(&mut assembled, 10_000);
    collected(&assembled, &[0xFA, 0xAA, 0, 0xFA, 0x2C, 3, 252]);
}

#[test]
fn mouse_requires_an_at_controller_and_slave_pic_and_reserves_irq12() {
    let mut unsupported = at_keyboard(vec![0xF4]);
    mouse_config(&mut unsupported).mouse = true;
    assert_eq!(
        unsupported.validate(),
        Err(ConfigError::UnsupportedMouse {
            reason: "unsupported board bare".into()
        })
    );
    let mut no_slave = config(true, true);
    no_slave.devices.retain(|d| d.id != DeviceId::SlavePic);
    assert_eq!(
        no_slave.validate(),
        Err(ConfigError::BoardDeviceMissing {
            board: BoardModel::Ibm5170,
            device: DeviceId::SlavePic
        })
    );
    let mut conflict = config(true, true);
    conflict
        .devices
        .iter_mut()
        .find(|d| d.id == DeviceId::Uart)
        .unwrap()
        .irq = Some(12);
    assert_eq!(
        conflict.validate(),
        Err(ConfigError::IrqConflict {
            a: DeviceId::Keyboard,
            b: DeviceId::Uart,
            line: 12,
        })
    );
    assert!(config(false, true).validate().is_ok());
}

#[test]
fn mouse_support_checks_follow_board_validation_and_name_the_missing_requirement() {
    let mut no_slave = config(true, true);
    no_slave.devices.retain(|d| d.id != DeviceId::SlavePic);
    assert_eq!(
        no_slave.validate(),
        Err(ConfigError::BoardDeviceMissing {
            board: BoardModel::Ibm5170,
            device: DeviceId::SlavePic,
        })
    );
    let mut no_controller = config(true, true);
    no_controller.devices.retain(|d| d.id != DeviceId::Keyboard);
    assert_eq!(
        no_controller.validate(),
        Err(ConfigError::BoardDeviceMissing {
            board: BoardModel::Ibm5170,
            device: DeviceId::Keyboard,
        })
    );
    for (missing, message) in [
        (None, "PS/2 mouse: unsupported board bare"),
        (Some(DeviceId::SlavePic), "PS/2 mouse: no slave PIC"),
        (Some(DeviceId::Keyboard), "PS/2 mouse: no 8042 at Keyboard"),
    ] {
        let mut bare = at_keyboard(vec![0xF4]);
        mouse_config(&mut bare).mouse = true;
        match missing {
            Some(DeviceId::Keyboard) => {
                bare.devices
                    .iter_mut()
                    .find(|d| d.id == DeviceId::Keyboard)
                    .unwrap()
                    .id = DeviceId::FwCfg;
            }
            Some(id) => bare.devices.retain(|d| d.id != id),
            None => {}
        }
        assert_eq!(bare.validate().unwrap_err().to_string(), message);
    }
}

#[test]
fn keyboard_and_mouse_deliver_interleaved_bytes_to_separate_guest_handlers() {
    let engines = [
        machine::SchedulerEngine::Deadline,
        #[cfg(feature = "reference-scheduler")]
        machine::SchedulerEngine::PerClock,
    ];
    for engine in engines {
        let mut config = config(true, true);
        let ImageSource::Inline(rom) = &mut config.images[0].source else {
            panic!("inline ROM required")
        };
        let entry = rom
            .windows(3)
            .position(|bytes| bytes == [0xBF, 0, 0x20])
            .unwrap();
        rom.copy_within(entry..0x180, entry + 12);
        rom[entry..entry + 12]
            .copy_from_slice(&[0xC7, 0x06, 0x84, 0, 0, 3, 0xC7, 0x06, 0x86, 0, 0, 0xF8]);
        for (pattern, value) in [([0xB0, 0xFB, 0xE6, 0x21], 0xF9), ([0xB0, 2, 0xE6, 0x60], 3)] {
            let offset = rom.windows(4).position(|bytes| bytes == pattern).unwrap();
            rom[offset + 1] = value;
        }
        // Keep the existing mouse handler's DI independent of keyboard storage.
        rom[0x300..0x314].copy_from_slice(&[
            0x50, 0x53, 0xE4, 0x60, 0x8B, 0x1E, 0x00, 0x22, 0x88, 0x87, 0x00, 0x21, 0xFF, 0x06,
            0x00, 0x22, 0xB0, 0x20, 0xE6, 0x20,
        ]);
        rom[0x314..0x317].copy_from_slice(&[0x5B, 0x58, 0xCF]);
        let mut assembled = assemble_with_options(
            &config,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        #[cfg(feature = "reference-scheduler")]
        {
            assembled.machine = assembled.machine.with_scheduler_engine(engine);
        }
        assert_eq!(assembled.machine.scheduler_engine(), engine);
        let index = assembled.device_index[&DeviceId::Keyboard];
        run_chunk(&mut assembled, 30_000);
        collected(&assembled, &[0xFA, 0xAA, 0, 0xFA]);
        let mut keyboard = Vec::new();
        let mut mouse = vec![0xFA, 0xAA, 0, 0xFA];
        for (keys, dx, dy, buttons, packet) in [
            (&[0x1E, 0x9E][..], 5, -7, 1, [0x29, 5, 249]),
            (&[0x30, 0xB0][..], -2, 3, 6, [0x1E, 254, 3]),
            (&[0x2E, 0xAE][..], 0, 0, 0, [8, 0, 0]),
        ] {
            inject_scancodes(&mut assembled.machine, index, keys);
            assert!(inject_mouse(&mut assembled.machine, index, dx, dy, buttons));
            run_chunk(&mut assembled, 10_000);
            keyboard.extend_from_slice(keys);
            mouse.extend(packet);
            collected(&assembled, &mouse);
            let mut actual = vec![0; keyboard.len() + 1];
            assembled
                .machine
                .cpu()
                .read_mem(0x2100, &mut actual)
                .unwrap();
            assert_eq!(&actual[..keyboard.len()], keyboard);
            assert_eq!(actual[keyboard.len()], 0);
        }
    }
}
