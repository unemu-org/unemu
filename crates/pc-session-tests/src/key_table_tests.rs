// SPDX-License-Identifier: AGPL-3.0-or-later
//! The window's physical keys reach the guest as the scan codes the desktop
//! always sent: the session's mapping to keyboard usages composed with the PC
//! core's set-1 encoder gives the table the window once encoded winit key
//! codes with directly, key for key, and a focus loss releases held keys in
//! the order it always did.
use winit::event::ElementState::{Pressed, Released};
use winit::keyboard::KeyCode;
use KeyCode::*;

/// The window's own set-1 table before the contract: winit key code,
/// make code and whether it carries the E0 prefix, in winit's order.
const BEFORE: [(KeyCode, u8, bool); 100] = [
    (Backquote, 0x29, false),
    (Backslash, 0x2B, false),
    (BracketLeft, 0x1A, false),
    (BracketRight, 0x1B, false),
    (Comma, 0x33, false),
    (Digit0, 0x0B, false),
    (Digit1, 0x02, false),
    (Digit2, 0x03, false),
    (Digit3, 0x04, false),
    (Digit4, 0x05, false),
    (Digit5, 0x06, false),
    (Digit6, 0x07, false),
    (Digit7, 0x08, false),
    (Digit8, 0x09, false),
    (Digit9, 0x0A, false),
    (Equal, 0x0D, false),
    (IntlBackslash, 0x56, false),
    (KeyA, 0x1E, false),
    (KeyB, 0x30, false),
    (KeyC, 0x2E, false),
    (KeyD, 0x20, false),
    (KeyE, 0x12, false),
    (KeyF, 0x21, false),
    (KeyG, 0x22, false),
    (KeyH, 0x23, false),
    (KeyI, 0x17, false),
    (KeyJ, 0x24, false),
    (KeyK, 0x25, false),
    (KeyL, 0x26, false),
    (KeyM, 0x32, false),
    (KeyN, 0x31, false),
    (KeyO, 0x18, false),
    (KeyP, 0x19, false),
    (KeyQ, 0x10, false),
    (KeyR, 0x13, false),
    (KeyS, 0x1F, false),
    (KeyT, 0x14, false),
    (KeyU, 0x16, false),
    (KeyV, 0x2F, false),
    (KeyW, 0x11, false),
    (KeyX, 0x2D, false),
    (KeyY, 0x15, false),
    (KeyZ, 0x2C, false),
    (Minus, 0x0C, false),
    (Period, 0x34, false),
    (Quote, 0x28, false),
    (Semicolon, 0x27, false),
    (Slash, 0x35, false),
    (AltLeft, 0x38, false),
    (AltRight, 0x38, true),
    (Backspace, 0x0E, false),
    (CapsLock, 0x3A, false),
    (ControlLeft, 0x1D, false),
    (ControlRight, 0x1D, true),
    (Enter, 0x1C, false),
    (ShiftLeft, 0x2A, false),
    (ShiftRight, 0x36, false),
    (Space, 0x39, false),
    (Tab, 0x0F, false),
    (Delete, 0x53, true),
    (End, 0x4F, true),
    (Home, 0x47, true),
    (Insert, 0x52, true),
    (PageDown, 0x51, true),
    (PageUp, 0x49, true),
    (ArrowDown, 0x50, true),
    (ArrowLeft, 0x4B, true),
    (ArrowRight, 0x4D, true),
    (ArrowUp, 0x48, true),
    (NumLock, 0x45, false),
    (Numpad0, 0x52, false),
    (Numpad1, 0x4F, false),
    (Numpad2, 0x50, false),
    (Numpad3, 0x51, false),
    (Numpad4, 0x4B, false),
    (Numpad5, 0x4C, false),
    (Numpad6, 0x4D, false),
    (Numpad7, 0x47, false),
    (Numpad8, 0x48, false),
    (Numpad9, 0x49, false),
    (NumpadAdd, 0x4E, false),
    (NumpadDecimal, 0x53, false),
    (NumpadDivide, 0x35, true),
    (NumpadEnter, 0x1C, true),
    (NumpadMultiply, 0x37, false),
    (NumpadSubtract, 0x4A, false),
    (Escape, 0x01, false),
    (ScrollLock, 0x46, false),
    (F1, 0x3B, false),
    (F2, 0x3C, false),
    (F3, 0x3D, false),
    (F4, 0x3E, false),
    (F5, 0x3F, false),
    (F6, 0x40, false),
    (F7, 0x41, false),
    (F8, 0x42, false),
    (F9, 0x43, false),
    (F10, 0x44, false),
    (F11, 0x57, false),
    (F12, 0x58, false),
];

fn codes(make: u8, extended: bool, released: bool) -> Vec<u8> {
    let code = make | if released { 0x80 } else { 0 };
    if extended {
        vec![0xE0, code]
    } else {
        vec![code]
    }
}

#[test]
fn host_keys_keep_the_set_1_codes_of_the_window_table() {
    for (key, make, extended) in BEFORE {
        assert_eq!(
            crate::set1_scancodes(key, Pressed),
            Some(codes(make, extended, false)),
            "{key:?}"
        );
        assert_eq!(
            crate::set1_scancodes(key, Released),
            Some(codes(make, extended, true)),
            "{key:?}"
        );
    }
    for key in [
        Pause,
        PrintScreen,
        SuperLeft,
        ContextMenu,
        Lang1,
        F13,
        MediaPlayPause,
    ] {
        assert_eq!(crate::set1_scancodes(key, Pressed), None, "{key:?}");
    }
}

#[test]
fn focus_loss_releases_held_host_keys_in_the_window_order() {
    let mut keyboard = unemu_pc_core::HostKeyboard::default();
    let usage = |key| machine_view::keys::key_usage(key).unwrap();
    for (key, _, _) in BEFORE.iter().rev() {
        keyboard.key_event(usage(*key), unemu_core_api::KeyState::Pressed);
    }
    let config = assembly::at_keyboard(vec![0xEB, 0xFE]);
    let mut assembled = machine_file::assemble(&config).unwrap();
    // Drop the makes, then release everything at once.
    let mut sink = Vec::new();
    while keyboard.pending_events() != 0 {
        sink.extend(keyboard.inject_at_boundary(&mut assembled));
        assembled = machine_file::assemble(&config).unwrap();
    }
    keyboard.release_all();
    let mut released = Vec::new();
    while keyboard.pending_events() != 0 {
        released.extend(keyboard.inject_at_boundary(&mut assembled));
        assembled = machine_file::assemble(&config).unwrap();
    }
    // Held Shift wraps the navigation keys' breaks with artificial
    // Shift makes; the window sorted held keys by winit key code and
    // released the physical Shifts last.
    let mut expected = Vec::new();
    for (key, make, extended) in BEFORE {
        if matches!(key, ShiftLeft | ShiftRight) {
            continue;
        }
        expected.extend(codes(make, extended, true));
        if matches!(
            key,
            Insert
                | Delete
                | Home
                | End
                | PageUp
                | PageDown
                | ArrowUp
                | ArrowDown
                | ArrowLeft
                | ArrowRight
                | NumpadDivide
        ) {
            expected.extend([0xE0, 0x2A, 0xE0, 0x36]);
        }
    }
    expected.extend([0xAA, 0xB6]);
    assert_eq!(released, expected);
}
