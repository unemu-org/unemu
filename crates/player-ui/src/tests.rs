// SPDX-License-Identifier: AGPL-3.0-or-later
//! The player over the test core, headless, as the application's tests draw
//! without a window: each control of "What a running machine offers" is
//! there exactly when the description offers it, the menus' commands reach
//! the machine, the screen keeps the machine's shape, the pointer aims, and
//! the Input dialog binds a key that then works the machine's pad.

use super::*;
use crate::menus::Entry;
use core_launch::{prepare, Cores, HostDirs};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use ui_kit::test_support::{painted_at, Pointer};
use unemu_core_api::{Core, Key};
use unemu_test_core::{picture, TestCore};

static CORES: [&dyn Core; 1] = [&TestCore];

/// A directory of its own for one test, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "player-ui-tests-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join("files")).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A player of one script, with the host time the test has reached. The
/// player goes before the directory its machine keeps its records in.
struct Rig {
    player: Player,
    ctx: egui::Context,
    at: Duration,
    _scratch: Scratch,
}

/// The default test machine: 60 000 ticks a second, a frame every 1 000.
fn rig(lines: &str) -> Rig {
    rig_in(Scratch::new(), lines)
}

/// The machine of `lines` in `scratch`, which may hold what an earlier
/// machine of the same script left there.
fn rig_in(scratch: Scratch, lines: &str) -> Rig {
    let path = scratch.0.join("files").join("machine.utc");
    std::fs::write(&path, format!("unemu-test-core 1\n{lines}\n")).unwrap();
    let dirs = HostDirs {
        saves: scratch.0.join("saves"),
        states: scratch.0.join("states"),
    };
    let prepared = prepare(&path, &Cores::new(&CORES), None, &dirs)
        .unwrap()
        .into_ready()
        .unwrap();
    let mut player = Player::start(prepared, PadBindings::defaults(), &mut io::sink()).unwrap();
    player.full_slices = true;
    player.session().full_slices = true;
    Rig {
        player,
        ctx: egui::Context::default(),
        at: Duration::ZERO,
        _scratch: scratch,
    }
}

impl Rig {
    /// Tick every sixtieth of a second for `ms` more milliseconds of host
    /// time.
    fn run(&mut self, ms: u64) {
        let end = self.at + Duration::from_millis(ms);
        while self.at < end {
            self.at = (self.at + Duration::from_micros(16_667)).min(end);
            self.player.tick(&self.ctx, false, self.at, &mut io::sink());
        }
    }

    fn command(&mut self, command: Command) -> Option<Request> {
        self.player.command(command, self.at, &mut io::sink())
    }

    /// The machine's virtual time.
    fn now(&mut self) -> u64 {
        self.player.session().machine().now().0
    }

    /// The pixel at `(x, y)` of the latest picture.
    fn pixel(&mut self, x: u32, y: u32) -> u32 {
        let picture = self.player.session().picture().expect("a picture");
        picture.pixels[(y * picture.width + x) as usize]
    }

    /// The frames finished since the machine last started, as its picture's
    /// background shows them.
    fn frames(&mut self) -> u32 {
        let visible = self.player.session().picture().unwrap().visible;
        self.pixel(visible.x, visible.y)
    }
}

/// The labels of a menu's items.
fn labels(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Item(item) => Some(item.label.clone()),
            _ => None,
        })
        .collect()
}

fn item(entries: &[Entry], label: &str) -> menus::Item {
    entries
        .iter()
        .find_map(|entry| match entry {
            Entry::Item(item) if item.label == label => Some(item.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {label} in {entries:?}"))
}

const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// A disk drive with two disks, the first inserted, an empty tape deck and
/// a fixed cartridge slot.
fn two_disks() -> String {
    format!(
        "slot 0 floppy removable fd Disk drive\n\
         medium 0 fd {DIGEST} 10 Disk 1\n\
         medium 0 fd {DIGEST} 10 Disk 2\n\
         insert 0 0\n\
         slot 1 tape removable tp Tape deck\n\
         slot 2 cartridge fixed utc Cartridge\n\
         media-changes"
    )
}

#[test]
fn a_machine_that_offers_nothing_has_the_controls_every_machine_has() {
    let mut rig = rig("");
    rig.run(100);
    let player = &rig.player;
    assert_eq!(
        player.offers(),
        Offers {
            reset: false,
            power_cycle_by_reset: false,
            media: vec![],
            saved_state: false,
            input: false,
            show_overscan: false,
            pointer_capture: false,
            aiming: false,
        }
    );
    assert_eq!(
        labels(&player.machine_entries()),
        ["Pause", "Power cycle", "Stop"]
    );
    assert!(player.media_entries().is_none());
    assert_eq!(
        labels(&player.view_entries()),
        [
            "Scale to window",
            "Integer scale",
            "Full screen",
            "Refresh remote",
            "Mute audio"
        ]
    );
    assert_eq!(
        labels(&player.speed_entries()),
        ["100%", "200%", "400%", "Unlimited", "Hold for Unlimited"]
    );
    // Input… is not offered, so the command opens nothing.
    rig.command(Command::Input);
    assert!(!rig.player.has_dialog());
}

#[test]
fn each_control_is_present_exactly_when_the_description_offers_it() {
    let mut rig = rig(&format!(
        "resets soft,hard\n\
         pad 0 up,down,a,b Controller 1\n\
         pad 1 up,down Controller 2\n\
         pointer 3 2\n\
         picture 256 240\n\
         visible 0 8 256 224\n\
         {}",
        two_disks()
    ));
    // Show overscan follows the picture, so it waits for the first one.
    assert!(!rig.player.offers().show_overscan);
    rig.run(100);
    let player = &rig.player;
    assert_eq!(
        player.offers(),
        Offers {
            reset: true,
            power_cycle_by_reset: true,
            media: vec![SlotId(0), SlotId(1)],
            saved_state: false,
            input: true,
            show_overscan: true,
            pointer_capture: true,
            aiming: false,
        }
    );
    assert_eq!(
        labels(&player.machine_entries()),
        ["Pause", "Reset", "Power cycle", "Stop", "Input…"]
    );
    assert!(labels(&player.view_entries()).contains(&"Show overscan".to_owned()));
    // One Media menu, a section per removable slot: the fixed cartridge
    // slot has none.
    let media = player.media_entries().unwrap();
    let headings: Vec<_> = media
        .iter()
        .filter_map(|entry| match entry {
            Entry::Heading(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(headings, ["Disk drive — Disk 1", "Tape deck — empty"]);
    assert_eq!(
        labels(&media),
        [
            "Eject",
            "Next in catalogue",
            "Disk 1",
            "Disk 2",
            "Eject",
            "Next in catalogue"
        ]
    );
    // The medium in the slot is marked; an empty slot greys Eject and an
    // empty catalogue greys Next, each saying why.
    assert_eq!(item(&media, "Disk 1").mark, "●");
    assert_eq!(item(&media, "Disk 2").mark, "");
    assert!(item(&media, "Eject").enabled);
    let tape: Vec<_> = media
        .iter()
        .skip_while(|entry| !matches!(entry, Entry::Heading(text) if text.starts_with("Tape")))
        .cloned()
        .collect();
    assert_eq!(
        item(&tape, "Eject").why.as_deref(),
        Some("Nothing is in the tape deck")
    );
    assert_eq!(
        item(&tape, "Next in catalogue").why.as_deref(),
        Some("The catalogue is empty")
    );
    assert_eq!(item(&media, "Eject").shortcut, "Ctrl+Alt+E");
    assert_eq!(item(&tape, "Eject").shortcut, "Ctrl+Alt+Shift+E");
}

#[test]
fn a_hard_reset_alone_offers_no_reset_and_power_cycles_by_resetting() {
    let rig = rig("resets hard");
    let offers = rig.player.offers();
    assert!(!offers.reset && offers.power_cycle_by_reset);
    assert!(!labels(&rig.player.machine_entries()).contains(&"Reset".to_owned()));
}

#[test]
fn an_absolute_pointer_aims_and_a_fixed_slot_has_no_media_menu() {
    let rig = rig("absolute 1 1 Light gun\nslot 0 cartridge fixed utc Cartridge");
    let offers = rig.player.offers();
    assert!(offers.aiming && !offers.pointer_capture && offers.media.is_empty());
    assert!(rig.player.media_entries().is_none());
    assert_eq!(
        rig.player.message(),
        "The pointer over the screen aims the Light gun; the primary button is its trigger"
    );
}

/// Open `menu` of the menu bar, drawn as the application draws it, and
/// return what it paints; then click `click`, if given, and return the
/// commands that pushed.
fn open_menu(player: &Player, menu: &str, click: Option<&str>) -> (Vec<String>, Vec<Command>) {
    let pointer = Pointer::new(egui::vec2(1000.0, 700.0));
    let commands = std::cell::RefCell::new(Vec::new());
    let mut chrome = (true, true, true);
    let mut draw = |ui: &mut egui::Ui| {
        let (toolbar, status_bar, classic) = &mut chrome;
        egui::MenuBar::new().ui(ui, |ui| {
            player.menus(
                ui,
                ViewChrome {
                    toolbar,
                    status_bar,
                    classic,
                },
                &mut commands.borrow_mut(),
            );
        });
    };
    let bar = pointer.frame(Vec::new(), &mut draw);
    pointer.click(painted_at(&bar, menu).center(), &mut draw);
    let shown = pointer.frame(Vec::new(), &mut draw);
    if let Some(label) = click {
        pointer.click(painted_at(&shown, label).center(), &mut draw);
    }
    let texts = shown.into_iter().map(|(text, _)| text).collect();
    (texts, commands.into_inner())
}

#[test]
fn the_drawn_menus_hold_what_the_description_offers_and_push_their_commands() {
    let bare = rig("");
    let (texts, _) = open_menu(&bare.player, "Machine", None);
    assert!(texts.contains(&"Power cycle".to_owned()), "{texts:?}");
    assert!(!texts.contains(&"Reset".to_owned()));
    assert!(!texts.contains(&"Input…".to_owned()));

    let mut full = rig(&format!(
        "resets soft\npad 0 up,down Controller 1\n{}",
        two_disks()
    ));
    full.run(50);
    let (texts, commands) = open_menu(&full.player, "Machine", Some("Reset"));
    assert!(texts.contains(&"Input…".to_owned()), "{texts:?}");
    assert!(texts.contains(&"Ctrl+Alt+End".to_owned()), "{texts:?}");
    assert_eq!(commands, [Command::Reset]);
    let (texts, commands) = open_menu(&full.player, "Media", Some("Disk 2"));
    assert!(
        texts.contains(&"Disk drive — Disk 1".to_owned()),
        "{texts:?}"
    );
    assert_eq!(commands, [Command::Insert(SlotId(0), 1)]);
    let (texts, commands) = open_menu(&full.player, "View", Some("Integer scale"));
    assert!(texts.contains(&"Toolbar".to_owned()), "{texts:?}");
    assert_eq!(commands, [Command::IntegerScale]);
    // macOS writes the player's chords with Command.
    full.player.set_look(true, true);
    let (texts, _) = open_menu(&full.player, "Machine", None);
    assert!(texts.contains(&"⌘End".to_owned()), "{texts:?}");
}

#[test]
fn reset_and_power_cycle_reach_the_machine() {
    // Reset is the soft reset: it restarts the frame count and keeps the
    // machine's time. This machine lists no other kind.
    let mut rig = rig("resets soft");
    rig.run(500);
    let before = rig.now();
    assert!(rig.frames() >= 25, "{}", rig.frames());
    assert_eq!(rig.command(Command::Reset), None);
    assert_eq!(rig.player.message(), "Reset");
    rig.run(100);
    assert!(rig.frames() <= 7, "{}", rig.frames());
    assert!(rig.now() > before);
    // Without a hard reset, Power cycle finishes the machine and starts it
    // again from its file: a fresh machine at time zero.
    rig.run(400);
    rig.command(Command::PowerCycle);
    assert_eq!(rig.now(), 0);
    assert_eq!(rig.player.message(), "Power cycled");
    rig.run(100);
    let after = rig.now();
    assert!(after > 0 && after <= 7_000, "{after}");

    // A listed hard reset is the power cycle: the same machine, whose time
    // goes on. Reset is not offered, so it does nothing.
    let mut rig = self::rig("resets hard");
    rig.run(500);
    let frames = rig.frames();
    rig.command(Command::Reset);
    rig.run(20);
    assert!(rig.frames() >= frames);
    let before = rig.now();
    rig.command(Command::PowerCycle);
    assert_eq!(rig.player.message(), "Power cycled");
    assert_eq!(rig.now(), before);
    rig.run(50);
    assert!(rig.frames() <= 4, "{}", rig.frames());
}

#[test]
fn pause_holds_the_machine_and_stop_finishes_it() {
    let mut rig = rig("");
    rig.run(200);
    rig.command(Command::Pause);
    assert!(rig.player.paused());
    assert_eq!(
        item(&rig.player.machine_entries(), "Resume").command,
        Some(Command::Pause)
    );
    let held = rig.now();
    rig.run(300);
    assert_eq!(rig.now(), held);
    assert_eq!(
        rig.player.status_cells()[1],
        status::Cell::Light(false, "Paused")
    );
    rig.command(Command::Pause);
    rig.run(300);
    // Resuming anchors pacing again: the paused time is not caught up.
    let ran = rig.now() - held;
    assert!(ran > 0 && ran <= 20_000, "{ran}");
    assert_eq!(
        rig.player.status_cells()[1],
        status::Cell::Light(true, "Running")
    );
    assert_eq!(
        rig.command(Command::Stop),
        Some(Request::Stopped { failure: None })
    );
    assert!(rig.player.session.is_none());
}

#[test]
fn the_media_menu_changes_the_medium_at_the_next_boundary() {
    let mut rig = rig(&two_disks());
    rig.run(20);
    let inserted = |rig: &Rig| rig.player.slots[0].state.as_ref().unwrap().inserted;
    assert_eq!(inserted(&rig), Some(0));
    rig.command(Command::Insert(SlotId(0), 1));
    assert_eq!(rig.player.message(), "Disk drive: Disk 2");
    rig.run(20);
    assert_eq!(inserted(&rig), Some(1));
    assert_eq!(
        item(&rig.player.media_entries().unwrap(), "Disk 2").mark,
        "●"
    );
    rig.command(Command::Eject(SlotId(0)));
    assert_eq!(rig.player.message(), "Disk drive: ejected");
    rig.run(20);
    assert_eq!(inserted(&rig), None);
    rig.command(Command::Next(SlotId(0)));
    assert_eq!(rig.player.message(), "Disk drive: Disk 1");
    rig.run(20);
    assert_eq!(inserted(&rig), Some(0));
    // The change kept the slot busy: its chip lights for a moment.
    assert_eq!(
        rig.player.status_cells()[2],
        status::Cell::Slot(true, "Disk drive: Disk 1".into(), false)
    );
    rig.run(300);
    assert_eq!(
        rig.player.status_cells()[2],
        status::Cell::Slot(false, "Disk drive: Disk 1".into(), false)
    );
}

#[test]
fn speed_mute_and_the_view_options_reach_the_session_and_the_view() {
    let mut rig = rig("picture 256 240\nvisible 0 8 256 224");
    rig.run(20);
    rig.command(Command::Speed(Speed::Double));
    assert_eq!(rig.player.session().pacing.selected(), Speed::Double);
    assert_eq!(item(&rig.player.speed_entries(), "200%").mark, "●");
    rig.command(Command::FastForward(true));
    assert_eq!(rig.player.session().pacing.effective(), Speed::Unlimited);
    rig.command(Command::FastForward(false));
    // Only the normal speed plays sound, so the user's mute shows there.
    rig.command(Command::Speed(Speed::Normal));
    rig.player.session().audio.set_active_for_test(true);
    rig.command(Command::Mute);
    assert!(rig.player.session().audio.muted());
    assert_eq!(
        labels(&rig.player.view_entries()).last().unwrap(),
        "Unmute audio"
    );
    assert!(!rig.player.show_overscan);
    rig.command(Command::ShowOverscan);
    assert!(rig.player.show_overscan);
    assert_eq!(item(&rig.player.view_entries(), "Show overscan").mark, "✓");
    rig.command(Command::IntegerScale);
    assert_eq!(item(&rig.player.view_entries(), "Integer scale").mark, "●");
    assert_eq!(rig.command(Command::Fullscreen), Some(Request::Fullscreen));
    // A picture shown whole has no overscan to show.
    let mut whole = self::rig("");
    whole.run(20);
    whole.command(Command::ShowOverscan);
    assert!(!whole.player.show_overscan);
}

/// Draw the screen into a panel of `size` at the viewport's origin, with
/// the host pointer at `pointer`, and return the cursor the frame asked for.
fn draw_screen(rig: &mut Rig, size: egui::Vec2, pointer: Option<egui::Pos2>) -> egui::CursorIcon {
    let player = &mut rig.player;
    let events = vec![match pointer {
        Some(at) => egui::Event::PointerMoved(at),
        None => egui::Event::PointerGone,
    }];
    let mut output = rig.ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ui| {
            player.screen(ui);
        },
    );
    output.textures_delta.clear();
    output.platform_output.cursor_icon
}

#[test]
fn the_screen_keeps_the_shape_the_machine_states() {
    let mut rig = rig("picture 256 240\naspect 128 105\nvisible 0 8 256 224");
    rig.run(50);
    let size = egui::vec2(900.0, 700.0);
    let shape = |rect: egui::Rect| rect.width() / rect.height();
    // Overscan hidden: the visible 256 × 224 at 128:105 for the whole
    // picture is 64:49, as wide as the panel allows and centred.
    draw_screen(&mut rig, size, None);
    let shown = rig.player.screen_rect().unwrap();
    assert!((shape(shown) - 64.0 / 49.0).abs() < 1e-4, "{shown:?}");
    assert!((shown.width() - 900.0).abs() < 1e-3);
    assert!((shown.center().y - 350.0).abs() < 1e-3);
    assert_eq!(rig.player.layout.unwrap().source.height, 224);
    // Shown, the whole picture at 128:105.
    rig.command(Command::ShowOverscan);
    draw_screen(&mut rig, size, None);
    let shown = rig.player.screen_rect().unwrap();
    assert!((shape(shown) - 128.0 / 105.0).abs() < 1e-4, "{shown:?}");
    // Integer scale: a whole multiple of the rows shown, the shape kept.
    rig.command(Command::IntegerScale);
    draw_screen(&mut rig, size, None);
    let shown = rig.player.screen_rect().unwrap();
    assert_eq!(shown.height(), 480.0);
    assert!((shape(shown) - 128.0 / 105.0).abs() < 1e-4);
    rig.command(Command::ShowOverscan);
    draw_screen(&mut rig, size, None);
    let shown = rig.player.screen_rect().unwrap();
    assert_eq!(shown.height(), 672.0);
    assert!((shape(shown) - 64.0 / 49.0).abs() < 1e-4);
}

#[test]
fn the_pointer_over_the_screen_aims_with_a_cross_hair() {
    let mut rig = rig("picture 64 48\nabsolute 1 1 Light gun\nvisible 0 8 64 32");
    rig.run(50);
    let size = egui::vec2(640.0, 480.0);
    draw_screen(&mut rig, size, None);
    let shown = rig.player.screen_rect().unwrap();
    // The centre of the shown rectangle is the middle of the visible part:
    // column 32, row 8 + 16 of the whole picture.
    let cursor = draw_screen(&mut rig, size, Some(shown.center()));
    assert_eq!(cursor, egui::CursorIcon::Crosshair);
    rig.run(50);
    assert_eq!(rig.pixel(32, 24), picture::cross_colour(0));
    assert_eq!(rig.pixel(34, 24), picture::cross_colour(0));
    // Off the picture the place is none, and the cross goes.
    let cursor = draw_screen(&mut rig, size, Some(egui::pos2(1.0, 1.0)));
    assert_ne!(cursor, egui::CursorIcon::Crosshair);
    rig.run(50);
    assert_ne!(rig.pixel(32, 24), picture::cross_colour(0));
    // A machine without an absolute pointer keeps the default cursor.
    let mut plain = self::rig("picture 64 48");
    plain.run(50);
    draw_screen(&mut plain, size, None);
    let centre = plain.player.screen_rect().unwrap().center();
    assert_eq!(
        draw_screen(&mut plain, size, Some(centre)),
        egui::CursorIcon::Default
    );
}

/// Draw the player's dialogs for one frame of `pointer`.
fn dialog_frame(
    pointer: &Pointer,
    player: &mut Player,
    events: Vec<egui::Event>,
    requests: &mut Vec<Request>,
) -> Vec<(String, egui::Rect)> {
    pointer.frame(events, &mut |ui| requests.extend(player.dialogs(ui.ctx())))
}

fn input_dialog(player: &mut Player) -> &mut input_dialog::InputDialog {
    match &mut player.dialog {
        Some(Dialog::Input(dialog)) => dialog,
        _ => panic!("the Input dialog is not open"),
    }
}

#[test]
fn the_input_dialog_binds_a_key_that_then_works_the_pad() {
    let mut rig =
        rig("pad 0 up,down,left,right,a,b,start,select Controller 1\npad 1 up,down Controller 2");
    rig.run(20);
    rig.command(Command::Input);
    assert!(rig.player.has_dialog());
    let pointer = Pointer::new(egui::vec2(1000.0, 800.0));
    let mut requests = Vec::new();
    let mut shown = Vec::new();
    for _ in 0..3 {
        shown = dialog_frame(&pointer, &mut rig.player, Vec::new(), &mut requests);
    }
    // A tab per pad port by its label; a row per control with its keys.
    painted_at(&shown, "Controller 1");
    painted_at(&shown, "Controller 2");
    painted_at(&shown, "Arrow Up");
    painted_at(&shown, "Shift Right");
    painted_at(&shown, input_dialog::HINT);
    // Click a row: it waits for a key.
    let up = painted_at(&shown, "Up").center();
    pointer.click(up, &mut |ui| {
        requests.extend(rig.player.dialogs(ui.ctx()));
    });
    let shown = dialog_frame(&pointer, &mut rig.player, Vec::new(), &mut requests);
    painted_at(&shown, "Press a key…");
    rig.player.key_event(KeyCode::KeyK, ElementState::Pressed);
    rig.player.key_event(KeyCode::KeyK, ElementState::Released);
    let shown = dialog_frame(&pointer, &mut rig.player, Vec::new(), &mut requests);
    painted_at(&shown, "K");
    // The machine's pad is untouched until OK.
    assert_eq!(rig.player.session().pad_bindings().control(Key::K), None);
    let ok = painted_at(&shown, "OK").center();
    pointer.click(ok, &mut |ui| {
        requests.extend(rig.player.dialogs(ui.ctx()));
    });
    assert_eq!(requests, [Request::PadBindings]);
    assert!(!rig.player.has_dialog());
    assert_eq!(
        rig.player.pad_bindings().control(Key::K),
        Some((0, PadControl::Up))
    );
    assert_eq!(rig.player.pad_bindings().control(Key::ARROW_UP), None);
    // The next press of K moves the test machine's field.
    rig.player.key_event(KeyCode::KeyK, ElementState::Pressed);
    rig.run(50);
    let field = picture::field_area(rig.player.session().picture().unwrap().visible);
    assert_eq!(
        rig.pixel(field.x, field.y),
        picture::field_colour(&[&[PadControl::Up]])
    );
    rig.player.key_event(KeyCode::KeyK, ElementState::Released);
    rig.run(50);
    assert_eq!(rig.pixel(field.x, field.y), picture::field_colour(&[]));
}

#[test]
fn a_waiting_row_takes_escape_and_delete_and_a_key_leaves_its_old_control() {
    let mut rig = rig("pad 0 up,down,a,b Controller 1");
    rig.command(Command::Input);
    let pointer = Pointer::new(egui::vec2(1000.0, 800.0));
    let mut requests = Vec::new();
    for _ in 0..3 {
        dialog_frame(&pointer, &mut rig.player, Vec::new(), &mut requests);
    }
    // Esc leaves the row as it was, and the same press does not close the
    // dialog when egui sees it after the raw key.
    input_dialog(&mut rig.player).waiting = Some(PadControl::Up);
    rig.player.key_event(KeyCode::Escape, ElementState::Pressed);
    let escape = egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    dialog_frame(
        &pointer,
        &mut rig.player,
        vec![escape.clone()],
        &mut requests,
    );
    assert!(rig.player.has_dialog());
    let dialog = input_dialog(&mut rig.player);
    assert_eq!(dialog.waiting, None);
    assert_eq!(dialog.keys_of(PadControl::Up), ["Arrow Up"]);
    // Nor when egui sees it first, while the row still waits.
    dialog.waiting = Some(PadControl::Up);
    dialog_frame(
        &pointer,
        &mut rig.player,
        vec![escape.clone()],
        &mut requests,
    );
    assert!(rig.player.has_dialog());
    rig.player.key_event(KeyCode::Escape, ElementState::Pressed);
    let dialog = input_dialog(&mut rig.player);
    assert_eq!(dialog.keys_of(PadControl::Up), ["Arrow Up"]);
    // Delete clears the row.
    dialog.waiting = Some(PadControl::Up);
    rig.player.key_event(KeyCode::Delete, ElementState::Pressed);
    let dialog = input_dialog(&mut rig.player);
    assert!(dialog.keys_of(PadControl::Up).is_empty());
    // A key replaces the row's keys and leaves the control that had it.
    dialog.waiting = Some(PadControl::A);
    rig.player
        .key_event(KeyCode::ArrowDown, ElementState::Pressed);
    let dialog = input_dialog(&mut rig.player);
    assert_eq!(dialog.keys_of(PadControl::A), ["Arrow Down"]);
    assert!(dialog.keys_of(PadControl::Down).is_empty());
    // A key pressed while no row waits changes nothing.
    rig.player.key_event(KeyCode::KeyJ, ElementState::Pressed);
    assert_eq!(input_dialog(&mut rig.player).keys_of(PadControl::B), ["Z"]);
    // Defaults restores every player's keys.
    let shown = dialog_frame(&pointer, &mut rig.player, Vec::new(), &mut requests);
    pointer.click(painted_at(&shown, "Defaults").center(), &mut |ui| {
        requests.extend(rig.player.dialogs(ui.ctx()));
    });
    assert_eq!(input_dialog(&mut rig.player).draft, PadBindings::defaults());
    // Escape with no row waiting cancels, and nothing changed.
    input_dialog(&mut rig.player).draft = PadBindings::empty();
    dialog_frame(&pointer, &mut rig.player, vec![escape], &mut requests);
    assert!(!rig.player.has_dialog());
    assert!(requests.is_empty());
    assert_eq!(rig.player.pad_bindings(), &PadBindings::defaults());
}

#[test]
fn a_machine_that_fails_shows_a_message_box_and_stop_reports_it() {
    let mut rig = rig("at 600 fail the guest gave up");
    rig.run(100);
    assert!(rig.player.has_dialog());
    // A failure is not a pause: the player says the machine stopped.
    assert!(!rig.player.paused() && !rig.player.running());
    assert_eq!(rig.player.condition(), Condition::Stopped);
    let pointer = Pointer::new(egui::vec2(1000.0, 800.0));
    let mut requests = Vec::new();
    let mut shown = Vec::new();
    for _ in 0..3 {
        shown = dialog_frame(&pointer, &mut rig.player, Vec::new(), &mut requests);
    }
    painted_at(&shown, "Machine");
    assert!(
        shown
            .iter()
            .any(|(text, _)| text.contains("the guest gave up")),
        "{shown:?}"
    );
    // A click outside leaves the box; OK closes it.
    pointer.click(egui::pos2(5.0, 5.0), &mut |ui| {
        requests.extend(rig.player.dialogs(ui.ctx()));
    });
    assert!(rig.player.has_dialog());
    let shown = dialog_frame(&pointer, &mut rig.player, Vec::new(), &mut requests);
    pointer.click(painted_at(&shown, "OK").center(), &mut |ui| {
        requests.extend(rig.player.dialogs(ui.ctx()));
    });
    assert!(!rig.player.has_dialog());
    match rig.command(Command::Stop) {
        Some(Request::Stopped {
            failure: Some(failure),
        }) => assert!(failure.contains("the guest gave up"), "{failure}"),
        other => panic!("{other:?}"),
    }
}

/// The texts the screen paints.
fn screen_texts(rig: &mut Rig) -> Vec<String> {
    let pointer = Pointer::new(egui::vec2(640.0, 480.0));
    let player = &mut rig.player;
    pointer
        .frame(Vec::new(), &mut |ui| {
            player.screen(ui);
        })
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

fn note(rig: &Rig) -> String {
    match &rig.player.machine_entries()[0] {
        Entry::Note(text) => text.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_machine_that_switches_itself_off_is_stopped_not_paused() {
    let mut rig = rig("at 600 power-off");
    rig.run(100);
    assert!(!rig.player.paused() && !rig.player.running());
    assert_eq!(rig.player.message(), SWITCHED_OFF);
    assert_eq!(
        rig.player.status_cells()[1],
        status::Cell::Light(false, "Stopped")
    );
    assert_eq!(note(&rig), "For the machine: machine.utc (stopped)");
    let pause = item(&rig.player.machine_entries(), "Pause");
    assert!(!pause.enabled);
    assert_eq!(pause.why.as_deref(), Some("The machine has stopped"));
    // Ctrl+Alt+P does nothing: no pause, no PAUSED, the message stays.
    rig.command(Command::Pause);
    assert!(!rig.player.paused());
    assert_eq!(rig.player.message(), SWITCHED_OFF);
    assert!(!screen_texts(&mut rig).contains(&"PAUSED".to_owned()));
    // Power cycle starts it again, running.
    rig.command(Command::PowerCycle);
    assert!(rig.player.running());
    assert_eq!(rig.player.condition(), Condition::Running);
}

#[test]
fn a_halted_machine_says_so_and_power_cycle_starts_it_again() {
    let mut rig = rig("at 600 halt");
    rig.run(100);
    assert!(!rig.player.paused() && !rig.player.running());
    assert_eq!(rig.player.message(), HALTED);
    assert_eq!(
        rig.player.status_cells()[1],
        status::Cell::Light(false, "Halted")
    );
    assert_eq!(note(&rig), "For the machine: machine.utc (halted)");
    let pause = item(&rig.player.machine_entries(), "Pause");
    assert_eq!(pause.why.as_deref(), Some("The machine waits for a reset"));
    rig.command(Command::Pause);
    assert!(!rig.player.paused());
    assert!(!screen_texts(&mut rig).contains(&"PAUSED".to_owned()));
    // Without a hard reset Power cycle starts the machine again.
    rig.command(Command::PowerCycle);
    assert!(rig.player.running());
    assert_eq!(rig.now(), 0);
    rig.run(5);
    assert!(rig.now() > 0);
    // The user's pause of a running machine does show PAUSED.
    rig.command(Command::Pause);
    assert_eq!(note(&rig), "For the machine: machine.utc (paused)");
    assert!(screen_texts(&mut rig).contains(&"PAUSED".to_owned()));
}

#[test]
fn a_halted_machine_that_lists_a_hard_reset_power_cycles_by_resetting() {
    let mut rig = rig("resets hard\nat 600 halt");
    rig.run(100);
    assert_eq!(rig.player.condition(), Condition::Halted);
    let halted_at = rig.now();
    assert!(halted_at >= 600);
    rig.command(Command::PowerCycle);
    // The same machine: its time goes on from the halt, and it runs.
    assert_eq!(rig.now(), halted_at);
    assert!(rig.player.running());
    rig.run(50);
    assert!(rig.now() > halted_at);
    // Reset of a halted machine runs it again, too.
    let mut rig = self::rig("resets soft\nat 600 halt");
    rig.run(100);
    let halted_at = rig.now();
    rig.command(Command::Reset);
    assert_eq!(rig.player.condition(), Condition::Running);
    rig.run(50);
    assert!(rig.now() > halted_at);
}

#[test]
fn power_cycle_resumes_and_keeps_the_speed_and_the_mute() {
    // By restarting: the speed, the user's mute and the resume.
    let mut rig = rig("resets soft");
    rig.run(50);
    rig.command(Command::Speed(Speed::Normal));
    rig.player.session().audio.set_active_for_test(true);
    rig.command(Command::Mute);
    assert!(rig.player.session().audio.user_muted());
    rig.command(Command::Speed(Speed::Double));
    rig.command(Command::Pause);
    assert!(rig.player.paused());
    rig.command(Command::PowerCycle);
    assert_eq!(rig.now(), 0);
    assert!(!rig.player.paused());
    assert_eq!(rig.player.session().pacing.selected(), Speed::Double);
    assert_eq!(rig.player.selected_speed(), Speed::Double);
    assert_eq!(item(&rig.player.speed_entries(), "200%").mark, "●");
    // The new machine's sound is muted once the host device plays it.
    assert!(!rig.player.session().audio.user_muted());
    rig.player.session().audio.set_active_for_test(true);
    rig.run(20);
    assert!(rig.player.session().audio.user_muted());

    // By resetting: the resume too.
    let mut rig = self::rig("resets hard");
    rig.run(50);
    rig.command(Command::Pause);
    rig.command(Command::PowerCycle);
    assert!(!rig.player.paused());
    assert!(rig.now() > 0);
}

#[test]
fn a_machine_that_cannot_start_again_shows_why_and_still_stops() {
    let mut rig = rig("resets soft");
    rig.run(50);
    rig.command(Command::Speed(Speed::Quadruple));
    // The file changed since it was opened: the start refuses it.
    let path = rig._scratch.0.join("files").join("machine.utc");
    std::fs::write(&path, "unemu-test-core 1\ntitle changed\n").unwrap();
    rig.command(Command::PowerCycle);
    assert!(rig.player.session.is_none());
    assert_eq!(rig.player.message(), STOPPED);
    assert_eq!(rig.player.condition(), Condition::Stopped);
    assert!(!rig.player.paused());
    assert_eq!(rig.player.selected_speed(), Speed::Quadruple);
    match &rig.player.dialog {
        Some(Dialog::Message(message)) => {
            assert_eq!(message.title, "Power cycle");
            assert!(message
                .text
                .starts_with("The machine could not start again"));
        }
        _ => panic!("no Power cycle box"),
    }
    // Ticks and the menus go on without a machine; Pause does nothing.
    rig.run(20);
    assert!(!item(&rig.player.machine_entries(), "Pause").enabled);
    rig.command(Command::Pause);
    assert!(!rig.player.paused());
    assert_eq!(
        rig.command(Command::Stop),
        Some(Request::Stopped { failure: None })
    );
}

#[test]
fn releases_reach_the_machine_while_a_dialog_shows() {
    let mut rig = rig("pad 0 up,down Controller 1\nabsolute 1 1 Light gun\npicture 64 48");
    rig.run(20);
    let size = egui::vec2(640.0, 480.0);
    draw_screen(&mut rig, size, None);
    let centre = rig.player.screen_rect().unwrap().center();
    draw_screen(&mut rig, size, Some(centre));
    rig.player
        .key_event(KeyCode::ArrowUp, ElementState::Pressed);
    rig.player
        .mouse_button(MouseButton::Left, ElementState::Pressed);
    rig.run(50);
    let field = picture::field_area(rig.player.session().picture().unwrap().visible);
    assert_eq!(
        rig.pixel(field.x, field.y),
        picture::field_colour(&[&[PadControl::Up]])
    );
    assert_eq!(rig.pixel(32, 24), picture::cross_colour(1));
    rig.command(Command::Input);
    // While the dialog shows, presses wait and releases go through.
    rig.player
        .key_event(KeyCode::ArrowDown, ElementState::Pressed);
    rig.player
        .key_event(KeyCode::ArrowUp, ElementState::Released);
    rig.player
        .mouse_button(MouseButton::Left, ElementState::Released);
    rig.player
        .guest_key(KeyCode::ArrowDown, ElementState::Pressed);
    let pointer = Pointer::new(egui::vec2(1000.0, 800.0));
    let mut requests = Vec::new();
    let mut shown = Vec::new();
    for _ in 0..3 {
        shown = dialog_frame(&pointer, &mut rig.player, Vec::new(), &mut requests);
    }
    pointer.click(painted_at(&shown, "Cancel").center(), &mut |ui| {
        requests.extend(rig.player.dialogs(ui.ctx()));
    });
    assert!(!rig.player.has_dialog());
    rig.run(50);
    assert_eq!(rig.pixel(field.x, field.y), picture::field_colour(&[]));
    assert_eq!(rig.pixel(32, 24), picture::cross_colour(0));
    // Past the dialog, the pass-through key reaches the pad.
    rig.player
        .guest_key(KeyCode::ArrowDown, ElementState::Pressed);
    rig.run(50);
    assert_eq!(
        rig.pixel(field.x, field.y),
        picture::field_colour(&[&[PadControl::Down]])
    );
}

#[test]
fn the_slots_follow_the_machine_while_it_does_not_advance() {
    use unemu_core_api::{Content, NewMedium, WriteTarget};
    let add = |rig: &mut Rig, name: &str| {
        rig.player
            .session()
            .machine_mut()
            .add_medium(
                SlotId(0),
                NewMedium::new(
                    name,
                    Content::Bytes(vec![0; 10]),
                    [0; 32],
                    10,
                    "fd",
                    WriteTarget::ReadOnly,
                ),
            )
            .unwrap();
    };
    let listed = |rig: &Rig, name: &str| {
        labels(&rig.player.media_entries().unwrap()).contains(&name.to_owned())
    };
    // Paused by the user.
    let mut rig = rig(&two_disks());
    rig.run(20);
    rig.command(Command::Pause);
    add(&mut rig, "Disk 3");
    rig.run(20);
    assert!(listed(&rig, "Disk 3"));
    // Halted.
    let mut rig = self::rig(&format!("{}\nat 600 halt", two_disks()));
    rig.run(100);
    assert_eq!(rig.player.condition(), Condition::Halted);
    add(&mut rig, "Disk 4");
    rig.run(20);
    assert!(listed(&rig, "Disk 4"));
}

#[test]
fn the_application_s_media_hotkeys_follow_the_menu_s_checks() {
    let mut rig = rig(&two_disks());
    rig.run(20);
    let before = rig.player.message().to_owned();
    // The tape deck is empty and has no catalogue.
    rig.command(Command::Eject(SlotId(1)));
    rig.command(Command::Next(SlotId(1)));
    rig.command(Command::Insert(SlotId(0), 7));
    assert_eq!(rig.player.message(), before);
    assert!(rig.player.session().media_events.is_empty());
}

#[test]
fn a_cancelled_question_gives_the_captured_pointer_back() {
    let mut rig = rig("pointer 1 2");
    rig.run(20);
    rig.player.focus_changed(true);
    rig.player.recapture_pointer();
    assert!(rig.player.pointer_captured());
    rig.player.release_pointer();
    assert!(!rig.player.pointer_captured());
    // Not while the user paused the machine.
    rig.command(Command::Pause);
    rig.player.recapture_pointer();
    assert!(!rig.player.pointer_captured());
    rig.command(Command::Pause);
    rig.player.recapture_pointer();
    assert!(rig.player.pointer_captured());
}

#[test]
fn the_title_and_the_status_line_name_the_content_and_its_platform() {
    let rig = rig(&format!("title Two drives\n{}", two_disks()));
    assert_eq!(
        rig.player.title("unemu"),
        "Two drives — Test machine — unemu"
    );
    assert_eq!(
        rig.player.status_cells(),
        [
            status::Cell::Message("Opened machine.utc".into()),
            status::Cell::Light(true, "Running"),
            status::Cell::Slot(false, "Disk drive: Disk 1".into(), false),
            status::Cell::Slot(false, "Tape deck: empty".into(), true),
            status::Cell::Text("Test machine".into()),
            status::Cell::Text("Two drives".into()),
            status::Cell::Text("🔇 unavailable".into()),
            status::Cell::Text("Speed 100%".into()),
        ]
    );
    // The status line draws without a window.
    let pointer = Pointer::new(egui::vec2(1000.0, 40.0));
    let texts = pointer.frame(Vec::new(), &mut |ui| rig.player.status(ui));
    painted_at(&texts, "Tape deck: empty");
}

/// The NES platform's name, the longest a core gives.
const NES: &str = "Nintendo Entertainment System (NTSC)";

/// A player's status line of `condition` with `title` and `speed`.
fn nes_cells(condition: Condition, title: &str, speed: &str) -> Vec<status::Cell> {
    vec![
        status::Cell::Message("Pointer captured — Ctrl+Alt+G releases".into()),
        status::Cell::Light(condition == Condition::Running, condition.word()),
        status::Cell::Slot(false, "Cartridge: Super Game".into(), false),
        status::Cell::Text(NES.into()),
        status::Cell::Text(title.into()),
        status::Cell::Text("🔊 48 kHz".into()),
        status::Cell::Text(speed.into()),
    ]
}

/// Draw `cells` `width` wide; return each cell's rectangle and every painted
/// text.
fn draw_status(
    width: f32,
    cells: Vec<status::Cell>,
) -> (Vec<egui::Rect>, Vec<(String, egui::Rect)>) {
    let pointer = Pointer::new(egui::vec2(width, 40.0));
    let mut areas = Vec::new();
    let painted = pointer.frame(Vec::new(), &mut |ui| {
        areas = status::draw_cells(ui, cells.clone());
    });
    (areas, painted)
}

/// Every painted text lies inside the cell it starts in: shown whole or
/// ellipsized, never cut by the cell's edge.
fn assert_texts_inside(areas: &[egui::Rect], painted: &[(String, egui::Rect)], case: &str) {
    for (text, glyphs) in painted {
        let area = areas
            .iter()
            .find(|area| area.left() <= glyphs.left() && glyphs.left() < area.right())
            .unwrap_or_else(|| panic!("{case}: {text:?} at {glyphs:?} starts in no cell"));
        assert!(
            glyphs.right() <= area.right(),
            "{case}: {text:?} covers {glyphs:?}, its cell {area:?}"
        );
    }
}

/// Where `text` is painted whole, not ellipsized: a label cut short keeps
/// its text but paints fewer glyphs.
fn painted_whole(painted: &[(String, egui::Rect)], text: &str) -> egui::Rect {
    let glyphs = painted_at(painted, text);
    let mut whole = 0.0;
    Pointer::new(egui::vec2(100.0, 40.0)).frame(Vec::new(), &mut |ui| {
        let font = egui::TextStyle::Body.resolve(ui.style());
        whole = ui
            .painter()
            .layout_no_wrap(text.to_owned(), font, egui::Color32::PLACEHOLDER)
            .size()
            .x;
    });
    assert!(
        glyphs.width() >= whole - 0.5,
        "{text:?} paints {} of its {whole} px",
        glyphs.width()
    );
    glyphs
}

const ALL_CONDITIONS: [Condition; 4] = [
    Condition::Running,
    Condition::Paused,
    Condition::Halted,
    Condition::Stopped,
];

#[test]
fn every_status_cell_holds_its_text_and_the_light_does_not_move() {
    let mut light = None;
    for condition in ALL_CONDITIONS {
        let (areas, painted) =
            draw_status(1040.0, nes_cells(condition, "Super Game", "Speed 100%"));
        assert_texts_inside(&areas, &painted, &format!("{condition:?}"));
        painted_whole(&painted, condition.word());
        painted_whole(&painted, NES);
        // The light's cell is as wide in every state, so nothing shifts.
        assert_eq!(*light.get_or_insert(areas[1]), areas[1], "{condition:?}");
    }
}

#[test]
fn a_narrow_status_line_ellipsizes_the_names_and_keeps_the_light_and_the_message() {
    let title = "The Legend of the Mystical Ninja: The Extended Anniversary Edition";
    for condition in ALL_CONDITIONS {
        let (areas, painted) = draw_status(760.0, nes_cells(condition, title, "Speed 100%"));
        let case = format!("{condition:?} at 760 px");
        assert_texts_inside(&areas, &painted, &case);
        // The light's word is painted whole, inside its cell.
        let word = painted_whole(&painted, condition.word());
        assert!(
            areas[1].contains_rect(word),
            "{case}: {word:?} {:?}",
            areas[1]
        );
        assert!(
            areas[0].width() >= status::MESSAGE_MIN,
            "{case}: the message is {} wide",
            areas[0].width()
        );
    }
}

#[test]
fn sound_and_speed_widen_for_their_text() {
    let speed = "Speed 100% 1.0x · fast-forward 8x";
    let (areas, painted) = draw_status(1040.0, nes_cells(Condition::Running, "Super Game", speed));
    let glyphs = painted_whole(&painted, speed);
    let cell = areas[areas.len() - 1];
    assert!(cell.contains_rect(glyphs), "{glyphs:?} {cell:?}");
}

#[test]
fn keys_read_by_the_names_the_preferences_file_derives_them_from() {
    assert_eq!(key_label(Key::ARROW_UP), "Arrow Up");
    assert_eq!(key_label(Key::SHIFT_RIGHT), "Shift Right");
    assert_eq!(key_label(Key::DIGIT_1), "1");
    assert_eq!(key_label(Key::X), "X");
    assert_eq!(key_label(Key::ENTER), "Enter");
}

#[test]
fn the_marks_the_player_writes_are_the_mockups_characters() {
    use ui_kit::shell_icons::inventory::{Art, Place, INVENTORY};
    assert_eq!(
        [
            menus::PAUSE_MARK,
            menus::STOP_MARK,
            menus::CHOSEN_MARK,
            menus::ON_MARK,
            ui_kit::skin::EJECT_MARK,
            ui_kit::skin::audio_mark(true),
            ui_kit::skin::audio_mark(false),
        ],
        ["Ⅱ", "■", "●", "✓", "⏏", "🔊", "🔇"]
    );
    for mark in ["Ⅱ", "■", "●", "✓", "⏏", "🔊", "🔇"] {
        let ch = mark.chars().next().unwrap();
        assert!(
            INVENTORY.iter().any(|entry| entry.mockup == "player.html"
                && matches!(entry.art, Art::Glyph(g) if g == ch)
                && matches!(entry.place, Place::Text(_))),
            "{mark} is not listed as written in the player"
        );
    }
}

/// The quick slot's file of the rig's content.
fn quick_state(rig: &Rig) -> PathBuf {
    quick_state::path(rig.player.prepared().states_dir().unwrap())
}

/// The title and the text of the message box the player shows.
fn message_box(rig: &Rig) -> (String, String) {
    match &rig.player.dialog {
        Some(Dialog::Message(message)) => (message.title.clone(), message.text.clone()),
        _ => panic!("no message box"),
    }
}

/// The latest picture's pixels.
fn pixels(rig: &mut Rig) -> Vec<u32> {
    rig.player.session().picture().unwrap().pixels.clone()
}

#[test]
fn save_state_run_on_and_load_state_bring_the_saved_picture_back() {
    let mut rig = rig("saved-state");
    rig.run(200);
    assert!(rig.player.offers().saved_state);
    // After Stop, separated: Save state, and Load state greyed with its
    // reason until a state exists; Ctrl+Alt+L does nothing meanwhile.
    let entries = rig.player.machine_entries();
    assert_eq!(
        labels(&entries),
        ["Pause", "Power cycle", "Stop", "Save state", "Load state"]
    );
    assert_eq!(entries[entries.len() - 3], Entry::Separator);
    let save = item(&entries, "Save state");
    assert!(save.enabled);
    assert_eq!(save.shortcut, "Ctrl+Alt+S");
    let load = item(&entries, "Load state");
    assert!(!load.enabled);
    assert_eq!(load.shortcut, "Ctrl+Alt+L");
    assert_eq!(
        load.why.as_deref(),
        Some("No state saved for this content yet")
    );
    let (texts, _) = open_menu(&rig.player, "Machine", None);
    assert!(texts.contains(&"Ctrl+Alt+S".to_owned()), "{texts:?}");
    rig.command(Command::LoadState);
    assert!(!rig.player.has_dialog());
    assert_eq!(rig.player.message(), "Opened machine.utc");

    // Save: the two lines, then the core's own bytes, at the content's
    // states directory.
    let (texts, commands) = open_menu(&rig.player, "Machine", Some("Save state"));
    assert!(texts.contains(&"Load state".to_owned()));
    assert_eq!(commands, [Command::SaveState]);
    rig.command(Command::SaveState);
    assert_eq!(rig.player.message(), "State saved");
    let saved_at = rig.now();
    let saved_picture = pixels(&mut rig);
    let saved_frames = rig.frames();
    let path = quick_state(&rig);
    let digest: String = (rig.player.prepared().sha256().unwrap().iter())
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert!(path.ends_with(format!(
        "states/unemu-test-core/{}/{digest}/quick.state",
        &digest[..2]
    )));
    let identity = rig.player.session().state_identity().unwrap();
    let mut core = Vec::new();
    rig.player.session().save_state(&mut core).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        quick_state::encode(&identity, &core)
    );
    assert!(!quick_state::partial_path(rig.player.prepared().states_dir().unwrap()).exists());
    assert!(item(&rig.player.machine_entries(), "Load state").enabled);

    // Run on, then load: the saved picture and the saved time are back.
    rig.run(300);
    assert!(rig.frames() > saved_frames + 10, "{}", rig.frames());
    rig.command(Command::LoadState);
    assert!(!rig.player.has_dialog());
    assert_eq!(rig.player.message(), "State loaded");
    assert_eq!(rig.now(), saved_at);
    assert_eq!(pixels(&mut rig), saved_picture);
    assert!(rig.player.running());
    rig.run(100);
    assert!(rig.now() > saved_at);

    // The next machine of the same content finds the slot at its start.
    rig.command(Command::Stop);
    let Rig {
        player, _scratch, ..
    } = rig;
    drop(player);
    let mut again = rig_in(_scratch, "saved-state");
    assert!(item(&again.player.machine_entries(), "Load state").enabled);
    again.run(50);
    again.command(Command::LoadState);
    assert_eq!(again.player.message(), "State loaded");
    assert_eq!(again.now(), saved_at);
    assert_eq!(pixels(&mut again), saved_picture);
}

#[test]
fn a_state_of_another_identity_or_a_damaged_file_is_refused_and_the_machine_runs_on() {
    let mut rig = rig("saved-state");
    rig.run(100);
    rig.command(Command::SaveState);
    let path = quick_state(&rig);
    let good = std::fs::read(&path).unwrap();
    let identity = rig.player.session().state_identity().unwrap();
    let core = good[quick_state::encode(&identity, &[]).len()..].to_vec();
    rig.run(200);
    for (file, reason) in [
        // Bytes the core itself would take: only the identity line says
        // they are another machine's.
        (
            quick_state::encode("unemu-test-core 0.0.0 script other", &core),
            Some("this state was saved by another release of the core, or for another setup"),
        ),
        (b"unemu-state 1\ncut short".to_vec(), Some(DAMAGED)),
        (
            quick_state::encode(&identity, &core).split_off(1),
            Some(DAMAGED),
        ),
        (
            [b"unemu-state 2".as_slice(), &good[13..]].concat(),
            Some(DAMAGED),
        ),
        // An identity line longer than the header may be.
        (quick_state::encode(&"x".repeat(5000), &core), Some(DAMAGED)),
        // The core's own refusal of its bytes, cut short.
        (
            quick_state::encode(&identity, &core[..core.len() / 2]),
            None,
        ),
    ] {
        std::fs::write(&path, &file).unwrap();
        let now = rig.now();
        let picture = pixels(&mut rig);
        rig.command(Command::LoadState);
        let (title, text) = message_box(&rig);
        assert_eq!(title, "Load state");
        let given = text
            .strip_prefix("The state was not loaded: ")
            .and_then(|text| text.strip_suffix(".\n\nThe machine runs on as it was."))
            .unwrap_or_else(|| panic!("{text}"));
        match reason {
            Some(reason) => assert_eq!(given, reason),
            // The core's own words, whatever they are.
            None => assert!(!given.is_empty(), "{text}"),
        }
        assert_eq!(rig.player.message(), "State not loaded");
        assert_eq!(rig.now(), now);
        assert_eq!(pixels(&mut rig), picture);
        assert!(rig.player.running());
        rig.player.dialog = None;
        rig.run(50);
        assert!(rig.now() > now);
    }
    // The file that was refused is left as it is.
    assert_eq!(
        std::fs::read(&path).unwrap(),
        quick_state::encode(&identity, &core[..core.len() / 2])
    );
}

/// The reason for a file without the slot's layout.
const DAMAGED: &str = "the file is damaged, or it is not a saved state this program reads";

#[test]
fn a_state_saved_while_running_runs_again_after_a_halt_and_one_saved_halted_halts_unpaused() {
    // Saved while running, loaded once the guest halted: the machine runs
    // and advances again.
    let mut rig = rig("saved-state\nat 6000 halt");
    rig.run(50);
    rig.command(Command::SaveState);
    let saved_at = rig.now();
    rig.run(200);
    assert_eq!(rig.player.condition(), Condition::Halted);
    rig.command(Command::LoadState);
    assert_eq!(rig.player.message(), "State loaded");
    assert_eq!(rig.player.condition(), Condition::Running);
    assert_eq!(rig.now(), saved_at);
    rig.run(20);
    assert!(rig.now() > saved_at, "{}", rig.now());

    // Saved while halted, loaded into a running machine the user paused:
    // halted again, and not paused.
    let mut rig = self::rig("saved-state\nat 600 halt");
    rig.run(100);
    assert_eq!(rig.player.condition(), Condition::Halted);
    rig.command(Command::SaveState);
    assert_eq!(rig.player.message(), "State saved");
    rig.command(Command::PowerCycle);
    rig.run(5);
    rig.command(Command::Pause);
    assert!(rig.player.paused());
    rig.command(Command::LoadState);
    assert_eq!(rig.player.message(), "State loaded");
    assert_eq!(rig.player.condition(), Condition::Halted);
    assert!(!rig.player.paused());
}

#[test]
fn a_save_that_fails_keeps_the_state_saved_before() {
    let mut rig = rig("saved-state");
    rig.run(100);
    rig.command(Command::SaveState);
    let path = quick_state(&rig);
    let first = std::fs::read(&path).unwrap();
    rig.run(200);
    // Something in the way of the temporary file: the save fails before the
    // slot is touched.
    let partial = quick_state::partial_path(rig.player.prepared().states_dir().unwrap());
    std::fs::create_dir(&partial).unwrap();
    rig.command(Command::SaveState);
    let (title, text) = message_box(&rig);
    assert_eq!(title, "Save state");
    assert!(text.starts_with("The state was not saved: "), "{text}");
    assert!(
        text.ends_with("\n\nThe state saved before is kept."),
        "{text}"
    );
    assert_eq!(rig.player.message(), "State not saved");
    assert_eq!(std::fs::read(&path).unwrap(), first);
    rig.player.dialog = None;
    // Out of the way, the next save replaces it.
    std::fs::remove_dir(&partial).unwrap();
    rig.command(Command::SaveState);
    assert_eq!(rig.player.message(), "State saved");
    assert_ne!(std::fs::read(&path).unwrap(), first);
}

#[test]
fn a_machine_without_saved_state_has_neither_item_and_saves_nothing() {
    let mut rig = rig("");
    rig.run(100);
    assert!(!labels(&rig.player.machine_entries())
        .iter()
        .any(|label| label.ends_with(" state")));
    rig.command(Command::SaveState);
    rig.command(Command::LoadState);
    assert!(!rig.player.has_dialog());
    assert!(!rig.player.prepared().states_dir().unwrap().exists());
    // A stopped machine greys both, so neither key acts.
    let mut rig = self::rig("saved-state\nat 600 power-off");
    rig.run(100);
    let entries = rig.player.machine_entries();
    assert_eq!(
        item(&entries, "Save state").why.as_deref(),
        Some("The machine has stopped")
    );
    rig.command(Command::SaveState);
    assert!(!rig.player.has_dialog());
    assert!(!quick_state(&rig).exists());
    // macOS writes the chords with Command.
    rig.player.set_look(true, true);
    let (texts, _) = open_menu(&rig.player, "Machine", None);
    assert!(texts.contains(&"⌘S".to_owned()), "{texts:?}");
    assert!(texts.contains(&"⌘L".to_owned()), "{texts:?}");
}

/// The script's bytes, served by their digest and size.
struct ScriptPlan(Vec<u8>, [u8; 32]);

impl core_launch::ContentPlan for ScriptPlan {
    fn open(&self) -> Result<Box<dyn unemu_core_api::ContentSource>, unemu_core_api::Error> {
        Ok(Box::new(ScriptPlan(self.0.clone(), self.1)))
    }
}

impl unemu_core_api::ContentSource for ScriptPlan {
    fn open(
        &self,
        sha256: &[u8; 32],
        size: u64,
    ) -> Result<unemu_core_api::Content, unemu_core_api::Error> {
        assert_eq!((*sha256, size), (self.1, self.0.len() as u64));
        Ok(unemu_core_api::Content::Bytes(self.0.clone()))
    }
}

/// A storage for a script that keeps no record.
struct NoRecords;

impl core_launch::StoragePlan for NoRecords {
    fn open(&self) -> Result<std::rc::Rc<dyn unemu_core_api::Storage>, unemu_core_api::Error> {
        Ok(std::rc::Rc::new(NoRecords))
    }
}

impl unemu_core_api::Storage for NoRecords {
    fn open(&self, _name: &str) -> Result<Box<dyn unemu_core_api::Record>, unemu_core_api::Error> {
        panic!("the script keeps no record")
    }
}

#[test]
fn a_machine_started_from_parts_has_no_place_for_saved_states_and_is_named_by_its_title() {
    // The same script a file start runs, its config and content handed in as
    // parts, with no states directory.
    let scratch = Scratch::new();
    let script = b"unemu-test-core 1\nsaved-state\n".to_vec();
    let path = scratch.0.join("files").join("machine.utc");
    std::fs::write(&path, &script).unwrap();
    let dirs = HostDirs {
        saves: scratch.0.join("saves"),
        states: scratch.0.join("states"),
    };
    let opened = prepare(&path, &Cores::new(&CORES), None, &dirs)
        .unwrap()
        .into_ready()
        .unwrap();
    let parts = core_launch::Parts {
        core: &TestCore,
        platform: unemu_test_core::PLATFORM,
        title: "Stored machine".to_owned(),
        notes: Vec::new(),
        config: opened.config().clone(),
        content: Box::new(ScriptPlan(script, *opened.sha256().unwrap())),
        storage: Box::new(NoRecords),
        options: Vec::new(),
    };
    let mut player = Player::start(
        Prepared::from_parts(parts),
        PadBindings::defaults(),
        &mut io::sink(),
    )
    .unwrap();
    assert_eq!(player.message(), "Opened Stored machine");
    assert!(!player.offers().saved_state);
    assert!(!labels(&player.machine_entries())
        .iter()
        .any(|label| label.ends_with(" state")));
    player.command(Command::SaveState, Duration::ZERO, &mut io::sink());
    player.command(Command::LoadState, Duration::ZERO, &mut io::sink());
    assert!(!player.has_dialog());
    assert!(!scratch.0.join("states").exists());
    // The file's own start offers them.
    let player = Player::start(opened, PadBindings::defaults(), &mut io::sink()).unwrap();
    assert!(player.offers().saved_state);
}
