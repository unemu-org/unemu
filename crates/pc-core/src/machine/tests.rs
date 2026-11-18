// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use assembly::{ObservationPolicy, ObservationRequest};
use unemu_core_api::{Core, Key, KeyState, CONTRACT};

fn observed(config: &MachineConfig, display: DeviceId) -> AssembledMachine {
    assembly::assemble_with_observation_and_options(
        config,
        &ObservationPolicy {
            routes: vec![ObservationRequest {
                device: display,
                output: OutputKind::TextScanout,
                byte_limit: 0,
            }],
            ..ObservationPolicy::default()
        },
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap()
}

fn key(key: Key, state: KeyState) -> InputEvent {
    InputEvent::Key {
        port: KEYBOARD_PORT,
        key,
        state,
    }
}

fn keyboard_state(assembled: &AssembledMachine) -> Vec<u8> {
    let index = assembled.device_index[&DeviceId::Keyboard];
    assembled.machine.bus().device(index).unwrap().save()
}

/// The same keys and the same chunks, once through `dyn Machine` and once
/// through the assembled machine with the encoder and the cap the desktop
/// drove it with before the contract existed, end in the same machine.
#[test]
fn a_session_through_the_contract_matches_the_assembled_machine() {
    let config = assembly::at_keyboard(vec![0xEB, 0xFE]);
    let mut machine =
        PcMachine::from_assembled(&config, assembly::assemble(&config).unwrap()).unwrap();
    let mut direct = assembly::assemble(&config).unwrap();
    direct.machine.set_idle_limit(Some(u64::MAX));
    let mut keyboard = HostKeyboard::default();
    let rate = CpuRate {
        numerator: NonZeroU64::new(config.clock.retired_per_pit_clock).unwrap(),
        denominator: NonZeroU64::new(config.clock.retired_per_pit_clock_denominator).unwrap(),
    };
    let script = [
        (Key::SHIFT_LEFT, KeyState::Pressed),
        (Key::A, KeyState::Pressed),
        (Key::A, KeyState::Repeated),
        (Key::A, KeyState::Released),
        (Key::SHIFT_LEFT, KeyState::Released),
        (Key::ARROW_UP, KeyState::Pressed),
        (Key::ARROW_UP, KeyState::Released),
    ];
    let pristine = keyboard_state(&direct);
    for (round, (pressed, state)) in script.into_iter().enumerate() {
        let contract: &mut dyn Machine = &mut machine;
        contract.input(key(pressed, state));
        keyboard.key_event(pressed, state);
        let until = (round as u64 + 1) * 5_000;
        assert_eq!(
            contract.run(RunLimit::until(Ticks(until))),
            Ok(Stop::Deadline)
        );
        keyboard.inject_at_boundary(&mut direct);
        let clock = direct.machine.clock().0;
        let time = direct.machine.virtual_time();
        let cap = run_cap(until, clock, time, rate);
        assert_eq!(
            direct.machine.run_bounded(Some(cap)).unwrap(),
            RunOutcome::RetiredCapReached
        );
        assert_eq!(contract.now(), Ticks(direct.machine.clock().0));
        assert_eq!(contract.steps(), direct.machine.virtual_time());
        assert_eq!(contract.status(), Status::Ready);
        assert_eq!(
            keyboard_state(machine.assembled_ref()),
            keyboard_state(&direct)
        );
    }
    assert_ne!(
        keyboard_state(&direct),
        pristine,
        "the keys reached the 8042"
    );
}

#[test]
fn the_picture_is_the_rasterised_text_of_the_completed_boundary() {
    let config = assembly::at_vga_text(vec![0xC7, 0x06, 0, 0, 0xDB, 0x1F, 0xF4]);
    let mut machine = PcMachine::from_assembled(&config, observed(&config, DeviceId::Vga)).unwrap();
    assert_eq!(machine.description().displays.len(), 1);
    assert_eq!(machine.description().displays[0].label, "VGA");
    assert_eq!(machine.run(RunLimit::until(Ticks(1_000))), Ok(Stop::Halted));
    assert_eq!(machine.status(), Status::Halted);
    assert_eq!(machine.run(RunLimit::until(Ticks(2_000))), Ok(Stop::Halted));
    let frame = machine.frame(DisplayId(0)).unwrap();
    assert_eq!((frame.width, frame.height), (1280, 800));
    assert_eq!(frame.aspect, (4, 3));
    let first = frame.serial;
    for y in 0..800 {
        for x in 0..1280 {
            assert_eq!(
                frame.pixels[y * 1280 + x],
                if y < 32 && x < 16 { 0xFFFFFF } else { 0 }
            );
        }
    }
    assert_eq!(
        machine.frame(DisplayId(0)).unwrap().serial,
        first,
        "an unchanged screen keeps its serial"
    );
    assert_eq!(
        machine
            .run(RunLimit::until(Ticks(3_000)).with_stop_on_frame(true))
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
    assert!(!machine.description().features.frame_stops);
    assert_eq!(
        machine.frame(DisplayId(1)).unwrap_err().kind,
        ErrorKind::Unsupported
    );
}

#[test]
fn the_reset_button_restarts_only_a_halted_pc() {
    let mut rom = vec![0; 0x8000];
    rom[..5].copy_from_slice(&[0xB0, 0xFE, 0xE6, 0x64, 0xF4]);
    rom[0x7FF0..0x7FF4].copy_from_slice(&[0xB0, 0x55, 0xFA, 0xF4]);
    let mut config = assembly::bios(rom, None);
    config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
    let mut machine = PcMachine::from_assembled(&config, observed(&config, DeviceId::Mda)).unwrap();
    assert_eq!(
        machine.reset(ResetKind::Soft).unwrap_err().kind,
        ErrorKind::Usage
    );
    assert_eq!(
        machine.reset(ResetKind::Hard).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    // The guest pulses the 8042 reset line, then halts with interrupts off
    // at the reset vector: the reset happened inside the run.
    assert_eq!(
        machine.run(RunLimit::until(Ticks(10_000))),
        Ok(Stop::Halted)
    );
    assert_eq!(machine.reset_requests(), 1);
    assert_eq!(machine.cpu_resets(), 1);
    let mut messages = Vec::new();
    machine.drain_messages(&mut messages);
    assert_eq!(messages, [Message::new(MessageLevel::Note, "cpu reset")]);
    machine.reset(ResetKind::Soft).unwrap();
    assert_eq!(machine.status(), Status::Ready);
    assert_eq!(machine.cpu_resets(), 2);
    assert_eq!(
        machine.reset_summary().as_deref(),
        Some("cpu resets in session: 2")
    );
    machine.drain_messages(&mut messages);
    assert_eq!(messages.len(), 1, "only the first reset is reported");
    assert_eq!(machine.finish(), Ok(Finished::new()));
}

#[test]
fn the_description_lists_the_configured_drives_ports_and_clock() {
    let mut config = assembly::bios(vec![0; 0x8000], Some(vec![0; 1_474_560]));
    config.drives[0].write = assembly::WriteMode::ReadOnly;
    let machine = PcMachine::from_assembled(&config, observed(&config, DeviceId::Mda)).unwrap();
    let description = machine.description();
    assert_eq!(description.clock, Rate::new(105_000_000, 88));
    assert_eq!(description.step_unit, "instruction");
    assert_eq!(
        description.audio,
        Some(AudioDescription::new(Rate::new(48_000, 1)))
    );
    assert_eq!(description.features, PC_FEATURES);
    let slots: Vec<_> = description
        .slots
        .iter()
        .map(|slot| (slot.id, slot.kind, slot.label.as_str(), slot.removable))
        .collect();
    assert_eq!(slots, [(SlotId(0), SlotKind::Floppy, "A:", true)]);
    assert!(description
        .inputs
        .iter()
        .any(|port| port.id == KEYBOARD_PORT && port.kind == InputKind::Keyboard));
    let mut machine = machine;
    let slots = machine.slot_states();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].inserted, Some(0));
    let writes: Vec<_> = slots[0]
        .catalogue
        .iter()
        .map(|medium| medium.writes)
        .collect();
    assert_eq!(writes, [WriteKind::ReadOnly]);
    let mut messages = Vec::new();
    machine.drain_messages(&mut messages);
    assert_eq!(messages.len(), 1);
    assert!(messages[0]
        .text
        .starts_with("warning: running unlisted firmware"));
}

#[test]
fn a_drive_without_a_controller_refuses_changes_as_a_usage_error() {
    let config = assembly::at_keyboard(vec![0xEB, 0xFE]);
    let mut machine =
        PcMachine::from_assembled(&config, assembly::assemble(&config).unwrap()).unwrap();
    assert_eq!(
        machine
            .change_medium(SlotId(0), MediaAction::Next)
            .unwrap_err()
            .kind,
        ErrorKind::Usage
    );
    assert_eq!(
        machine
            .change_medium(SlotId(2), MediaAction::Eject)
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
    assert!(machine.slot_states().is_empty());
}

#[test]
fn the_pc_core_describes_its_platform_and_creates_no_machine_yet() {
    let core: &dyn Core = &crate::PcCore;
    let description = core.description();
    assert!(CONTRACT.drives(description.implements));
    assert_eq!(description.id, "unemupc");
    assert_eq!(description.platforms.len(), 1);
    assert_eq!(description.platforms[0].id, "ibm-pc");
    assert!(description.platforms[0].config_types.is_empty());
    assert!(description
        .notices
        .iter()
        .any(|notice| notice.text == crate::FONT_NOTICE));
    struct Nothing;
    impl unemu_core_api::ContentSource for Nothing {
        fn open(&self, _sha256: &[u8; 32], _size: u64) -> Result<unemu_core_api::Content, Error> {
            Err(Error::new(ErrorKind::Content, "nothing"))
        }
    }
    let request = unemu_core_api::CreateRequest::new(
        "ibm-pc",
        unemu_core_api::ConfigDocument::new("application/toml", b""),
        &Nothing,
        std::path::Path::new("session"),
    );
    assert_eq!(
        core.create(&request).err().map(|error| error.kind),
        Some(ErrorKind::Unsupported)
    );
}
