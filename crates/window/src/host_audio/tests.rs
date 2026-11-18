// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host audio of a running PC session; they never open a host device.

use cpu::Cpu;
use std::io;

fn presenter() -> crate::Presenter {
    let mut rom = vec![0u8; 0x8000];
    rom[..2].copy_from_slice(&[0xEB, 0xFE]);
    let mut config = assembly::bios(rom, None);
    config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
    crate::Presenter::new(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
        &mut io::sink(),
    )
    .unwrap()
}

#[test]
fn presenter_wiring_is_inert_until_enabled_and_captures_each_run_boundary() {
    for rate in [48000, 44100, 16000] {
        let mut silent = presenter();
        let mut playing = presenter();
        playing.audio.configure_for_test(rate);
        assert_eq!(playing.audio.opened_for_test(), (false, false));
        assert!(!playing.audio.enabled());
        for host in [&mut silent, &mut playing] {
            host.assembled()
                .add_audio_source(Box::new(audio::EdgeSource::new([1234, -2345])));
        }
        // Simulate a consumer without enumerating or opening a physical device.
        playing.audio.set_active_for_test(true);
        for millis in [20, 40, 60] {
            silent
                .advance_fully(std::time::Duration::from_millis(millis))
                .unwrap();
            playing
                .advance_fully(std::time::Duration::from_millis(millis))
                .unwrap();
            let clock = playing.assembled_ref().machine.clock();
            assert_eq!(silent.assembled_ref().machine.clock(), clock);
            assert_eq!(
                silent.assembled_ref().machine.virtual_time(),
                playing.assembled_ref().machine.virtual_time()
            );
            assert_eq!(
                silent.assembled_ref().machine.cpu().retired(),
                playing.assembled_ref().machine.cpu().retired()
            );
            let input_frames = audio::completed_frames(clock);
            let expected = (input_frames - 1) * u64::from(rate) / 48000 + 1;
            assert_eq!(playing.audio.queued_for_test(), expected as usize);
            assert_eq!(silent.audio.queued_for_test(), 0);
        }
        let mut output = [0i16; 2];
        playing.audio.read_for_test(&mut output, 2);
        assert_eq!(output, [1234, -2345]);
    }
}

#[test]
fn mute_chord_is_host_only_and_f12_reaches_the_guest() {
    use winit::{
        event::ElementState::{Pressed, Released},
        keyboard::KeyCode::*,
    };
    let mut host = presenter();
    assert_eq!(host.audio.status_text(), "Audio unavailable");
    host.audio.set_active_for_test(true);
    host.key_event(ControlLeft, Pressed);
    host.key_event(AltRight, Pressed);
    let pending = host.keyboard().pending_events();
    host.key_event(KeyM, Pressed);
    host.key_event(KeyM, Pressed);
    assert!(host.audio.muted());
    host.key_event(KeyM, Released);
    assert_eq!(host.keyboard().pending_events(), pending);
    host.key_event(KeyM, Pressed);
    host.key_event(KeyM, Released);
    assert!(!host.audio.muted());
    host.key_event(F12, Pressed);
    host.key_event(F12, Released);
    assert_eq!(host.keyboard().pending_events(), pending + 2);
    host.audio.set_active_for_test(false);
    host.key_event(KeyM, Pressed);
    host.key_event(KeyM, Released);
    assert_eq!(host.keyboard().pending_events(), pending + 4);
}

#[test]
fn held_guest_m_is_not_stolen_when_modifiers_arrive_later() {
    use winit::{
        event::ElementState::{Pressed, Released},
        keyboard::KeyCode::*,
    };
    let mut host = presenter();
    host.audio.set_active_for_test(true);
    host.key_event(KeyM, Pressed);
    host.key_event(ControlRight, Pressed);
    host.key_event(AltLeft, Pressed);
    host.key_event(KeyM, Pressed);
    host.key_event(KeyM, Released);
    assert!(!host.audio.muted());
    assert_eq!(host.keyboard().pending_events(), 5);
    host.focus_changed(false);
    assert!(host.mute_chord_idle());
}

#[test]
fn presenter_reports_latched_audio_errors_once_and_recovers() {
    let mut host = presenter();
    host.audio.set_active_for_test(true);
    let edges = audio::EdgeSource::new([100, -100]);
    host.assembled().add_audio_source(Box::new(edges.clone()));
    let mut future = host.assembled_ref().machine.clock();
    future.0 += 1;
    edges.record_level(future, [200, -200]);
    edges.record_level(host.assembled_ref().machine.clock(), [300, -300]);
    let mut warnings = Vec::new();
    host.advance_fully_with_warnings(std::time::Duration::from_millis(20), &mut warnings)
        .unwrap();
    assert!(host.running);
    assert!(host.error.is_none());
    assert!(host.audio.available());
    assert!(host.audio.queued_for_test() > 0);
    // The mixer has now observed this time, so another old device edge is late.
    edges.record_level(future, [400, -400]);
    host.advance_fully_with_warnings(std::time::Duration::from_millis(40), &mut warnings)
        .unwrap();
    assert!(host.running);
    assert_eq!(String::from_utf8(warnings).unwrap().lines().count(), 1);
}

fn speaker_presenter() -> crate::Presenter {
    // Program channel 2 and repeatedly toggle speaker data while GATE stays high.
    let program = [
        0xFA, 0xB0, 0xB6, 0xE6, 0x43, 0xB0, 5, 0xE6, 0x42, 0xB0, 0, 0xE6, 0x42, 0xB0, 3, 0xE6,
        0x61, 0x34, 2, 0xEB, 0xFA,
    ];
    let mut rom = vec![0u8; 0x8000];
    rom[..program.len()].copy_from_slice(&program);
    let mut config = assembly::bios(rom, None);
    config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
    crate::Presenter::new(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
        &mut io::sink(),
    )
    .unwrap()
}

#[test]
fn unmute_refreshes_trimmed_speaker_history_before_the_next_run() {
    let mut actual = speaker_presenter();
    let mut reference = speaker_presenter();
    for host in [&mut actual, &mut reference] {
        host.audio.set_active_for_test(true);
        host.audio.toggle_mute();
        host.advance_fully(std::time::Duration::from_millis(20))
            .unwrap();
        assert_eq!(host.audio.queued_for_test(), 0);
        host.audio.toggle_mute();
    }
    // This independent explicit flush, with the session's host output
    // enabled again, is the expected output-enable boundary.
    unemu_core_api::Machine::set_outputs(
        reference.machine_mut(),
        unemu_core_api::Outputs::NONE.with_audio(true),
    );
    reference.assembled().capture_audio().unwrap();
    for host in [&mut actual, &mut reference] {
        host.advance_fully(std::time::Duration::from_millis(40))
            .unwrap();
    }
    let snapshot = |host: &crate::Presenter| host.audio.drain_for_test();
    let actual = snapshot(&actual);
    assert!(!actual.is_empty());
    assert!(actual.iter().any(|frame| frame[0] > 0 && frame[0] < 16384));
    assert_eq!(actual, snapshot(&reference));
}

#[test]
fn speed_mute_labels_explain_the_user_preference_toggle() {
    let mut host = speaker_presenter();
    host.audio.set_active_for_test(true);
    host.select_speed(crate::Speed::Double, std::time::Duration::ZERO);
    assert_eq!(host.audio.status_text(), "Audio muted (speed)");
    assert_eq!(host.audio.mute_label(), "User mute: off (speed muted)");
    host.audio.toggle_mute();
    assert!(host.audio.muted());
    assert_eq!(host.audio.status_text(), "Audio muted (speed + user)");
    assert_eq!(host.audio.mute_label(), "User mute: on (speed muted)");
    host.select_speed(crate::Speed::Normal, std::time::Duration::ZERO);
    assert_eq!(host.audio.status_text(), "Audio muted (user)");
    assert!(host.audio.muted());
    host.select_speed(crate::Speed::Double, std::time::Duration::ZERO);
    host.audio.toggle_mute();
    assert_eq!(host.audio.status_text(), "Audio muted (speed)");
    host.select_speed(crate::Speed::Normal, std::time::Duration::ZERO);
    assert!(!host.audio.muted());
    assert_eq!(host.audio.status_text(), "Audio on");
}
