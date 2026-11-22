// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests of the desktop's session of one PC: `machine_view::Session` driving
//! a `unemu_pc_core::PcMachine` assembled from a machine config, the way the
//! desktop runs a machine (design/spec.md §9.4,
//! `backlog/docs/design/core-contract.md` §5 and §8.1.10).
//!
//! The session is neutral and may not depend on a PC crate, and the desktop
//! (`window`) is the application, so these tests live in neither: here a
//! change to the session or to the PC core runs them, and a change to the
//! desktop's shell does not. They cover pacing against the host clock, host
//! input at run boundaries (keys, the media and mute chords, the pointer),
//! resets, persistence at Stop, host audio and the host mix levels. The
//! tests of the desktop's own code (its command line, its setup from a
//! machine file, the drive lights) stay with the desktop.
//!
//! The crate has no library code: everything below is built for its tests
//! only. Its layer is `pc` (`scripts/crate-boundaries.sh`).

#[cfg(test)]
use assembly::{AssembledMachine, DeviceId, ObservationPolicy, ObservationRequest, OutputKind};
#[cfg(test)]
use machine_view::Session;
#[cfg(test)]
use std::{
    error::Error,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    time::Duration,
};
#[cfg(test)]
use unemu_core_api::DisplayId;
#[cfg(test)]
use unemu_pc_core::PcMachine;
#[cfg(test)]
use winit::{
    event::{ElementState, MouseButton},
    keyboard::KeyCode,
};

/// The session's pacing, and the AT's tick rate for the tests that pace by it.
#[cfg(test)]
mod pacing {
    pub(crate) use machine_view::pacing::{next_frame, wake, Speed};

    /// The AT's timer input, 14.31818 MHz divided by twelve (IBM Personal
    /// Computer AT Technical Reference, System Board, "System Timers"): the
    /// tick rate every PC describes.
    pub(crate) const PIT: unemu_core_api::Rate = unemu_core_api::Rate::new(105_000_000, 88);

    pub(crate) fn frame_target(
        elapsed: std::time::Duration,
        clock: u64,
        dropped: u128,
        ratio: u32,
    ) -> (u64, u128) {
        machine_view::pacing::frame_target(elapsed, clock, dropped, ratio, PIT)
    }

    /// Elapsed host time as ticks of the AT's timer input.
    pub(crate) fn virtual_ticks(elapsed: std::time::Duration) -> u128 {
        machine_view::pacing::virtual_ticks(elapsed, PIT)
    }
}
#[cfg(test)]
use pacing::{wake, Speed};

// The PC library's worker, which names an image's workspace.
#[cfg(test)]
use pc_library::library_worker;

/// The set-1 codes of a host key: the session's mapping to a keyboard usage,
/// then the PC core's encoder, as the guest receives them.
#[cfg(test)]
fn set1_scancodes(key: KeyCode, state: ElementState) -> Option<Vec<u8>> {
    unemu_pc_core::set1_scancodes(
        machine_view::keys::key_usage(key)?,
        if state == ElementState::Pressed {
            unemu_core_api::KeyState::Pressed
        } else {
            unemu_core_api::KeyState::Released
        },
    )
}

/// A path in the core checkout holding machine files and firmware fixtures.
#[cfg(test)]
fn default_fixture(relative: &str) -> PathBuf {
    pc_harness::test_support::root().join(relative)
}

#[test]
fn fixture_root_probe() {
    if let Some(root) = std::env::var_os("UNEMUPC_SESSION_FIXTURE_PROBE") {
        let path = default_fixture("vendor/probe/fixture.bin");
        assert_eq!(path, PathBuf::from(root).join("vendor/probe/fixture.bin"));
        assert_eq!(std::fs::read(path).unwrap(), b"external session fixture");
    }
}

#[test]
fn session_fixtures_use_the_external_core_checkout() {
    let temp = machine_file::test_support::TempDir::new("session-fixture-root");
    std::fs::create_dir_all(temp.path().join("vendor/probe")).unwrap();
    std::fs::write(
        temp.path().join("vendor/probe/fixture.bin"),
        b"external session fixture",
    )
    .unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fixture_root_probe", "--nocapture"])
        .env("UNEMUPC_FIXTURE_ROOT", temp.path())
        .env("UNEMUPC_SESSION_FIXTURE_PROBE", temp.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

/// The desktop's session of one PC: the contract session over the PC core's
/// machine. The desktop's `Presenter` adds its media dialogs' catalogue and
/// the drive lights beside it; these tests need neither.
#[cfg(test)]
type Presenter = Session<PcMachine>;

/// A session of `config` with the MDA as its one observed card, assembled
/// with `options`.
#[cfg(test)]
fn new_presenter(
    config: &assembly::MachineConfig,
    options: assembly::AssemblyOptions,
    warnings: &mut impl io::Write,
) -> Result<Presenter, Box<dyn Error>> {
    let assembled = machine_file::assemble_with_observation_and_options(
        config,
        &ObservationPolicy {
            routes: vec![ObservationRequest {
                device: DeviceId::Mda,
                output: OutputKind::TextScanout,
                byte_limit: 0,
            }],
            ..ObservationPolicy::default()
        },
        options,
    )?;
    presenter_from_assembled(config, assembled, warnings)
}

/// A session of a machine already assembled from `config`, on its first
/// display.
#[cfg(test)]
fn presenter_from_assembled(
    config: &assembly::MachineConfig,
    assembled: AssembledMachine,
    warnings: &mut impl io::Write,
) -> Result<Presenter, Box<dyn Error>> {
    let machine = PcMachine::from_assembled(config, assembled)?;
    let mut session = Session::new(Box::new(machine), DisplayId(0), warnings)?;
    // The machines here run real firmware, whose POST melody would come out
    // of the host's speakers during a test run: the tests behave as a host
    // without an output device does, as the desktop's own tests do.
    session
        .audio
        .refuse_device("host audio output is not opened in tests");
    Ok(session)
}

/// What the tests read of the PC behind a session.
#[cfg(test)]
trait PcSession {
    /// The assembled PC, for the tests that reach its bus, CPU and drives.
    fn assembled(&mut self) -> &mut AssembledMachine;
    fn assembled_ref(&self) -> &AssembledMachine;
    /// The PC's keyboard FIFO, for the tests that watch what waits for the 8042.
    fn keyboard(&self) -> &unemu_pc_core::HostKeyboard;
    /// Seal the machine's journals and write its warnings once, as Stop does.
    fn finish_persistence(&mut self, warnings: &mut impl io::Write) -> Result<(), Box<dyn Error>>;
    /// The session's reset total, written as the desktop writes it at Stop.
    fn write_reset_summary(&self, diagnostics: &mut impl io::Write);
}

#[cfg(test)]
impl PcSession for Presenter {
    fn assembled(&mut self) -> &mut AssembledMachine {
        self.machine_mut().assembled()
    }

    fn assembled_ref(&self) -> &AssembledMachine {
        self.machine().assembled_ref()
    }

    fn keyboard(&self) -> &unemu_pc_core::HostKeyboard {
        self.machine().keyboard()
    }

    fn finish_persistence(&mut self, warnings: &mut impl io::Write) -> Result<(), Box<dyn Error>> {
        self.finish(warnings)
    }

    fn write_reset_summary(&self, diagnostics: &mut impl io::Write) {
        if let Some(summary) = self.machine().reset_summary() {
            let _ = writeln!(diagnostics, "{summary}");
        }
    }
}

#[cfg(test)]
mod host_audio;
#[cfg(test)]
mod key_table_tests;
#[cfg(test)]
mod mouse;
#[cfg(test)]
mod sound_levels_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod launch_tests;
