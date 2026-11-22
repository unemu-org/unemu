// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's command vocabulary: every command a menu, a toolbar, a
//! hotkey or a view pushes, the session state it is checked against, and
//! which commands the state allows. The shell carries them out; see
//! `backlog/docs/design/machine-images.md` §7 and `backlog/docs/architecture.md` § Observation and presentation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Stopped,
    Running,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Start,
    /// File › Open…: a file, run by whichever compiled-in core opens it.
    Open,
    GamingMode,
    Layers,
    ImportImage,
    ImportFirmware,
    /// File › Edit metadata… on the selected firmware package.
    EditFirmware,
    /// File › Import installation media….
    ImportMedia,
    DeleteTag,
    StoreFolder,
    Pause,
    Stop,
    Reset,
    PowerCycle,
    /// Insert… of a floppy drive: the media picker.
    Insert(u8),
    /// Create from files… of a floppy drive: the media picker in its Create
    /// view.
    CreateFromFiles(u8),
    /// Browse for an image file… of the media picker: a disk image file on
    /// this computer, or a disk of the drive's catalogue, with its write mode.
    InsertFile(u8),
    /// Insert of the set selected in the library window: the media picker
    /// with that set chosen.
    InsertSet(u8),
    /// Change the write mode of the floppy in a drive, in its Insert dialog.
    WriteMode(u8),
    CreateBlank,
    Next(u8),
    /// A disk of a floppy drive's Catalogue submenu, by its catalogue index:
    /// what Insert of the media picker's This machine does with it.
    CatalogueDisk(u8, usize),
    /// All N disks… of a drive's Catalogue submenu: the media picker on This
    /// machine, with the disk in the drive chosen.
    CatalogueAll(u8),
    Eject(u8),
    Mute,
    Speed(machine_view::pacing::Speed),
    FastForward,
    Release,
    Fullscreen,
    Library,
    Exit,
    Minimize,
    Shortcuts,
    About,
    Placeholder(&'static str),
    /// Save state (Ctrl+Alt+S) and Load state (Ctrl+Alt+L): a machine
    /// started from a file whose core saves state has them; the PC has
    /// none, so they stay disabled while it runs.
    SaveState,
    LoadState,
    FirmwareFolders,
    Preferences,
    Settings,
    /// Machine › Save as new image…: the Disk layers save form.
    SaveImage,
    /// Image › Push to…, the toolbar's Push… in the library and the Push
    /// to… buttons: the selected library items, to a registry.
    Push,
    /// Image › Push here…, the toolbar's Push… on a Remotes node and the
    /// Push here… button of a node's pane: library items, to the selected
    /// registry or namespace.
    PushHere,
    /// Image › Pull… and the toolbar's Pull…: the tags selected in the
    /// library's Remotes, into the local store.
    Pull,
    /// Image › Pull by reference…: a registry image named by its reference.
    PullReference,
    /// File › Get templates…: Remotes › the default registry › `unemu/` ›
    /// Machine images.
    GetTemplates,
    /// View › Refresh remote (F5) and the toolbar's Refresh: list the shown
    /// registry again.
    RefreshRemote,
    /// Rename… of the selected machine (F2).
    Rename,
    /// Duplicate… of the selected machine (Ctrl+D).
    Duplicate,
    /// Delete machine… of the selected machine.
    DeleteMachine,
    /// The Delete key: Delete machine… on a machine, Delete tag… on an image.
    DeleteSelection,
    /// Reset to image… of the selected machine.
    ResetMachine,
    /// Machine › Show source image.
    ShowSource,
    /// New machine from image… (Ctrl+Shift+N).
    NewFromImage,
    /// Image › Start…: the selected image, or the selected machine's source.
    ImageStart,
    /// Image › Gaming mode: a disposable copy of the image itself.
    ImageGaming,
    /// Image › Settings…: a new revision of the image.
    ImageSettings,
    /// Image › Layers…: the image's own chains.
    ImageLayers,
}

impl Command {
    /// The library's own keys, which an edited text field keeps for itself.
    pub fn library_key(self) -> bool {
        matches!(
            self,
            Command::Rename
                | Command::Duplicate
                | Command::DeleteSelection
                | Command::NewFromImage
                | Command::Start
        )
    }
}

#[derive(Clone, Copy)]
pub struct Enablement {
    pub state: State,
    pub ready: bool,
    /// Drives that take a floppy now: those of the session, or while stopped,
    /// those of the selected machine, whose floppies Start puts in.
    pub floppy: [bool; 2],
    pub audio: bool,
}

impl Enablement {
    pub fn allows(self, command: Command) -> bool {
        match command {
            Command::Start | Command::GamingMode | Command::ImageStart | Command::ImageGaming => {
                self.state == State::Stopped && self.ready
            }
            Command::Duplicate
            | Command::DeleteMachine
            | Command::DeleteSelection
            | Command::ResetMachine
            | Command::ShowSource
            | Command::NewFromImage => self.state == State::Stopped,
            Command::Pause
            | Command::Stop
            | Command::Reset
            | Command::PowerCycle
            | Command::Release
            | Command::Speed(_)
            | Command::FastForward => self.state != State::Stopped,
            Command::Insert(unit)
            | Command::CreateFromFiles(unit)
            | Command::InsertFile(unit)
            | Command::CatalogueDisk(unit, _)
            | Command::CatalogueAll(unit)
            | Command::Eject(unit) => self.floppy.get(unit as usize).copied().unwrap_or(false),
            Command::InsertSet(unit) | Command::WriteMode(unit) | Command::Next(unit) => {
                self.state != State::Stopped
                    && self.floppy.get(unit as usize).copied().unwrap_or(false)
            }
            Command::Mute => self.state != State::Stopped && self.audio,
            Command::SaveState | Command::LoadState => false,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enablement_requires_ready_firmware_and_live_capabilities() {
        for state in [State::Stopped, State::Running, State::Paused] {
            for ready in [false, true] {
                let model = Enablement {
                    state,
                    ready,
                    floppy: [true, false],
                    audio: true,
                };
                assert_eq!(
                    model.allows(Command::Start),
                    state == State::Stopped && ready
                );
                for command in [
                    Command::Pause,
                    Command::Stop,
                    Command::Reset,
                    Command::PowerCycle,
                    Command::InsertSet(0),
                    Command::WriteMode(0),
                    Command::Next(0),
                    Command::Mute,
                ] {
                    assert_eq!(model.allows(command), state != State::Stopped);
                }
                // A stopped machine's drives take floppies for Start.
                for command in [
                    Command::Insert(0),
                    Command::InsertFile(0),
                    Command::Eject(0),
                ] {
                    assert!(model.allows(command), "{state:?} {command:?}");
                }
                assert!(!model.allows(Command::Eject(1)));
                assert!(!model.allows(Command::Insert(1)));
                assert!(!model.allows(Command::InsertFile(1)));
                assert!(!model.allows(Command::WriteMode(1)));
                assert!(!model.allows(Command::Insert(2)));
                assert!(!model.allows(Command::SaveState));
                assert!(!model.allows(Command::LoadState));
            }
        }
        assert!(!Enablement {
            state: State::Running,
            ready: true,
            floppy: [false; 2],
            audio: false
        }
        .allows(Command::Mute));
    }
}
