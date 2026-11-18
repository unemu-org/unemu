// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keys by their USB HID usage to IBM PC scan code set 1 (design/spec.md §7.5).
//!
//! IBM Personal System/2 Hardware Interface Technical Reference — Common
//! Interfaces (October 1990), "Keyboards (101- and 102-Key)", "Set 1 Scan-Code
//! Tables", pp. 27–30, defines make/break codes and E0 extensions. Host positions
//! follow the US keyboard; the guest interprets modifiers and locks. No character
//! or host-layout translation occurs.
//! Figures 8–9 define artificial Shift wrappers for navigation and keypad
//! Divide; these prevent BIOS lock translation from turning grey arrows into
//! digits. Shift-held state comes from physical events. NumLock follows the
//! guest's keyboard LED parameter (Hex ED, bit 1), read when encoding the next
//! key at a run boundary. Ctrl+NumLock and guest reboots cannot drift a host copy.
//! Only the first make carries a fake-Shift wrapper; repeats use the base make,
//! as described in "Scan Codes", "Set 1 Scan-Code Tables" (typematic codes).
//! Pause/Break and PrintScreen are intentionally unmapped: their special
//! multi-byte, modifier-dependent protocols are outside this simple mapping.

use assembly::{keyboard_leds_if_ready, try_inject_scancode, AssembledMachine, DeviceId};
use std::collections::{BTreeSet, VecDeque};
use unemu_core_api::{Key, KeyState};

/// Encode the base physical make/break; unsupported keys produce no bytes.
/// [`HostKeyboard`] adds the state-dependent artificial Shift wrappers.
/// A repeated press emits the same make sequence, without an intervening
/// break, implementing host-driven typematic. Guest typematic-rate commands do
/// not change the host repeat rate. E0 precedes both make and break codes.
/// A host repeat ([`KeyState::Repeated`]) is a press.
pub fn set1_scancodes(key: Key, state: KeyState) -> Option<Vec<u8>> {
    let (code, extended) = match key {
        Key::ESCAPE => (0x01, false),
        Key::DIGIT_1 => (0x02, false),
        Key::DIGIT_2 => (0x03, false),
        Key::DIGIT_3 => (0x04, false),
        Key::DIGIT_4 => (0x05, false),
        Key::DIGIT_5 => (0x06, false),
        Key::DIGIT_6 => (0x07, false),
        Key::DIGIT_7 => (0x08, false),
        Key::DIGIT_8 => (0x09, false),
        Key::DIGIT_9 => (0x0A, false),
        Key::DIGIT_0 => (0x0B, false),
        Key::MINUS => (0x0C, false),
        Key::EQUAL => (0x0D, false),
        Key::BACKSPACE => (0x0E, false),
        Key::TAB => (0x0F, false),
        Key::Q => (0x10, false),
        Key::W => (0x11, false),
        Key::E => (0x12, false),
        Key::R => (0x13, false),
        Key::T => (0x14, false),
        Key::Y => (0x15, false),
        Key::U => (0x16, false),
        Key::I => (0x17, false),
        Key::O => (0x18, false),
        Key::P => (0x19, false),
        Key::BRACKET_LEFT => (0x1A, false),
        Key::BRACKET_RIGHT => (0x1B, false),
        Key::ENTER => (0x1C, false),
        Key::CONTROL_LEFT => (0x1D, false),
        Key::A => (0x1E, false),
        Key::S => (0x1F, false),
        Key::D => (0x20, false),
        Key::F => (0x21, false),
        Key::G => (0x22, false),
        Key::H => (0x23, false),
        Key::J => (0x24, false),
        Key::K => (0x25, false),
        Key::L => (0x26, false),
        Key::SEMICOLON => (0x27, false),
        Key::QUOTE => (0x28, false),
        Key::BACKQUOTE => (0x29, false),
        Key::SHIFT_LEFT => (0x2A, false),
        Key::BACKSLASH => (0x2B, false),
        Key::Z => (0x2C, false),
        Key::X => (0x2D, false),
        Key::C => (0x2E, false),
        Key::V => (0x2F, false),
        Key::B => (0x30, false),
        Key::N => (0x31, false),
        Key::M => (0x32, false),
        Key::COMMA => (0x33, false),
        Key::PERIOD => (0x34, false),
        Key::SLASH => (0x35, false),
        Key::SHIFT_RIGHT => (0x36, false),
        Key::NUMPAD_MULTIPLY => (0x37, false),
        Key::ALT_LEFT => (0x38, false),
        Key::SPACE => (0x39, false),
        Key::CAPS_LOCK => (0x3A, false),
        Key::F1 => (0x3B, false),
        Key::F2 => (0x3C, false),
        Key::F3 => (0x3D, false),
        Key::F4 => (0x3E, false),
        Key::F5 => (0x3F, false),
        Key::F6 => (0x40, false),
        Key::F7 => (0x41, false),
        Key::F8 => (0x42, false),
        Key::F9 => (0x43, false),
        Key::F10 => (0x44, false),
        Key::NUM_LOCK => (0x45, false),
        Key::SCROLL_LOCK => (0x46, false),
        Key::NUMPAD_7 => (0x47, false),
        Key::NUMPAD_8 => (0x48, false),
        Key::NUMPAD_9 => (0x49, false),
        Key::NUMPAD_SUBTRACT => (0x4A, false),
        Key::NUMPAD_4 => (0x4B, false),
        Key::NUMPAD_5 => (0x4C, false),
        Key::NUMPAD_6 => (0x4D, false),
        Key::NUMPAD_ADD => (0x4E, false),
        Key::NUMPAD_1 => (0x4F, false),
        Key::NUMPAD_2 => (0x50, false),
        Key::NUMPAD_3 => (0x51, false),
        Key::NUMPAD_0 => (0x52, false),
        Key::NUMPAD_DECIMAL => (0x53, false),
        Key::NON_US_BACKSLASH => (0x56, false),
        Key::F11 => (0x57, false),
        Key::F12 => (0x58, false),
        Key::NUMPAD_ENTER => (0x1C, true),
        Key::CONTROL_RIGHT => (0x1D, true),
        Key::NUMPAD_DIVIDE => (0x35, true),
        Key::ALT_RIGHT => (0x38, true),
        Key::HOME => (0x47, true),
        Key::ARROW_UP => (0x48, true),
        Key::PAGE_UP => (0x49, true),
        Key::ARROW_LEFT => (0x4B, true),
        Key::ARROW_RIGHT => (0x4D, true),
        Key::END => (0x4F, true),
        Key::ARROW_DOWN => (0x50, true),
        Key::PAGE_DOWN => (0x51, true),
        Key::INSERT => (0x52, true),
        Key::DELETE => (0x53, true),
        _ => return None,
    };
    let code = code | if state == KeyState::Released { 0x80 } else { 0 };
    Some(if extended {
        vec![0xE0, code]
    } else {
        vec![code]
    })
}

/// The order [`HostKeyboard::release_all`] releases held keys in: the order
/// the desktop has always used, letters and punctuation first, then the
/// modifiers and editing keys, navigation, the keypad and the function keys.
/// A fixed order keeps the breaks of a focus loss independent of the host.
const RELEASE_ORDER: [Key; 100] = [
    Key::BACKQUOTE,
    Key::BACKSLASH,
    Key::BRACKET_LEFT,
    Key::BRACKET_RIGHT,
    Key::COMMA,
    Key::DIGIT_0,
    Key::DIGIT_1,
    Key::DIGIT_2,
    Key::DIGIT_3,
    Key::DIGIT_4,
    Key::DIGIT_5,
    Key::DIGIT_6,
    Key::DIGIT_7,
    Key::DIGIT_8,
    Key::DIGIT_9,
    Key::EQUAL,
    Key::NON_US_BACKSLASH,
    Key::A,
    Key::B,
    Key::C,
    Key::D,
    Key::E,
    Key::F,
    Key::G,
    Key::H,
    Key::I,
    Key::J,
    Key::K,
    Key::L,
    Key::M,
    Key::N,
    Key::O,
    Key::P,
    Key::Q,
    Key::R,
    Key::S,
    Key::T,
    Key::U,
    Key::V,
    Key::W,
    Key::X,
    Key::Y,
    Key::Z,
    Key::MINUS,
    Key::PERIOD,
    Key::QUOTE,
    Key::SEMICOLON,
    Key::SLASH,
    Key::ALT_LEFT,
    Key::ALT_RIGHT,
    Key::BACKSPACE,
    Key::CAPS_LOCK,
    Key::CONTROL_LEFT,
    Key::CONTROL_RIGHT,
    Key::ENTER,
    Key::SHIFT_LEFT,
    Key::SHIFT_RIGHT,
    Key::SPACE,
    Key::TAB,
    Key::DELETE,
    Key::END,
    Key::HOME,
    Key::INSERT,
    Key::PAGE_DOWN,
    Key::PAGE_UP,
    Key::ARROW_DOWN,
    Key::ARROW_LEFT,
    Key::ARROW_RIGHT,
    Key::ARROW_UP,
    Key::NUM_LOCK,
    Key::NUMPAD_0,
    Key::NUMPAD_1,
    Key::NUMPAD_2,
    Key::NUMPAD_3,
    Key::NUMPAD_4,
    Key::NUMPAD_5,
    Key::NUMPAD_6,
    Key::NUMPAD_7,
    Key::NUMPAD_8,
    Key::NUMPAD_9,
    Key::NUMPAD_ADD,
    Key::NUMPAD_DECIMAL,
    Key::NUMPAD_DIVIDE,
    Key::NUMPAD_ENTER,
    Key::NUMPAD_MULTIPLY,
    Key::NUMPAD_SUBTRACT,
    Key::ESCAPE,
    Key::SCROLL_LOCK,
    Key::F1,
    Key::F2,
    Key::F3,
    Key::F4,
    Key::F5,
    Key::F6,
    Key::F7,
    Key::F8,
    Key::F9,
    Key::F10,
    Key::F11,
    Key::F12,
];

#[derive(Clone, Copy)]
struct KeyTransition {
    key: Key,
    state: KeyState,
    repeat: bool,
    left_shift: bool,
    right_shift: bool,
}

const CTRL_ALT_DEL: [u8; 6] = [0x1D, 0x38, 0x53, 0xD3, 0xB8, 0x9D];

enum HostInput {
    Key(KeyTransition),
    CtrlAltDel,
}

/// Host-owned event FIFO; byte encoding waits for guest LEDs at a run boundary.
/// Enqueueing never borrows or advances the guest machine.
#[derive(Default)]
pub struct HostKeyboard {
    pending: VecDeque<u8>,
    held: BTreeSet<Key>,
    events: VecDeque<HostInput>,
    typed_ranges: Vec<std::ops::Range<usize>>,
}

impl HostKeyboard {
    /// Queue a physical key event, including repeated makes. A release for a
    /// key not held is ignored, including synthetic releases after focus loss.
    pub fn key_event(&mut self, key: Key, state: KeyState) {
        if set1_scancodes(key, state).is_none() {
            return;
        }
        let state = if state == KeyState::Repeated {
            KeyState::Pressed
        } else {
            state
        };
        if state == KeyState::Released && !self.held.remove(&key) {
            return;
        }
        let repeat = state == KeyState::Pressed && !self.held.insert(key);
        self.events.push_back(HostInput::Key(KeyTransition {
            key,
            state,
            repeat,
            left_shift: self.held.contains(&Key::SHIFT_LEFT),
            right_shift: self.held.contains(&Key::SHIFT_RIGHT),
        }));
    }

    /// Queue one complete keyboard-owned reset chord after releasing held keys.
    /// At the next boundary the whole chord transfers even while inhibited.
    /// The keypad Delete codes avoid navigation wrappers tied to guest NumLock.
    /// IBM PS/2 Hardware Interface Technical Reference, Keyboards, Set 1
    /// Scan-Code Tables, defines these makes and their high-bit breaks.
    pub fn ctrl_alt_del(&mut self) {
        self.release_all();
        self.events.push_back(HostInput::CtrlAltDel);
    }

    fn encode(&mut self, input: HostInput, num_lock: bool) {
        let event = match input {
            HostInput::Key(event) => event,
            HostInput::CtrlAltDel => {
                self.pending.extend(CTRL_ALT_DEL);
                return;
            }
        };
        let KeyTransition {
            key,
            state,
            repeat,
            left_shift: left,
            right_shift: right,
        } = event;
        let Some(mut bytes) = set1_scancodes(key, state) else {
            return;
        };
        let navigation = matches!(
            key,
            Key::INSERT
                | Key::DELETE
                | Key::HOME
                | Key::END
                | Key::PAGE_UP
                | Key::PAGE_DOWN
                | Key::ARROW_UP
                | Key::ARROW_DOWN
                | Key::ARROW_LEFT
                | Key::ARROW_RIGHT
        );
        let divide = key == Key::NUMPAD_DIVIDE;
        let mut shift_codes = Vec::new();
        if divide || (navigation && !num_lock) {
            if left {
                shift_codes.push(0xAA);
            }
            if right {
                shift_codes.push(0xB6);
            }
        }
        if navigation && num_lock && !left && !right {
            shift_codes.push(0x2A);
        }
        if state == KeyState::Pressed {
            if !repeat {
                for code in shift_codes {
                    self.pending.extend([0xE0, code]);
                }
            }
            self.pending.extend(bytes);
        } else {
            for code in shift_codes {
                bytes.extend([0xE0, code ^ 0x80]);
            }
            self.pending.extend(bytes);
        }
    }

    /// Release non-Shift keys first so their artificial Shift restorations are
    /// followed by the physical Shift breaks. The keys go up in one fixed,
    /// host-independent order: letters and punctuation first, the function
    /// keys last.
    pub fn release_all(&mut self) {
        let mut keys: Vec<_> = self.held.iter().copied().collect();
        keys.sort_by_key(|key| RELEASE_ORDER.iter().position(|listed| listed == key));
        for &key in &keys {
            if !matches!(key, Key::SHIFT_LEFT | Key::SHIFT_RIGHT) {
                self.key_event(key, KeyState::Released);
            }
        }
        for key in [Key::SHIFT_LEFT, Key::SHIFT_RIGHT] {
            self.key_event(key, KeyState::Released);
        }
    }

    /// Number of key transitions waiting, including a partially delivered sequence.
    pub fn pending_events(&self) -> usize {
        self.events.len() + usize::from(!self.pending.is_empty())
    }

    /// Transfer in order immediately before a run chunk, retaining bytes when
    /// the 8042 cannot accept input yet. The machine cannot run concurrently:
    /// the frontend and guest share one thread and this exclusive borrow.
    /// Reset chords transfer as a whole to the keyboard buffer even if inhibited;
    /// their releases must already belong to the keyboard when POST resets it.
    /// IBM PC AT Technical Reference, Keyboard, Keyboard Buffer specifies 16
    /// bytes. A backlog exceeding the remaining space is discarded; the reset
    /// chord waits until all six bytes fit.
    /// Returns only accepted bytes, for replay at the current virtual time.
    pub fn inject_at_boundary(&mut self, assembled: &mut AssembledMachine) -> Vec<u8> {
        let mut delivered = Vec::new();
        self.typed_ranges.clear();
        let Some(&index) = assembled.device_index.get(&DeviceId::Keyboard) else {
            return delivered;
        };
        while let Some(reset) = self
            .events
            .iter()
            .position(|input| matches!(input, HostInput::CtrlAltDel))
        {
            let Some(leds) = assembled.keyboard_leds() else {
                return delivered;
            };
            let capacity = assembled
                .machine
                .bus_mut()
                .device_mut(index)
                .and_then(|device| device.as_any_mut())
                .and_then(|device| device.downcast_mut::<devices::I8042>())
                .map_or(0, |keyboard| keyboard.typed_capacity_remaining());
            if capacity < CTRL_ALT_DEL.len() {
                return delivered;
            }
            // Keep short modifier sequences balanced, but leave room for every
            // reset-chord release in the keyboard's 16-byte typed buffer.
            for _ in 0..=reset {
                if let Some(event) = self.events.pop_front() {
                    self.encode(event, leds & 2 != 0);
                }
            }
            if self.pending.len() > capacity {
                self.pending.clear();
                self.pending.extend(CTRL_ALT_DEL);
            }
            let bytes: Vec<_> = self.pending.drain(..).collect();
            assembled.type_scancodes(&bytes);
            self.typed_ranges
                .push(delivered.len()..delivered.len() + bytes.len());
            delivered.extend(bytes);
        }
        loop {
            if self.pending.is_empty() {
                if self.events.is_empty() {
                    break;
                }
                let Some(leds) = keyboard_leds_if_ready(&mut assembled.machine, index) else {
                    break;
                };
                if let Some(event) = self.events.pop_front() {
                    self.encode(event, leds & 2 != 0);
                }
            }
            let Some(&byte) = self.pending.front() else {
                break;
            };
            if !try_inject_scancode(&mut assembled.machine, index, byte) {
                break;
            }
            self.pending.pop_front();
            delivered.push(byte);
        }
        delivered
    }

    /// Ranges delivered as keyboard-owned sequences at the latest boundary.
    /// Replay must preserve this ownership while the interface is inhibited.
    pub fn typed_ranges(&self) -> &[std::ops::Range<usize>] {
        &self.typed_ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unemu_core_api::KeyState::{Pressed, Released};

    /// A usage the keyboard page leaves unassigned, as a media key arrives
    /// from a host that maps it to no keyboard usage.
    const UNASSIGNED: Key = Key(0xFFFF);

    #[test]
    fn reset_chord_waits_until_the_keyboard_has_room_for_all_six_bytes() {
        let mut assembled = assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        assembled
            .machine
            .bus_mut()
            .port_write(0x64, 1, 0xAD)
            .unwrap();
        assert!(assembled.type_scancodes(&[0x1E; 11]));
        let mut input = HostKeyboard::default();
        input.ctrl_alt_del();
        assert!(input.inject_at_boundary(&mut assembled).is_empty());
        assert_eq!(input.pending_events(), 1);
        assembled
            .machine
            .bus_mut()
            .port_write(0x60, 1, 0xFF)
            .unwrap();
        assert_eq!(input.inject_at_boundary(&mut assembled), CTRL_ALT_DEL);
        assert_eq!(input.pending_events(), 0);
    }

    #[test]
    fn reset_chord_accounts_for_bytes_already_owned_by_the_keyboard() {
        let mut assembled = assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        assembled
            .machine
            .bus_mut()
            .port_write(0x64, 1, 0xAD)
            .unwrap();
        assert!(assembled.type_scancodes(&[0x1E; 10]));
        let mut input = HostKeyboard::default();
        for _ in 0..3 {
            input.key_event(Key::B, Pressed);
            input.key_event(Key::B, Released);
        }
        input.ctrl_alt_del();
        assert_eq!(input.inject_at_boundary(&mut assembled), CTRL_ALT_DEL);
        assert_eq!(input.pending_events(), 0);
    }

    #[test]
    fn reset_chord_drops_a_backlog_that_exceeds_the_keyboard_buffer() {
        let mut assembled = assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        assembled
            .machine
            .bus_mut()
            .port_write(0x64, 1, 0xAD)
            .unwrap();
        let mut input = HostKeyboard::default();
        for _ in 0..20 {
            input.key_event(Key::A, Pressed);
            input.key_event(Key::A, Released);
        }
        input.ctrl_alt_del();
        assert_eq!(input.inject_at_boundary(&mut assembled), CTRL_ALT_DEL);
        assert_eq!(input.pending_events(), 0);
    }

    fn take_codes(input: &mut HostKeyboard, leds: u8) -> Vec<u8> {
        let mut codes: Vec<_> = input.pending.drain(..).collect();
        while let Some(event) = input.events.pop_front() {
            input.encode(event, leds & 2 != 0);
            codes.extend(input.pending.drain(..));
        }
        codes
    }

    fn set_guest_leds(assembled: &mut AssembledMachine, leds: u8) {
        for byte in [0xED, u32::from(leds)] {
            assert_eq!(
                assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
                0
            );
            assembled
                .machine
                .bus_mut()
                .port_write(0x60, 1, byte)
                .unwrap();
            assert_eq!(
                assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
                0
            );
            let cap = assembled.machine.virtual_time() + 100_000;
            assembled.machine.run_bounded(Some(cap)).unwrap();
            assert_eq!(
                assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
                1
            );
            assert_eq!(
                assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
                0xFA
            );
        }
        let cap = assembled.machine.virtual_time() + 100_000;
        assembled.machine.run_bounded(Some(cap)).unwrap();
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
            0
        );
    }

    #[test]
    fn reset_chord_is_owned_by_keyboard_even_while_interface_is_inhibited() {
        for (inhibited, queued_modifiers) in [(false, false), (true, false), (true, true)] {
            let mut assembled =
                assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
            let mut input = HostKeyboard::default();
            if inhibited {
                assembled
                    .machine
                    .bus_mut()
                    .port_write(0x64, 1, 0xAD)
                    .unwrap();
            }
            let mut expected = Vec::new();
            if queued_modifiers {
                input.key_event(Key::CONTROL_LEFT, Pressed);
                input.key_event(Key::ALT_LEFT, Pressed);
                expected.extend([0x1D, 0x38, 0xB8, 0x9D]);
            }
            input.ctrl_alt_del();
            expected.extend([0x1D, 0x38, 0x53, 0xD3, 0xB8, 0x9D]);
            assert_eq!(input.inject_at_boundary(&mut assembled), expected);
            assert_eq!(input.pending_events(), 0);
            if !inhibited {
                for (time, byte) in [(0, 0x1D), (100_000, 0x38), (200_000, 0x53)] {
                    if time != 0 {
                        assembled.machine.run_bounded(Some(time)).unwrap();
                    }
                    if byte == 0x53 {
                        // INT 9 inhibits the interface before consuming Delete and jumping to POST.
                        assembled
                            .machine
                            .bus_mut()
                            .port_write(0x64, 1, 0xAD)
                            .unwrap();
                    }
                    assert_eq!(
                        assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
                        byte
                    );
                }
            } else {
                assert_eq!(
                    assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
                    0
                );
            }
            assembled
                .machine
                .bus_mut()
                .port_write(0x60, 1, 0xFF)
                .unwrap();
            let start = assembled.machine.virtual_time();
            for (elapsed, reply) in [
                (100_000, Some(0xFA)),
                (500_000, Some(0xAA)),
                (600_000, None),
            ] {
                assert!(input.inject_at_boundary(&mut assembled).is_empty());
                assembled
                    .machine
                    .run_bounded(Some(start + elapsed))
                    .unwrap();
                assert_eq!(
                    assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
                    u32::from(reply.is_some())
                );
                if let Some(byte) = reply {
                    assert_eq!(
                        assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
                        byte
                    );
                }
            }
            assert!(input.held.is_empty());
            assert_eq!(input.pending_events(), 0);
        }
    }

    #[test]
    fn reset_chord_is_balanced_independently_of_guest_locks() {
        for leds in [0, 2, 7] {
            let mut input = HostKeyboard::default();
            input.key_event(Key::CONTROL_LEFT, Pressed);
            input.key_event(Key::ALT_RIGHT, Pressed);
            input.ctrl_alt_del();
            assert_eq!(
                take_codes(&mut input, leds),
                [0x1D, 0xE0, 0x38, 0xE0, 0xB8, 0x9D, 0x1D, 0x38, 0x53, 0xD3, 0xB8, 0x9D,]
            );
            input.release_all();
            assert!(take_codes(&mut input, leds).is_empty());
        }
    }

    #[test]
    fn plain_keys_have_exact_make_and_break_codes() {
        for (key, make) in [
            (Key::ESCAPE, 0x01),
            (Key::DIGIT_1, 0x02),
            (Key::DIGIT_2, 0x03),
            (Key::DIGIT_3, 0x04),
            (Key::DIGIT_4, 0x05),
            (Key::DIGIT_5, 0x06),
            (Key::DIGIT_6, 0x07),
            (Key::DIGIT_7, 0x08),
            (Key::DIGIT_8, 0x09),
            (Key::DIGIT_9, 0x0A),
            (Key::DIGIT_0, 0x0B),
            (Key::MINUS, 0x0C),
            (Key::EQUAL, 0x0D),
            (Key::BACKSPACE, 0x0E),
            (Key::TAB, 0x0F),
            (Key::Q, 0x10),
            (Key::W, 0x11),
            (Key::E, 0x12),
            (Key::R, 0x13),
            (Key::T, 0x14),
            (Key::Y, 0x15),
            (Key::U, 0x16),
            (Key::I, 0x17),
            (Key::O, 0x18),
            (Key::P, 0x19),
            (Key::BRACKET_LEFT, 0x1A),
            (Key::BRACKET_RIGHT, 0x1B),
            (Key::ENTER, 0x1C),
            (Key::CONTROL_LEFT, 0x1D),
            (Key::A, 0x1E),
            (Key::S, 0x1F),
            (Key::D, 0x20),
            (Key::F, 0x21),
            (Key::G, 0x22),
            (Key::H, 0x23),
            (Key::J, 0x24),
            (Key::K, 0x25),
            (Key::L, 0x26),
            (Key::SEMICOLON, 0x27),
            (Key::QUOTE, 0x28),
            (Key::BACKQUOTE, 0x29),
            (Key::SHIFT_LEFT, 0x2A),
            (Key::BACKSLASH, 0x2B),
            (Key::Z, 0x2C),
            (Key::X, 0x2D),
            (Key::C, 0x2E),
            (Key::V, 0x2F),
            (Key::B, 0x30),
            (Key::N, 0x31),
            (Key::M, 0x32),
            (Key::COMMA, 0x33),
            (Key::PERIOD, 0x34),
            (Key::SLASH, 0x35),
            (Key::SHIFT_RIGHT, 0x36),
            (Key::NUMPAD_MULTIPLY, 0x37),
            (Key::ALT_LEFT, 0x38),
            (Key::SPACE, 0x39),
            (Key::CAPS_LOCK, 0x3A),
            (Key::F1, 0x3B),
            (Key::F2, 0x3C),
            (Key::F3, 0x3D),
            (Key::F4, 0x3E),
            (Key::F5, 0x3F),
            (Key::F6, 0x40),
            (Key::F7, 0x41),
            (Key::F8, 0x42),
            (Key::F9, 0x43),
            (Key::F10, 0x44),
            (Key::NUM_LOCK, 0x45),
            (Key::SCROLL_LOCK, 0x46),
            (Key::NUMPAD_7, 0x47),
            (Key::NUMPAD_8, 0x48),
            (Key::NUMPAD_9, 0x49),
            (Key::NUMPAD_SUBTRACT, 0x4A),
            (Key::NUMPAD_4, 0x4B),
            (Key::NUMPAD_5, 0x4C),
            (Key::NUMPAD_6, 0x4D),
            (Key::NUMPAD_ADD, 0x4E),
            (Key::NUMPAD_1, 0x4F),
            (Key::NUMPAD_2, 0x50),
            (Key::NUMPAD_3, 0x51),
            (Key::NUMPAD_0, 0x52),
            (Key::NUMPAD_DECIMAL, 0x53),
            (Key::NON_US_BACKSLASH, 0x56),
            (Key::F11, 0x57),
            (Key::F12, 0x58),
        ] {
            assert_eq!(set1_scancodes(key, Pressed), Some(vec![make]), "{key:?}");
            assert_eq!(
                set1_scancodes(key, Released),
                Some(vec![make | 0x80]),
                "{key:?}"
            );
        }
    }

    #[test]
    fn every_extended_key_prefixes_make_and_break() {
        for (key, make) in [
            (Key::NUMPAD_ENTER, 0x1C),
            (Key::CONTROL_RIGHT, 0x1D),
            (Key::NUMPAD_DIVIDE, 0x35),
            (Key::ALT_RIGHT, 0x38),
            (Key::HOME, 0x47),
            (Key::ARROW_UP, 0x48),
            (Key::PAGE_UP, 0x49),
            (Key::ARROW_LEFT, 0x4B),
            (Key::ARROW_RIGHT, 0x4D),
            (Key::END, 0x4F),
            (Key::ARROW_DOWN, 0x50),
            (Key::PAGE_DOWN, 0x51),
            (Key::INSERT, 0x52),
            (Key::DELETE, 0x53),
        ] {
            assert_eq!(
                set1_scancodes(key, Pressed),
                Some(vec![0xE0, make]),
                "{key:?}"
            );
            assert_eq!(
                set1_scancodes(key, Released),
                Some(vec![0xE0, make | 0x80]),
                "{key:?}"
            );
        }
    }

    #[test]
    fn shift_and_typematic_preserve_physical_event_order() {
        let mut input = HostKeyboard::default();
        for (key, state) in [
            (Key::SHIFT_LEFT, Pressed),
            (Key::A, Pressed),
            (Key::A, Pressed),
            (Key::A, Released),
            (Key::SHIFT_LEFT, Released),
        ] {
            input.key_event(key, state);
        }
        assert_eq!(take_codes(&mut input, 0), [0x2A, 0x1E, 0x1E, 0x9E, 0xAA]);
    }

    #[test]
    fn unsupported_and_special_keys_are_ignored() {
        let mut input = HostKeyboard::default();
        for key in [
            Key::PAUSE,
            Key::PRINT_SCREEN,
            Key::GUI_LEFT,
            UNASSIGNED,
            Key::LANG_1,
            Key::F13,
        ] {
            for state in [Pressed, Released] {
                assert_eq!(set1_scancodes(key, state), None);
                input.key_event(key, state);
            }
        }
        assert_eq!(input.pending_events(), 0);
    }

    #[test]
    fn navigation_wrappers_cover_every_numlock_and_shift_case() {
        for (key, make) in [
            (Key::INSERT, 0x52),
            (Key::DELETE, 0x53),
            (Key::HOME, 0x47),
            (Key::END, 0x4F),
            (Key::PAGE_UP, 0x49),
            (Key::PAGE_DOWN, 0x51),
            (Key::ARROW_UP, 0x48),
            (Key::ARROW_DOWN, 0x50),
            (Key::ARROW_LEFT, 0x4B),
            (Key::ARROW_RIGHT, 0x4D),
        ] {
            for (num_lock, shifts, before, after) in [
                (false, vec![], vec![], vec![]),
                (
                    false,
                    vec![Key::SHIFT_LEFT],
                    vec![0xE0, 0xAA],
                    vec![0xE0, 0x2A],
                ),
                (
                    false,
                    vec![Key::SHIFT_RIGHT],
                    vec![0xE0, 0xB6],
                    vec![0xE0, 0x36],
                ),
                (
                    false,
                    vec![Key::SHIFT_LEFT, Key::SHIFT_RIGHT],
                    vec![0xE0, 0xAA, 0xE0, 0xB6],
                    vec![0xE0, 0x2A, 0xE0, 0x36],
                ),
                (true, vec![], vec![0xE0, 0x2A], vec![0xE0, 0xAA]),
                (true, vec![Key::SHIFT_LEFT], vec![], vec![]),
                (true, vec![Key::SHIFT_RIGHT], vec![], vec![]),
                (
                    true,
                    vec![Key::SHIFT_LEFT, Key::SHIFT_RIGHT],
                    vec![],
                    vec![],
                ),
            ] {
                let mut input = HostKeyboard::default();
                if num_lock {
                    input.key_event(Key::NUM_LOCK, Pressed);
                    input.key_event(Key::NUM_LOCK, Released);
                }
                for &shift in &shifts {
                    input.key_event(shift, Pressed);
                }
                input.events.clear();
                input.pending.clear();
                input.key_event(key, Pressed);
                input.key_event(key, Pressed);
                input.key_event(key, Released);
                let mut expected = before.clone();
                expected.extend([0xE0, make]);
                expected.extend([0xE0, make, 0xE0, make | 0x80]);
                expected.extend(after);
                assert_eq!(
                    take_codes(&mut input, if num_lock { 2 } else { 0 }),
                    expected,
                    "{key:?}, num={num_lock}, shifts={shifts:?}"
                );
            }
        }
    }

    #[test]
    fn divide_wrappers_follow_both_shifts_independently_of_numlock() {
        for num_lock in [false, true] {
            for (shifts, before, after) in [
                (vec![], vec![], vec![]),
                (vec![Key::SHIFT_LEFT], vec![0xE0, 0xAA], vec![0xE0, 0x2A]),
                (vec![Key::SHIFT_RIGHT], vec![0xE0, 0xB6], vec![0xE0, 0x36]),
                (
                    vec![Key::SHIFT_LEFT, Key::SHIFT_RIGHT],
                    vec![0xE0, 0xAA, 0xE0, 0xB6],
                    vec![0xE0, 0x2A, 0xE0, 0x36],
                ),
            ] {
                let mut input = HostKeyboard::default();
                if num_lock {
                    input.key_event(Key::NUM_LOCK, Pressed);
                    input.key_event(Key::NUM_LOCK, Released);
                }
                for &shift in &shifts {
                    input.key_event(shift, Pressed);
                }
                input.events.clear();
                input.pending.clear();
                input.key_event(Key::NUMPAD_DIVIDE, Pressed);
                input.key_event(Key::NUMPAD_DIVIDE, Released);
                let mut expected = before;
                expected.extend([0xE0, 0x35, 0xE0, 0xB5]);
                expected.extend(after);
                assert_eq!(
                    take_codes(&mut input, if num_lock { 2 } else { 0 }),
                    expected
                );
            }
        }
    }

    #[test]
    fn guest_leds_select_wrappers_at_the_boundary_instead_of_numlock_makes() {
        for leds in [0, 1, 2, 4, 7] {
            let mut assembled =
                assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
            let mut input = HostKeyboard::default();
            input.key_event(Key::CONTROL_LEFT, Pressed);
            input.key_event(Key::NUM_LOCK, Pressed);
            input.key_event(Key::NUM_LOCK, Released);
            input.key_event(Key::CONTROL_LEFT, Released);
            take_codes(&mut input, 0);
            input.key_event(Key::ARROW_UP, Pressed);
            // The guest updates indicators after the host event was enqueued.
            set_guest_leds(&mut assembled, leds);
            input.inject_at_boundary(&mut assembled);
            assert_eq!(
                assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
                0xE0
            );
            let expected = if leds & 2 != 0 {
                vec![0x2A, 0xE0, 0x48]
            } else {
                vec![0x48]
            };
            assert_eq!(
                input.pending.into_iter().collect::<Vec<_>>(),
                expected,
                "LEDs {leds}"
            );
        }
    }

    #[test]
    fn queued_numlock_then_up_reads_leds_separately_for_each_key() {
        let mut assembled = assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        let mut input = HostKeyboard::default();
        input.key_event(Key::NUM_LOCK, Pressed);
        input.key_event(Key::ARROW_UP, Pressed);
        input.inject_at_boundary(&mut assembled);
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
            0x45
        );
        assembled.machine.run_bounded(Some(100_000)).unwrap();
        set_guest_leds(&mut assembled, 2);
        let mut delivered = Vec::new();
        for _ in 0..4 {
            if input.pending_events() == 0 {
                break;
            }
            input.inject_at_boundary(&mut assembled);
            assert_eq!(
                assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
                1
            );
            delivered.push(assembled.machine.bus_mut().port_read(0x60, 1).unwrap() as u8);
            let cap = assembled.machine.virtual_time() + 100_000;
            assembled.machine.run_bounded(Some(cap)).unwrap();
        }
        assert_eq!(delivered, [0xE0, 0x2A, 0xE0, 0x48]);
        assert_eq!(input.pending_events(), 0);
    }

    #[test]
    fn led_exchange_and_reset_are_observed_before_encoding_the_next_key() {
        let mut assembled = assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        let mut input = HostKeyboard::default();
        input.key_event(Key::ARROW_UP, Pressed);
        assembled
            .machine
            .bus_mut()
            .port_write(0x60, 1, 0xED)
            .unwrap();
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
            0
        );
        assembled.machine.run_bounded(Some(100_000)).unwrap();
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
            1
        );
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
            0xFA
        );
        input.inject_at_boundary(&mut assembled);
        assert!(
            input.pending.is_empty(),
            "do not encode while the LED parameter is outstanding"
        );
        assert_eq!(input.events.len(), 1);
        assembled.machine.bus_mut().port_write(0x60, 1, 2).unwrap();
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
            0
        );
        assembled.machine.run_bounded(Some(200_000)).unwrap();
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x64, 1).unwrap() & 1,
            1
        );
        assert_eq!(
            assembled.machine.bus_mut().port_read(0x60, 1).unwrap(),
            0xFA
        );
        assembled.machine.run_bounded(Some(300_000)).unwrap();
        input.inject_at_boundary(&mut assembled);
        assert_eq!(
            input.pending.iter().copied().collect::<Vec<_>>(),
            [0x2A, 0xE0, 0x48]
        );
        input.pending.clear();
        input.key_event(Key::ARROW_UP, Released);
        input.events.clear();
        input.key_event(Key::ARROW_UP, Pressed);
        let index = assembled.device_index[&DeviceId::Keyboard];
        assembled
            .machine
            .bus_mut()
            .device_mut(index)
            .unwrap()
            .reset();
        input.inject_at_boundary(&mut assembled);
        assert_eq!(input.pending.into_iter().collect::<Vec<_>>(), [0x48]);
    }

    #[test]
    fn wrapped_typematic_sends_only_the_base_make_after_the_first_press() {
        for key in [Key::ARROW_UP, Key::NUMPAD_DIVIDE] {
            for shift in [Key::SHIFT_LEFT, Key::SHIFT_RIGHT] {
                let mut input = HostKeyboard::default();
                input.key_event(shift, Pressed);
                take_codes(&mut input, 0);
                input.key_event(key, Pressed);
                let first = take_codes(&mut input, 0);
                assert_eq!(first.len(), 4);
                for _ in 0..3 {
                    input.key_event(key, Pressed);
                    assert_eq!(
                        take_codes(&mut input, 0),
                        set1_scancodes(key, Pressed).unwrap()
                    );
                }
                input.key_event(key, Released);
                assert_eq!(take_codes(&mut input, 0).len(), 4);
            }
        }
        let mut input = HostKeyboard::default();
        input.key_event(Key::ARROW_UP, Pressed);
        assert_eq!(take_codes(&mut input, 2), [0xE0, 0x2A, 0xE0, 0x48]);
        input.key_event(Key::ARROW_UP, Pressed);
        assert_eq!(take_codes(&mut input, 2), [0xE0, 0x48]);
        input.key_event(Key::ARROW_UP, Released);
        assert_eq!(take_codes(&mut input, 2), [0xE0, 0xC8, 0xE0, 0xAA]);
    }

    #[test]
    fn focus_loss_releases_wrapped_keys_before_physical_shifts_and_absorbs_duplicates() {
        for (shifts, wrappers, shift_breaks) in [
            (vec![], vec![], vec![]),
            (vec![Key::SHIFT_LEFT], vec![0xE0, 0x2A], vec![0xAA]),
            (vec![Key::SHIFT_RIGHT], vec![0xE0, 0x36], vec![0xB6]),
            (
                vec![Key::SHIFT_LEFT, Key::SHIFT_RIGHT],
                vec![0xE0, 0x2A, 0xE0, 0x36],
                vec![0xAA, 0xB6],
            ),
        ] {
            for key in [
                Key::A,
                Key::CONTROL_LEFT,
                Key::CONTROL_RIGHT,
                Key::ALT_LEFT,
                Key::ALT_RIGHT,
                Key::ARROW_UP,
                Key::NUMPAD_DIVIDE,
            ] {
                let mut input = HostKeyboard::default();
                for &shift in &shifts {
                    input.key_event(shift, Pressed);
                }
                input.key_event(key, Pressed);
                input.events.clear();
                input.pending.clear();
                input.release_all();
                let mut expected = set1_scancodes(key, Released).unwrap();
                if matches!(key, Key::ARROW_UP | Key::NUMPAD_DIVIDE) {
                    expected.extend(&wrappers);
                }
                expected.extend(&shift_breaks);
                assert_eq!(take_codes(&mut input, 0), expected, "{key:?} {shifts:?}");
                assert!(input.held.is_empty());
                input.release_all();
                input.key_event(key, Released);
                for &shift in &shifts {
                    input.key_event(shift, Released);
                }
                assert!(take_codes(&mut input, 0).is_empty());
            }
        }
        let mut input = HostKeyboard::default();
        input.key_event(Key::NUM_LOCK, Pressed);
        input.key_event(Key::NUM_LOCK, Released);
        input.key_event(Key::ARROW_UP, Pressed);
        input.events.clear();
        input.pending.clear();
        input.release_all();
        assert_eq!(take_codes(&mut input, 2), [0xE0, 0xC8, 0xE0, 0xAA]);
    }

    #[test]
    fn unheld_releases_emit_nothing_before_or_after_an_ordinary_key_pair() {
        let mut input = HostKeyboard::default();
        for key in [
            Key::A,
            Key::SHIFT_LEFT,
            Key::CONTROL_RIGHT,
            Key::ALT_RIGHT,
            Key::NUM_LOCK,
            Key::ARROW_UP,
        ] {
            input.key_event(key, Released);
        }
        assert_eq!(input.pending_events(), 0);
        input.key_event(Key::A, Pressed);
        input.key_event(Key::A, Released);
        input.key_event(Key::A, Released);
        assert_eq!(take_codes(&mut input, 0), [0x1E, 0x9E]);
    }

    #[test]
    fn release_order_lists_every_encoded_key_once() {
        for (index, key) in RELEASE_ORDER.iter().enumerate() {
            assert!(set1_scancodes(*key, Pressed).is_some(), "{key:?}");
            assert!(!RELEASE_ORDER[..index].contains(key), "{key:?}");
        }
        let encoded = (0..=u16::MAX)
            .map(Key)
            .filter(|key| set1_scancodes(*key, Pressed).is_some())
            .count();
        assert_eq!(encoded, RELEASE_ORDER.len());
    }
}
