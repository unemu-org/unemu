// SPDX-License-Identifier: AGPL-3.0-or-later
//! The session against a scripted machine: no platform, no host window.

use super::*;
use std::io::{Read, Write};
use unemu_core_api::{
    AudioDescription, DisplayDescription, Error, Features, Finished, Frame, InputPortDescription,
    MachineDescription, MessageLevel, NewMedium, Outputs, Rate, SlotDescription, SlotKind,
    SlotState, Status,
};
use winit::event::ElementState::{Pressed, Released};

/// A machine whose clock runs a million ticks a second and whose every run
/// reaches its deadline, unless a stop or a failure is scripted for it.
struct Scripted {
    description: MachineDescription,
    now: u64,
    status: Status,
    runs: Vec<RunLimit>,
    inputs: Vec<InputEvent>,
    changes: Vec<(SlotId, MediaAction)>,
    outputs: Vec<Outputs>,
    next: Vec<Result<Stop, Error>>,
    audio_failure: Option<ErrorKind>,
    messages: Vec<Message>,
    finish_warnings: Vec<String>,
    finished: u32,
    resets: u32,
    active: Vec<SlotId>,
    pixels: Vec<u32>,
}

fn scripted() -> Box<Scripted> {
    Box::new(Scripted {
        description: MachineDescription::new(Rate::new(1_000_000, 1), "step")
            .with_displays(vec![DisplayDescription::new(DisplayId(0), "Screen")])
            .with_audio(Some(AudioDescription::new(Rate::new(48_000, 1))))
            .with_inputs(vec![
                InputPortDescription::new(PortId(3), InputKind::Keyboard, "Keyboard"),
                InputPortDescription::new(
                    PortId(4),
                    InputKind::RelativePointer { buttons: 3 },
                    "Mouse",
                ),
            ])
            .with_slots(vec![
                SlotDescription::new(SlotId(7), SlotKind::FixedDisk, "C:", false),
                SlotDescription::new(SlotId(8), SlotKind::Floppy, "A:", true),
                SlotDescription::new(SlotId(9), SlotKind::Floppy, "B:", true),
            ])
            .with_features(Features::NONE.with_resets(&[ResetKind::Soft])),
        now: 0,
        status: Status::Ready,
        runs: Vec::new(),
        inputs: Vec::new(),
        changes: Vec::new(),
        outputs: Vec::new(),
        next: Vec::new(),
        audio_failure: None,
        messages: vec![Message::new(MessageLevel::Warning, "created")],
        finish_warnings: Vec::new(),
        finished: 0,
        resets: 0,
        active: Vec::new(),
        pixels: vec![0],
    })
}

impl Machine for Scripted {
    fn description(&self) -> &MachineDescription {
        &self.description
    }

    fn now(&self) -> Ticks {
        Ticks(self.now)
    }

    fn steps(&self) -> u64 {
        self.now
    }

    fn status(&self) -> Status {
        self.status
    }

    fn run(&mut self, limit: RunLimit) -> Result<Stop, Error> {
        self.runs.push(limit);
        if !self.next.is_empty() {
            let result = self.next.remove(0);
            self.status = match &result {
                Ok(Stop::Halted) => Status::Halted,
                Ok(Stop::PoweredOff) => Status::Off,
                Ok(_) => Status::Ready,
                Err(error) if error.kind == ErrorKind::Storage => self.status,
                Err(_) => Status::Failed,
            };
            return result;
        }
        self.now = limit.until.map_or(self.now, |until| until.0);
        Ok(Stop::Deadline)
    }

    fn frame(&mut self, _display: DisplayId) -> Result<Frame<'_>, Error> {
        self.pixels[0] = self.now as u32;
        Ok(Frame::new(1, 1, &self.pixels, (4, 3), self.now))
    }

    fn drain_audio(&mut self, out: &mut Vec<[i16; 2]>) -> Result<(), Error> {
        if let Some(kind) = self.audio_failure {
            return Err(Error::new(kind, "no sound"));
        }
        out.push([1, -1]);
        Ok(())
    }

    fn set_outputs(&mut self, outputs: Outputs) {
        self.outputs.push(outputs);
    }

    fn input(&mut self, event: InputEvent) {
        self.inputs.push(event);
    }

    fn slot_states(&mut self) -> Vec<SlotState> {
        let active = std::mem::take(&mut self.active);
        self.description
            .slots
            .iter()
            .map(|slot| SlotState::new(slot.id).with_active(active.contains(&slot.id)))
            .collect()
    }

    fn add_medium(&mut self, _slot: SlotId, _medium: NewMedium<'_>) -> Result<usize, Error> {
        Err(Error::new(ErrorKind::Unsupported, "no catalogue"))
    }

    fn change_medium(&mut self, slot: SlotId, action: MediaAction) -> Result<(), Error> {
        if slot == SlotId(9) {
            return Err(Error::new(ErrorKind::Usage, "drive B: is empty-handed"));
        }
        self.changes.push((slot, action));
        Ok(())
    }

    fn reset(&mut self, kind: ResetKind) -> Result<(), Error> {
        assert_eq!(kind, ResetKind::Soft);
        self.resets += 1;
        self.status = Status::Ready;
        self.messages
            .push(Message::new(MessageLevel::Note, "reset"));
        Ok(())
    }

    fn state_identity(&self) -> Option<String> {
        None
    }

    fn save_state(&mut self, _out: &mut dyn Write) -> Result<(), Error> {
        Err(Error::new(ErrorKind::Unsupported, "no state"))
    }

    fn load_state(&mut self, _state: &mut dyn Read) -> Result<(), Error> {
        Err(Error::new(ErrorKind::Unsupported, "no state"))
    }

    fn finish(&mut self) -> Result<Finished, Error> {
        self.finished += 1;
        Ok(Finished::new().with_warnings(self.finish_warnings.clone()))
    }

    fn drain_messages(&mut self, out: &mut Vec<Message>) {
        out.append(&mut self.messages);
    }
}

fn session(machine: Box<Scripted>) -> Session<Scripted> {
    let mut warnings = Vec::new();
    let session = Session::new(machine, DisplayId(0), &mut warnings).unwrap();
    assert_eq!(warnings, b"created\n");
    session
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

#[test]
fn a_session_runs_one_millisecond_chunks_to_the_paced_target_and_keeps_the_picture() {
    let mut session = session(scripted());
    session.advance_fully(ms(10)).unwrap();
    let machine = session.machine();
    assert_eq!(machine.now, 10_000);
    assert_eq!(machine.runs.len(), 10);
    assert_eq!(
        machine.runs[3],
        RunLimit::until(Ticks(4_000)),
        "chunks end a millisecond of guest time apart"
    );
    // Sound is drained before the first chunk and after every chunk.
    assert_eq!(machine.outputs.len(), 11);
    let picture = session.picture().unwrap();
    assert_eq!(
        (picture.width, picture.height, picture.serial),
        (1, 1, 10_000)
    );
    assert_eq!(picture.pixels, [10_000]);
    assert!(session.running);
    // A paused (occluded) session neither runs nor drains.
    session.occlusion_changed(true, ms(10));
    session.advance_fully(ms(30)).unwrap();
    assert_eq!(session.machine().runs.len(), 10);
    session.occlusion_changed(false, ms(40));
    session.advance_fully(ms(41)).unwrap();
    assert_eq!(session.machine().now, 11_000, "hidden time is discarded");
}

#[test]
fn a_halted_machine_waits_for_its_reset_and_a_guest_error_keeps_the_last_picture() {
    let mut machine = scripted();
    machine.next.push(Ok(Stop::Halted));
    let mut session = session(machine);
    session.advance_fully(ms(5)).unwrap();
    assert!(!session.running && session.halted);
    assert!(session.awaits_reset() && session.takes_media());
    assert_eq!(
        session.machine().finished,
        0,
        "a halted session ends at Stop"
    );
    session.media_events.push((SlotId(8), MediaAction::Next));
    let mut diagnostics = Vec::new();
    session.reset_halted(&mut diagnostics).unwrap();
    assert_eq!(diagnostics, b"reset\n");
    assert_eq!(session.machine().resets, 1);
    assert_eq!(session.machine().changes, [(SlotId(8), MediaAction::Next)]);
    assert!(session.running && !session.halted);
    session
        .machine_mut()
        .next
        .push(Err(Error::new(ErrorKind::Guest, "triple fault")));
    session.advance_fully(ms(6)).unwrap();
    assert!(!session.running);
    assert_eq!(session.error.as_ref().unwrap().to_string(), "triple fault");
    assert!(session.picture().is_some());
    assert!(!session.takes_media() && !session.awaits_reset());
    assert_eq!(
        session.machine().finished,
        0,
        "a failed session is never sealed"
    );
}

#[test]
fn a_machine_switched_off_is_finished_with_its_warnings() {
    let mut machine = scripted();
    machine.next.push(Ok(Stop::PoweredOff));
    machine.finish_warnings = vec!["journal kept".into()];
    let mut session = session(machine);
    let mut warnings = Vec::new();
    session
        .advance_fully_with_warnings(ms(5), &mut warnings)
        .unwrap();
    assert!(!session.running && !session.halted);
    assert_eq!(session.machine().finished, 1);
    assert_eq!(warnings, b"warning: journal kept\n");
}

#[test]
fn storage_failures_are_returned_and_audio_failures_are_reported_once() {
    let mut machine = scripted();
    machine.audio_failure = Some(ErrorKind::Unsupported);
    let mut session = session(machine);
    let mut warnings = Vec::new();
    session
        .advance_fully_with_warnings(ms(3), &mut warnings)
        .unwrap();
    session
        .advance_fully_with_warnings(ms(4), &mut warnings)
        .unwrap();
    assert_eq!(
        warnings,
        b"audio observation failed: no sound; continuing\n"
    );
    session.machine_mut().audio_failure = Some(ErrorKind::Storage);
    let error = session.advance_fully(ms(5)).unwrap_err();
    assert_eq!(error.to_string(), "no sound");
    session.machine_mut().audio_failure = None;
    session
        .machine_mut()
        .next
        .push(Err(Error::new(ErrorKind::Storage, "disk full")));
    assert_eq!(
        session.advance_fully(ms(6)).unwrap_err().to_string(),
        "disk full"
    );
    assert!(session.running && session.error.is_none());
}

#[test]
fn media_keys_change_the_first_and_second_removable_slot_and_never_reach_the_guest() {
    let mut session = session(scripted());
    for (key, state) in [
        (KeyCode::ControlLeft, Pressed),
        (KeyCode::AltLeft, Pressed),
        (KeyCode::KeyF, Pressed),
        (KeyCode::KeyF, Pressed),
        (KeyCode::KeyF, Released),
        (KeyCode::ShiftLeft, Pressed),
        (KeyCode::KeyE, Pressed),
        (KeyCode::KeyE, Released),
    ] {
        session.key_event(key, state);
    }
    assert_eq!(
        session.media_events,
        [
            (SlotId(8), MediaAction::Next),
            (SlotId(9), MediaAction::Eject)
        ]
    );
    let keys: Vec<_> = session
        .machine()
        .inputs
        .iter()
        .map(|event| match event {
            InputEvent::Key { port, key, state } => (*port, key.0, *state),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        keys,
        [
            (PortId(3), 0xE0, KeyState::Pressed),
            (PortId(3), 0xE2, KeyState::Pressed),
            (PortId(3), 0xE1, KeyState::Pressed),
        ],
        "the guest sees the modifiers only"
    );
    // A slot that cannot change now is skipped.
    session.apply_media_events().unwrap();
    assert_eq!(session.machine().changes, [(SlotId(8), MediaAction::Next)]);
}

#[test]
fn focus_loss_releases_everything_once_and_capture_forwards_counts_and_buttons() {
    let mut session = session(scripted());
    session.mouse.pointer_inside(true);
    session.mouse_button_with_grab(MouseButton::Left, Pressed, || Ok(()));
    session.mouse_button_with_grab(MouseButton::Left, Released, || Ok(()));
    assert!(session.mouse.captured());
    session.mouse_motion(1.5, -2.25);
    session.mouse_motion(0.5, -0.75);
    session.mouse_button_with_grab(MouseButton::Right, Pressed, || Ok(()));
    session.key_event(KeyCode::KeyA, Pressed);
    session.focus_changed(false);
    assert!(!session.mouse.captured());
    let pointer = PortId(4);
    assert_eq!(
        session.machine().inputs,
        [
            InputEvent::PointerMotion {
                port: pointer,
                dx: 1,
                dy: 2
            },
            InputEvent::PointerMotion {
                port: pointer,
                dx: 1,
                dy: 1
            },
            InputEvent::PointerButton {
                port: pointer,
                button: 1,
                pressed: true
            },
            InputEvent::Key {
                port: PortId(3),
                key: unemu_core_api::Key::A,
                state: KeyState::Pressed
            },
            InputEvent::ReleaseAll,
        ]
    );
    // A stopped session takes no input; its capture release still lifts the
    // guest's buttons.
    session.focus_changed(true);
    session.mouse.pointer_inside(true);
    session.mouse_button_with_grab(MouseButton::Left, Pressed, || Ok(()));
    session.mouse_button_with_grab(MouseButton::Left, Released, || Ok(()));
    session.mouse_button_with_grab(MouseButton::Middle, Pressed, || Ok(()));
    session.running = false;
    session.key_event(KeyCode::KeyB, Pressed);
    session.release_pointer();
    let inputs = &session.machine().inputs;
    assert_eq!(
        inputs[inputs.len() - 2..],
        [
            InputEvent::PointerButton {
                port: pointer,
                button: 2,
                pressed: true
            },
            InputEvent::PointerButton {
                port: pointer,
                button: 2,
                pressed: false
            },
        ]
    );
}

#[test]
fn the_drive_lights_take_each_busy_slot_once() {
    let mut session = session(scripted());
    session.machine_mut().active = vec![SlotId(7), SlotId(9)];
    assert_eq!(session.take_slot_activity(), [SlotId(7), SlotId(9)]);
    assert!(session.take_slot_activity().is_empty());
}

#[test]
fn sound_at_a_rate_the_window_does_not_play_disables_audio_with_one_warning() {
    let mut machine = scripted();
    machine.description = machine
        .description
        .clone()
        .with_audio(Some(AudioDescription::new(Rate::new(44_100, 1))));
    let mut session = session(machine);
    let mut warnings = Vec::new();
    session.audio.open(&mut warnings);
    session.audio.open(&mut warnings);
    session
        .advance_fully_with_warnings(ms(2), &mut warnings)
        .unwrap();
    assert_eq!(
        String::from_utf8(warnings).unwrap(),
        "audio disabled: the machine's sound runs at 44100/1 Hz; the window plays 48000 Hz\n"
    );
    assert!(!session.audio.available());
    assert!(session
        .machine()
        .outputs
        .iter()
        .all(|outputs| !outputs.audio));
}
