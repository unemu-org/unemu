// SPDX-License-Identifier: AGPL-3.0-or-later
//! The macOS application menu and system quit requests, as plain data.
//!
//! The menu bar stays inside the window on every platform; the macOS global
//! menu keeps only the application menu (`backlog/docs/design/machine-images.md`
//! §7). Its About and Quit items run the shell's own About and Exit commands, so
//! Quit asks before stopping a running machine exactly like File › Exit and never
//! terminates the process behind the shell's back. Hide, Hide Others and Show All
//! are the standard actions of AppKit's NSApplication (`hide:`,
//! `hideOtherApplications:`, `unhideAllApplications:`).
//!
//! A quit that the system starts (Dock › Quit, logout, restart, shutdown) takes
//! the same Exit path. AppKit asks the application delegate
//! `applicationShouldTerminate:`; the native layer answers "terminate later"
//! (AppKit, NSApplicationDelegate, NSTerminateLater), the shell's next pass
//! runs Exit, and [`NativeRequests::settle`] says when to answer with NSApplication's
//! `replyToApplicationShouldTerminate:`: YES once Stop machine and persistence
//! have finished, NO when the user cancels. The AppKit calls themselves live in
//! the macOS-only `macos_menu` module; this module is what the tests check on
//! every host.

use crate::shell_model::Command;
#[cfg(any(target_os = "macos", test))]
use std::ffi::CStr;

/// The application name the menu shows.
#[cfg(any(target_os = "macos", test))]
pub(crate) const APP_NAME: &str = "unemupc";

/// What choosing a menu item does.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuAction {
    /// The shell runs this command, as if chosen from its own menus.
    Shell(Command),
    /// The standard NSApplication action with this selector.
    Application(&'static CStr),
}

/// A key equivalent: the character together with Command, plus Option when set.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyEquivalent {
    pub key: &'static str,
    pub option: bool,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuEntry {
    Item {
        title: &'static str,
        key: Option<KeyEquivalent>,
        action: MenuAction,
    },
    Separator,
}

/// The application menu, top to bottom.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn application_menu() -> [MenuEntry; 7] {
    use MenuEntry::{Item, Separator};
    let command = |key| Some(KeyEquivalent { key, option: false });
    [
        Item {
            title: "About unemupc",
            key: None,
            action: MenuAction::Shell(Command::About),
        },
        Separator,
        Item {
            title: "Hide unemupc",
            key: command("h"),
            action: MenuAction::Application(c"hide:"),
        },
        Item {
            title: "Hide Others",
            key: Some(KeyEquivalent {
                key: "h",
                option: true,
            }),
            action: MenuAction::Application(c"hideOtherApplications:"),
        },
        Item {
            title: "Show All",
            key: None,
            action: MenuAction::Application(c"unhideAllApplications:"),
        },
        Separator,
        Item {
            title: "Quit unemupc",
            key: command("q"),
            action: MenuAction::Shell(Command::Exit),
        },
    ]
}

/// A request from the native application layer to the shell.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeRequest {
    /// A menu item that runs a shell command.
    Command(Command),
    /// AppKit asks whether the application may terminate.
    SystemQuit,
}

/// How far the shell's Exit has come at the end of a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExitProgress {
    /// No exit is under way: none was asked for, or the user cancelled it.
    Idle,
    /// The stop prompt asks whether to stop the running machine.
    Asking,
    /// The machine has stopped; another answer (Gaming mode's keep or discard)
    /// still holds the exit.
    Waiting,
    /// Stop and persistence have finished; the window closes now.
    Closing,
}

/// What the shell does next about a system quit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuitStep {
    /// Nothing this pass.
    Wait,
    /// The shell has just started asking (the stop prompt) or waiting for an
    /// answer (Gaming mode's keep or discard) for the system quit; bring the
    /// window forward so the user can answer even when it was hidden or
    /// minimized.
    ShowPrompt,
    /// Answer AppKit: terminate (true) or keep running (false).
    Reply(bool),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SystemQuit {
    #[default]
    None,
    /// AppKit has asked; the shell has not run its Exit yet.
    Requested,
    Pending {
        prompted: bool,
    },
    /// AppKit was told to terminate; it closes the window and ends the
    /// process itself.
    Terminating,
}

/// Menu commands queued for the next shell pass, and the state of a system quit.
#[derive(Default)]
pub(crate) struct NativeRequests {
    commands: Vec<Command>,
    quit: SystemQuit,
    /// Sees each reply to AppKit at the moment it is given.
    #[cfg(test)]
    pub(crate) on_reply: Option<Box<dyn FnMut(bool)>>,
}

impl NativeRequests {
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn push(&mut self, request: NativeRequest) {
        match request {
            NativeRequest::Command(command) => self.commands.push(command),
            // A repeated request joins the one already being answered.
            NativeRequest::SystemQuit => {
                if self.quit == SystemQuit::None {
                    self.quit = SystemQuit::Requested;
                }
            }
        }
    }

    /// The menu commands chosen since the last pass.
    pub(crate) fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    /// Whether AppKit has asked to terminate since the last pass; the shell
    /// then runs its Exit.
    pub(crate) fn take_system_quit(&mut self) -> bool {
        let requested = self.quit == SystemQuit::Requested;
        if requested {
            self.quit = SystemQuit::Pending { prompted: false };
        }
        requested
    }

    pub(crate) fn terminating(&self) -> bool {
        self.quit == SystemQuit::Terminating
    }

    /// Whether the shell's current Exit is answering a system quit.
    pub(crate) fn system_quit_pending(&self) -> bool {
        matches!(self.quit, SystemQuit::Pending { .. })
    }

    /// The step a system quit takes once the shell has reached `exit`. A quit
    /// the shell has not taken yet waits, so it is never answered before its
    /// Exit has run.
    pub(crate) fn settle(&mut self, exit: ExitProgress) -> QuitStep {
        let SystemQuit::Pending { prompted } = self.quit else {
            return QuitStep::Wait;
        };
        let step = match exit {
            ExitProgress::Asking | ExitProgress::Waiting if !prompted => {
                self.quit = SystemQuit::Pending { prompted: true };
                QuitStep::ShowPrompt
            }
            ExitProgress::Asking | ExitProgress::Waiting => QuitStep::Wait,
            ExitProgress::Closing => {
                self.quit = SystemQuit::Terminating;
                QuitStep::Reply(true)
            }
            ExitProgress::Idle => {
                self.quit = SystemQuit::None;
                QuitStep::Reply(false)
            }
        };
        #[cfg(test)]
        if let (QuitStep::Reply(terminate), Some(on_reply)) = (step, &mut self.on_reply) {
            on_reply(terminate);
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_menu_lists_the_standard_items_in_order() {
        use MenuEntry::{Item, Separator};
        let menu = application_menu();
        let titles: Vec<_> = menu
            .iter()
            .map(|entry| match entry {
                Item { title, .. } => *title,
                Separator => "-",
            })
            .collect();
        assert_eq!(
            titles,
            [
                "About unemupc",
                "-",
                "Hide unemupc",
                "Hide Others",
                "Show All",
                "-",
                "Quit unemupc"
            ]
        );
        for title in ["About ", "Hide ", "Quit "] {
            assert!(titles.contains(&format!("{title}{APP_NAME}").as_str()));
        }
        let keys: Vec<_> = menu
            .iter()
            .filter_map(|entry| match entry {
                Item { key, .. } => Some(key.map(|key| (key.key, key.option))),
                Separator => None,
            })
            .collect();
        assert_eq!(
            keys,
            [
                None,
                Some(("h", false)),
                Some(("h", true)),
                None,
                Some(("q", false))
            ]
        );
    }

    #[test]
    fn about_and_quit_send_shell_commands_and_hide_items_use_appkit_actions() {
        let actions: Vec<_> = application_menu()
            .into_iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item { action, .. } => Some(action),
                MenuEntry::Separator => None,
            })
            .collect();
        assert_eq!(
            actions,
            [
                MenuAction::Shell(Command::About),
                MenuAction::Application(c"hide:"),
                MenuAction::Application(c"hideOtherApplications:"),
                MenuAction::Application(c"unhideAllApplications:"),
                MenuAction::Shell(Command::Exit),
            ]
        );
        // Quit goes through the shell's Exit, never straight to `terminate:`.
        assert!(!actions.contains(&MenuAction::Application(c"terminate:")));
    }

    #[test]
    fn menu_commands_queue_for_the_next_pass() {
        let mut native = NativeRequests::default();
        native.push(NativeRequest::Command(Command::About));
        native.push(NativeRequest::Command(Command::Exit));
        assert_eq!(native.take_commands(), [Command::About, Command::Exit]);
        assert!(native.take_commands().is_empty());
        // A plain Quit from the menu owes AppKit nothing.
        assert!(!native.take_system_quit());
        assert_eq!(native.settle(ExitProgress::Closing), QuitStep::Wait);
        assert!(!native.terminating());
    }

    #[test]
    fn system_quit_with_nothing_to_ask_replies_terminate_once_closing() {
        let mut native = NativeRequests::default();
        native.push(NativeRequest::SystemQuit);
        native.push(NativeRequest::SystemQuit);
        assert!(native.take_commands().is_empty());
        // Nothing is answered before the shell has taken the request.
        assert_eq!(native.settle(ExitProgress::Idle), QuitStep::Wait);
        assert!(native.take_system_quit());
        assert!(!native.take_system_quit());
        assert_eq!(native.settle(ExitProgress::Closing), QuitStep::Reply(true));
        assert!(native.terminating());
        assert_eq!(native.settle(ExitProgress::Closing), QuitStep::Wait);
        assert_eq!(native.settle(ExitProgress::Idle), QuitStep::Wait);
    }

    #[test]
    fn system_quit_waits_for_the_prompt_and_cancel_replies_keep_running() {
        let mut native = NativeRequests::default();
        native.push(NativeRequest::SystemQuit);
        assert!(native.take_system_quit());
        assert_eq!(native.settle(ExitProgress::Asking), QuitStep::ShowPrompt);
        assert_eq!(native.settle(ExitProgress::Asking), QuitStep::Wait);
        assert_eq!(native.settle(ExitProgress::Idle), QuitStep::Reply(false));
        assert!(!native.terminating());
        assert_eq!(native.settle(ExitProgress::Idle), QuitStep::Wait);
        // The next system quit starts afresh.
        native.push(NativeRequest::SystemQuit);
        assert!(native.take_system_quit());
        assert!(native.system_quit_pending());
        assert_eq!(native.settle(ExitProgress::Asking), QuitStep::ShowPrompt);
        assert_eq!(native.settle(ExitProgress::Waiting), QuitStep::Wait);
        assert_eq!(native.settle(ExitProgress::Closing), QuitStep::Reply(true));
        assert!(!native.system_quit_pending());
    }

    #[test]
    fn system_quit_waiting_for_another_answer_also_comes_forward_once() {
        let mut native = NativeRequests::default();
        assert!(!native.system_quit_pending());
        native.push(NativeRequest::SystemQuit);
        assert!(native.take_system_quit());
        assert_eq!(native.settle(ExitProgress::Waiting), QuitStep::ShowPrompt);
        assert_eq!(native.settle(ExitProgress::Waiting), QuitStep::Wait);
        assert_eq!(native.settle(ExitProgress::Asking), QuitStep::Wait);
        assert_eq!(native.settle(ExitProgress::Closing), QuitStep::Reply(true));
    }
}
