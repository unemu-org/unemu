// SPDX-License-Identifier: AGPL-3.0-or-later
//! Physical host keys to IBM PC scan code set 1 (design/spec.md §7.5).
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
use winit::{event::ElementState, keyboard::KeyCode};

/// Encode the base physical make/break; unsupported keys produce no bytes.
/// [`HostKeyboard`] adds the state-dependent artificial Shift wrappers.
/// A repeated winit press emits the same make sequence, without an intervening
/// break, implementing host-driven typematic. Guest typematic-rate commands do
/// not change the host repeat rate. E0 precedes both make and break codes.
pub fn set1_scancodes(key: KeyCode, state: ElementState) -> Option<Vec<u8>> {
    use KeyCode::*;
    let (code, extended) = match key {
        Escape => (0x01, false),
        Digit1 => (0x02, false),
        Digit2 => (0x03, false),
        Digit3 => (0x04, false),
        Digit4 => (0x05, false),
        Digit5 => (0x06, false),
        Digit6 => (0x07, false),
        Digit7 => (0x08, false),
        Digit8 => (0x09, false),
        Digit9 => (0x0A, false),
        Digit0 => (0x0B, false),
        Minus => (0x0C, false),
        Equal => (0x0D, false),
        Backspace => (0x0E, false),
        Tab => (0x0F, false),
        KeyQ => (0x10, false),
        KeyW => (0x11, false),
        KeyE => (0x12, false),
        KeyR => (0x13, false),
        KeyT => (0x14, false),
        KeyY => (0x15, false),
        KeyU => (0x16, false),
        KeyI => (0x17, false),
        KeyO => (0x18, false),
        KeyP => (0x19, false),
        BracketLeft => (0x1A, false),
        BracketRight => (0x1B, false),
        Enter => (0x1C, false),
        ControlLeft => (0x1D, false),
        KeyA => (0x1E, false),
        KeyS => (0x1F, false),
        KeyD => (0x20, false),
        KeyF => (0x21, false),
        KeyG => (0x22, false),
        KeyH => (0x23, false),
        KeyJ => (0x24, false),
        KeyK => (0x25, false),
        KeyL => (0x26, false),
        Semicolon => (0x27, false),
        Quote => (0x28, false),
        Backquote => (0x29, false),
        ShiftLeft => (0x2A, false),
        Backslash => (0x2B, false),
        KeyZ => (0x2C, false),
        KeyX => (0x2D, false),
        KeyC => (0x2E, false),
        KeyV => (0x2F, false),
        KeyB => (0x30, false),
        KeyN => (0x31, false),
        KeyM => (0x32, false),
        Comma => (0x33, false),
        Period => (0x34, false),
        Slash => (0x35, false),
        ShiftRight => (0x36, false),
        NumpadMultiply => (0x37, false),
        AltLeft => (0x38, false),
        Space => (0x39, false),
        CapsLock => (0x3A, false),
        F1 => (0x3B, false),
        F2 => (0x3C, false),
        F3 => (0x3D, false),
        F4 => (0x3E, false),
        F5 => (0x3F, false),
        F6 => (0x40, false),
        F7 => (0x41, false),
        F8 => (0x42, false),
        F9 => (0x43, false),
        F10 => (0x44, false),
        NumLock => (0x45, false),
        ScrollLock => (0x46, false),
        Numpad7 => (0x47, false),
        Numpad8 => (0x48, false),
        Numpad9 => (0x49, false),
        NumpadSubtract => (0x4A, false),
        Numpad4 => (0x4B, false),
        Numpad5 => (0x4C, false),
        Numpad6 => (0x4D, false),
        NumpadAdd => (0x4E, false),
        Numpad1 => (0x4F, false),
        Numpad2 => (0x50, false),
        Numpad3 => (0x51, false),
        Numpad0 => (0x52, false),
        NumpadDecimal => (0x53, false),
        IntlBackslash => (0x56, false),
        F11 => (0x57, false),
        F12 => (0x58, false),
        NumpadEnter => (0x1C, true),
        ControlRight => (0x1D, true),
        NumpadDivide => (0x35, true),
        AltRight => (0x38, true),
        Home => (0x47, true),
        ArrowUp => (0x48, true),
        PageUp => (0x49, true),
        ArrowLeft => (0x4B, true),
        ArrowRight => (0x4D, true),
        End => (0x4F, true),
        ArrowDown => (0x50, true),
        PageDown => (0x51, true),
        Insert => (0x52, true),
        Delete => (0x53, true),
        _ => return None,
    };
    let code = code
        | if state == ElementState::Released {
            0x80
        } else {
            0
        };
    Some(if extended {
        vec![0xE0, code]
    } else {
        vec![code]
    })
}

#[derive(Clone, Copy)]
struct KeyTransition {
    key: KeyCode,
    state: ElementState,
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
    held: BTreeSet<KeyCode>,
    events: VecDeque<HostInput>,
}

impl HostKeyboard {
    /// Queue a winit physical event, including repeated makes. A release for a
    /// key not held is ignored, including synthetic releases after focus loss.
    pub fn key_event(&mut self, key: KeyCode, state: ElementState) {
        if set1_scancodes(key, state).is_none() {
            return;
        }
        if state == ElementState::Released && !self.held.remove(&key) {
            return;
        }
        let repeat = state == ElementState::Pressed && !self.held.insert(key);
        self.events.push_back(HostInput::Key(KeyTransition {
            key,
            state,
            repeat,
            left_shift: self.held.contains(&KeyCode::ShiftLeft),
            right_shift: self.held.contains(&KeyCode::ShiftRight),
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
            KeyCode::Insert
                | KeyCode::Delete
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::ArrowUp
                | KeyCode::ArrowDown
                | KeyCode::ArrowLeft
                | KeyCode::ArrowRight
        );
        let divide = key == KeyCode::NumpadDivide;
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
        if state == ElementState::Pressed {
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
    /// followed by the physical Shift breaks. Sorted order is host-independent.
    pub fn release_all(&mut self) {
        let keys: Vec<_> = self.held.iter().copied().collect();
        for &key in &keys {
            if !matches!(key, KeyCode::ShiftLeft | KeyCode::ShiftRight) {
                self.key_event(key, ElementState::Released);
            }
        }
        for key in [KeyCode::ShiftLeft, KeyCode::ShiftRight] {
            self.key_event(key, ElementState::Released);
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
    /// Returns only accepted bytes, for replay at the current virtual time.
    pub fn inject_at_boundary(&mut self, assembled: &mut AssembledMachine) -> Vec<u8> {
        let mut delivered = Vec::new();
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
            // Earlier transitions must join the chord even if interface inhibit
            // would otherwise leave their modifier makes in host staging.
            for _ in 0..=reset {
                if let Some(event) = self.events.pop_front() {
                    self.encode(event, leds & 2 != 0);
                }
            }
            let bytes: Vec<_> = self.pending.drain(..).collect();
            assembled.type_scancodes(&bytes);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use ElementState::{Pressed, Released};
    use KeyCode::*;

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
                input.key_event(ControlLeft, Pressed);
                input.key_event(AltLeft, Pressed);
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
            input.key_event(ControlLeft, Pressed);
            input.key_event(AltRight, Pressed);
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
            (Escape, 0x01),
            (Digit1, 0x02),
            (Digit2, 0x03),
            (Digit3, 0x04),
            (Digit4, 0x05),
            (Digit5, 0x06),
            (Digit6, 0x07),
            (Digit7, 0x08),
            (Digit8, 0x09),
            (Digit9, 0x0A),
            (Digit0, 0x0B),
            (Minus, 0x0C),
            (Equal, 0x0D),
            (Backspace, 0x0E),
            (Tab, 0x0F),
            (KeyQ, 0x10),
            (KeyW, 0x11),
            (KeyE, 0x12),
            (KeyR, 0x13),
            (KeyT, 0x14),
            (KeyY, 0x15),
            (KeyU, 0x16),
            (KeyI, 0x17),
            (KeyO, 0x18),
            (KeyP, 0x19),
            (BracketLeft, 0x1A),
            (BracketRight, 0x1B),
            (Enter, 0x1C),
            (ControlLeft, 0x1D),
            (KeyA, 0x1E),
            (KeyS, 0x1F),
            (KeyD, 0x20),
            (KeyF, 0x21),
            (KeyG, 0x22),
            (KeyH, 0x23),
            (KeyJ, 0x24),
            (KeyK, 0x25),
            (KeyL, 0x26),
            (Semicolon, 0x27),
            (Quote, 0x28),
            (Backquote, 0x29),
            (ShiftLeft, 0x2A),
            (Backslash, 0x2B),
            (KeyZ, 0x2C),
            (KeyX, 0x2D),
            (KeyC, 0x2E),
            (KeyV, 0x2F),
            (KeyB, 0x30),
            (KeyN, 0x31),
            (KeyM, 0x32),
            (Comma, 0x33),
            (Period, 0x34),
            (Slash, 0x35),
            (ShiftRight, 0x36),
            (NumpadMultiply, 0x37),
            (AltLeft, 0x38),
            (Space, 0x39),
            (CapsLock, 0x3A),
            (F1, 0x3B),
            (F2, 0x3C),
            (F3, 0x3D),
            (F4, 0x3E),
            (F5, 0x3F),
            (F6, 0x40),
            (F7, 0x41),
            (F8, 0x42),
            (F9, 0x43),
            (F10, 0x44),
            (NumLock, 0x45),
            (ScrollLock, 0x46),
            (Numpad7, 0x47),
            (Numpad8, 0x48),
            (Numpad9, 0x49),
            (NumpadSubtract, 0x4A),
            (Numpad4, 0x4B),
            (Numpad5, 0x4C),
            (Numpad6, 0x4D),
            (NumpadAdd, 0x4E),
            (Numpad1, 0x4F),
            (Numpad2, 0x50),
            (Numpad3, 0x51),
            (Numpad0, 0x52),
            (NumpadDecimal, 0x53),
            (IntlBackslash, 0x56),
            (F11, 0x57),
            (F12, 0x58),
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
            (NumpadEnter, 0x1C),
            (ControlRight, 0x1D),
            (NumpadDivide, 0x35),
            (AltRight, 0x38),
            (Home, 0x47),
            (ArrowUp, 0x48),
            (PageUp, 0x49),
            (ArrowLeft, 0x4B),
            (ArrowRight, 0x4D),
            (End, 0x4F),
            (ArrowDown, 0x50),
            (PageDown, 0x51),
            (Insert, 0x52),
            (Delete, 0x53),
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
            (ShiftLeft, Pressed),
            (KeyA, Pressed),
            (KeyA, Pressed),
            (KeyA, Released),
            (ShiftLeft, Released),
        ] {
            input.key_event(key, state);
        }
        assert_eq!(take_codes(&mut input, 0), [0x2A, 0x1E, 0x1E, 0x9E, 0xAA]);
    }

    #[test]
    fn unsupported_and_special_keys_are_ignored() {
        let mut input = HostKeyboard::default();
        for key in [Pause, PrintScreen, SuperLeft, MediaPlayPause, Lang1, F13] {
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
            (Insert, 0x52),
            (Delete, 0x53),
            (Home, 0x47),
            (End, 0x4F),
            (PageUp, 0x49),
            (PageDown, 0x51),
            (ArrowUp, 0x48),
            (ArrowDown, 0x50),
            (ArrowLeft, 0x4B),
            (ArrowRight, 0x4D),
        ] {
            for (num_lock, shifts, before, after) in [
                (false, vec![], vec![], vec![]),
                (false, vec![ShiftLeft], vec![0xE0, 0xAA], vec![0xE0, 0x2A]),
                (false, vec![ShiftRight], vec![0xE0, 0xB6], vec![0xE0, 0x36]),
                (
                    false,
                    vec![ShiftLeft, ShiftRight],
                    vec![0xE0, 0xAA, 0xE0, 0xB6],
                    vec![0xE0, 0x2A, 0xE0, 0x36],
                ),
                (true, vec![], vec![0xE0, 0x2A], vec![0xE0, 0xAA]),
                (true, vec![ShiftLeft], vec![], vec![]),
                (true, vec![ShiftRight], vec![], vec![]),
                (true, vec![ShiftLeft, ShiftRight], vec![], vec![]),
            ] {
                let mut input = HostKeyboard::default();
                if num_lock {
                    input.key_event(NumLock, Pressed);
                    input.key_event(NumLock, Released);
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
                (vec![ShiftLeft], vec![0xE0, 0xAA], vec![0xE0, 0x2A]),
                (vec![ShiftRight], vec![0xE0, 0xB6], vec![0xE0, 0x36]),
                (
                    vec![ShiftLeft, ShiftRight],
                    vec![0xE0, 0xAA, 0xE0, 0xB6],
                    vec![0xE0, 0x2A, 0xE0, 0x36],
                ),
            ] {
                let mut input = HostKeyboard::default();
                if num_lock {
                    input.key_event(NumLock, Pressed);
                    input.key_event(NumLock, Released);
                }
                for &shift in &shifts {
                    input.key_event(shift, Pressed);
                }
                input.events.clear();
                input.pending.clear();
                input.key_event(NumpadDivide, Pressed);
                input.key_event(NumpadDivide, Released);
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
            input.key_event(ControlLeft, Pressed);
            input.key_event(NumLock, Pressed);
            input.key_event(NumLock, Released);
            input.key_event(ControlLeft, Released);
            take_codes(&mut input, 0);
            input.key_event(ArrowUp, Pressed);
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
        input.key_event(NumLock, Pressed);
        input.key_event(ArrowUp, Pressed);
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
        input.key_event(ArrowUp, Pressed);
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
        input.key_event(ArrowUp, Released);
        input.events.clear();
        input.key_event(ArrowUp, Pressed);
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
        for key in [ArrowUp, NumpadDivide] {
            for shift in [ShiftLeft, ShiftRight] {
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
        input.key_event(ArrowUp, Pressed);
        assert_eq!(take_codes(&mut input, 2), [0xE0, 0x2A, 0xE0, 0x48]);
        input.key_event(ArrowUp, Pressed);
        assert_eq!(take_codes(&mut input, 2), [0xE0, 0x48]);
        input.key_event(ArrowUp, Released);
        assert_eq!(take_codes(&mut input, 2), [0xE0, 0xC8, 0xE0, 0xAA]);
    }

    #[test]
    fn focus_loss_releases_wrapped_keys_before_physical_shifts_and_absorbs_duplicates() {
        for (shifts, wrappers, shift_breaks) in [
            (vec![], vec![], vec![]),
            (vec![ShiftLeft], vec![0xE0, 0x2A], vec![0xAA]),
            (vec![ShiftRight], vec![0xE0, 0x36], vec![0xB6]),
            (
                vec![ShiftLeft, ShiftRight],
                vec![0xE0, 0x2A, 0xE0, 0x36],
                vec![0xAA, 0xB6],
            ),
        ] {
            for key in [
                KeyA,
                ControlLeft,
                ControlRight,
                AltLeft,
                AltRight,
                ArrowUp,
                NumpadDivide,
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
                if matches!(key, ArrowUp | NumpadDivide) {
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
        input.key_event(NumLock, Pressed);
        input.key_event(NumLock, Released);
        input.key_event(ArrowUp, Pressed);
        input.events.clear();
        input.pending.clear();
        input.release_all();
        assert_eq!(take_codes(&mut input, 2), [0xE0, 0xC8, 0xE0, 0xAA]);
    }

    #[test]
    fn unheld_releases_emit_nothing_before_or_after_an_ordinary_key_pair() {
        let mut input = HostKeyboard::default();
        for key in [KeyA, ShiftLeft, ControlRight, AltRight, NumLock, ArrowUp] {
            input.key_event(key, Released);
        }
        assert_eq!(input.pending_events(), 0);
        input.key_event(KeyA, Pressed);
        input.key_event(KeyA, Released);
        input.key_event(KeyA, Released);
        assert_eq!(take_codes(&mut input, 0), [0x1E, 0x9E]);
    }
}
