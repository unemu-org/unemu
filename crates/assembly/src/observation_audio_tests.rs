// SPDX-License-Identifier: AGPL-3.0-or-later
//! Audio facade tests at machine service boundaries, including halted PIT time.
//! See `backlog/docs/design/audio.md` §Validation.

use std::rc::Rc;

use audio::{NullAudioSink, SampleArea};
use device::VClock;

use super::*;
use crate::{assemble_with_observation, RunPolicy};

struct Constant;

impl AudioSource for Constant {
    fn render(&mut self, start: VClock, end: VClock) -> SampleArea {
        [
            321 * i128::from(end.0 - start.0),
            -123 * i128::from(end.0 - start.0),
        ]
    }
}

fn policy() -> ObservationPolicy {
    ObservationPolicy {
        output: OutputConfig {
            audio_sink: AudioSinkKind::Capture,
            frame_sink: FrameSinkKind::Capture,
            audio_capture_frame_limit: 10_000,
        },
        routes: vec![ObservationRequest {
            device: DeviceId::Vga,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..Default::default()
    }
}

#[test]
fn audio_source_free_machine_keeps_mixer_absent_and_capture_empty() {
    assert_eq!(OutputConfig::default().audio_sink, AudioSinkKind::None);
    let config = crate::at_vga_text(vec![0xEB, 0xFE]);
    for selection in [ObservationPolicy::default(), policy()] {
        let mut rig = assemble_with_observation(&config, &selection).unwrap();
        rig.run_session(&RunPolicy::natural(Some(5000), None))
            .unwrap();
        rig.capture_audio().unwrap();
        assert!(rig.observation.audio.borrow().mixer.is_none());
        if selection.output.audio_sink == AudioSinkKind::None {
            assert!(rig.observation.captured_audio().is_none());
        } else {
            assert_eq!(rig.observation.captured_audio().unwrap().frame_count(), 0);
        }
        assert_eq!(rig.machine.clock(), VClock(5000));
    }
}

#[test]
fn speaker_device_error_surfaces_at_next_observation_even_without_output() {
    for selection in [ObservationPolicy::default(), policy()] {
        let config = crate::at_vga_text(vec![0xEB, 0xFE]);
        let mut rig = assemble_with_observation(&config, &selection).unwrap();
        let edges = audio::EdgeSource::new([0; 2]);
        let writer = edges.clone();
        let pit = devices::pit8253::SharedPit8253::default();
        let sys = devices::SysControl::new().with_pit(pit.clone());
        pit.observe_speaker(move |at, high| {
            writer.record_level(at, [if high { 16384 } else { 0 }; 2])
        });
        let mut sys = testkit::MockBus::new(Box::new(sys));
        rig.add_audio_source(Box::new(edges.clone()));
        rig.run_session(&RunPolicy::natural(Some(100), None))
            .unwrap();
        sys.port_write(0x61, 1, 2).unwrap();
        assert_eq!(rig.capture_audio(), Err(AudioError::HistoryOrder));
        sys.tick(100);
        sys.port_write(0x61, 1, 0).unwrap();
        rig.capture_audio().unwrap();
        assert!(edges.pending_edges() <= 1);
    }
}

#[test]
fn audio_machine_capture_is_split_independent_and_preserves_display_and_cpu() {
    let config = crate::at_vga_text(vec![0xEB, 0xFE]);
    let mut whole = assemble_with_observation(&config, &policy()).unwrap();
    let mut split = assemble_with_observation(&config, &policy()).unwrap();
    let mut silent = assemble_with_observation(&config, &policy()).unwrap();
    whole.add_audio_source(Box::new(Constant));
    split.add_audio_source(Box::new(Constant));
    whole
        .run_session(&RunPolicy::natural(Some(5000), None))
        .unwrap();
    silent
        .run_session(&RunPolicy::natural(Some(5000), None))
        .unwrap();
    for cap in [1, 24, 25, 26, 49, 50, 1000, 5000] {
        split
            .run_session(&RunPolicy::natural(Some(cap), None))
            .unwrap();
    }
    let captured = whole.observation.captured_audio().unwrap().clone();
    assert_eq!(captured.frame_count(), 201);
    assert!(captured.frames().iter().all(|frame| *frame == [321, -123]));
    assert_eq!(
        Some(captured.clone()),
        split
            .observation
            .captured_audio()
            .map(|capture| capture.clone())
    );
    whole.capture_audio().unwrap();
    assert_eq!(
        Some(captured),
        whole
            .observation
            .captured_audio()
            .map(|capture| capture.clone())
    );
    for rig in [&whole, &split] {
        assert_eq!(rig.machine.clock(), silent.machine.clock());
        assert_eq!(rig.machine.virtual_time(), silent.machine.virtual_time());
        assert_eq!(rig.machine.cpu().retired(), silent.machine.cpu().retired());
        assert_eq!(
            rig.capture_text(DeviceId::Vga).unwrap(),
            silent.capture_text(DeviceId::Vga).unwrap()
        );
    }
}

#[test]
fn audio_halted_time_uses_pit_clock_at_each_cpu_rate() {
    for (numerator, denominator) in [(1, 1), (5, 2), (2, 3)] {
        let mut config = crate::at_vga_text(vec![0xFB, 0xF4]);
        config.clock.retired_per_pit_clock = numerator;
        config.clock.retired_per_pit_clock_denominator = denominator;
        let mut rig = assemble_with_observation(&config, &policy()).unwrap();
        rig.add_audio_source(Box::new(Constant));
        rig.run_session(&RunPolicy::natural(Some(10_000), Some(2000)))
            .unwrap();
        assert_eq!(rig.machine.cpu().retired(), 2);
        assert_eq!(rig.machine.clock().0, 2000 + 2 * denominator / numerator);
        assert_eq!(
            rig.observation.captured_audio().unwrap().frame_count(),
            audio::completed_frames(rig.machine.clock())
        );
    }
}

#[test]
fn audio_fanout_and_reconfiguration_preserve_stream_position_and_subscriptions() {
    struct Recorder(Rc<RefCell<CaptureAudioSink>>);
    impl AudioSink for Recorder {
        fn present(&mut self, block: AudioBlock<'_>) {
            self.0.borrow_mut().present(block);
        }
    }
    let mut machine = crate::assemble(&crate::at_vga_text(vec![0xEB, 0xFE]))
        .unwrap()
        .machine;
    machine.run_bounded(Some(125)).unwrap();
    let mut observation =
        Observation::bind(&policy_without_displays(), &HashMap::new(), &mut []).unwrap();
    observation.add_audio_source(VClock::ZERO, Box::new(Constant));
    let external = Rc::new(RefCell::new(CaptureAudioSink::with_frame_limit(10_000)));
    observation.subscribe_audio(Box::new(Recorder(external.clone())));
    observation.subscribe_audio(Box::new(NullAudioSink));
    observation.capture_audio(&mut machine, VClock(50)).unwrap();
    assert_eq!(*observation.captured_audio().unwrap(), *external.borrow());
    observation.configure_output(OutputConfig::default());
    observation.capture_audio(&mut machine, VClock(75)).unwrap();
    assert!(observation.captured_audio().is_none());
    observation.configure_output(policy().output);
    observation
        .capture_audio(&mut machine, VClock(100))
        .unwrap();
    assert_eq!(
        observation.captured_audio().unwrap().frames(),
        [[321, -123]]
    );
    assert_eq!(external.borrow().frame_count(), 4);
    assert_eq!(
        observation.capture_audio(&mut machine, VClock(99)),
        Err(AudioError::TimeReversed)
    );
    observation
        .capture_audio(&mut machine, VClock(125))
        .unwrap();
    assert_eq!(observation.captured_audio().unwrap().frame_count(), 2);
}

fn policy_without_displays() -> ObservationPolicy {
    ObservationPolicy {
        routes: Vec::new(),
        ..policy()
    }
}

#[test]
fn audio_disabled_output_does_not_read_source_history() {
    struct Unread;
    impl AudioSource for Unread {
        fn render(&mut self, _: VClock, _: VClock) -> SampleArea {
            panic!("unobserved audio was sampled");
        }
    }
    let mut machine = crate::assemble(&crate::at_vga_text(vec![0xEB, 0xFE]))
        .unwrap()
        .machine;
    machine.run_bounded(Some(125)).unwrap();
    let mut observation = Observation::default();
    observation.add_audio_source(VClock::ZERO, Box::new(Unread));
    observation.subscribe_audio(Box::new(NullAudioSink));
    observation
        .capture_audio(&mut machine, VClock(125))
        .unwrap();
    assert!(observation.captured_audio().is_none());
}

struct HistoryDevice {
    edges: audio::EdgeSource,
    now: Rc<std::cell::Cell<u64>>,
    journal: Option<Box<dyn device::SectorMedia>>,
}

impl Device for HistoryDevice {
    fn name(&self) -> &'static str {
        "synthetic-audio-history"
    }
    fn port_read(&mut self, _: u16, _: u8) -> Result<u32, device::DeviceError> {
        Ok(0)
    }
    fn port_write(&mut self, _: u16, _: u8, _: u32) -> Result<(), device::DeviceError> {
        Ok(())
    }
    fn mmio_read(&mut self, _: u64, _: u8) -> Result<u64, device::DeviceError> {
        Ok(0)
    }
    fn mmio_write(&mut self, _: u64, _: u8, _: u64) -> Result<(), device::DeviceError> {
        Ok(())
    }
    fn tick(&mut self, now: VClock) {
        if self.now.get() < 50 && now.0 >= 50 {
            self.edges.push(VClock(50), [2000; 2]).unwrap();
        }
        self.now.set(now.0);
        if let Some(mut journal) = self.journal.take() {
            journal.write_batch(&[device::SectorWrite {
                lba: 0,
                bytes: [0xA5; 512],
            }]);
        }
    }
    fn next_event(&self, _: VClock) -> device::NextEvent {
        if self.journal.is_some() {
            device::NextEvent::EveryPitClock
        } else {
            device::NextEvent::Dormant
        }
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn reset(&mut self) {}
    fn save(&self) -> Vec<u8> {
        self.now.get().to_le_bytes().to_vec()
    }
    fn restore(&mut self, bytes: &[u8]) -> Result<(), device::DeviceError> {
        let bytes: [u8; 8] =
            bytes
                .try_into()
                .map_err(|_| device::DeviceError::InvalidSnapshot {
                    device: "synthetic-audio-history",
                    reason: "clock length",
                })?;
        self.now.set(u64::from_le_bytes(bytes));
        Ok(())
    }
}

fn history_machine(
    program: &[u8],
    journal: Option<Box<dyn device::SectorMedia>>,
) -> (
    ::machine::Machine<cpu::X86JitCpu>,
    audio::EdgeSource,
    Rc<std::cell::Cell<u64>>,
) {
    let edges = audio::EdgeSource::new([1000; 2]);
    let now = Rc::new(std::cell::Cell::new(0));
    let mut cpu =
        cpu::X86JitCpu::new_real16_bare(0x10000, 0, 0, cpu::x86jit_backend::RealSegments::flat(0))
            .unwrap();
    cpu.map_ram(0, 0x10000).unwrap();
    cpu.write_mem(0, program).unwrap();
    let bus = bus::Bus::new(
        vec![Box::new(HistoryDevice {
            edges: edges.clone(),
            now: now.clone(),
            journal,
        })],
        bus::DecodeMap::default(),
    );
    (
        ::machine::Machine::new(
            cpu,
            bus,
            Box::new(chipset::NoInterruptController),
            Vec::new(),
            None,
        ),
        edges,
        now,
    )
}

#[test]
fn dormant_audio_device_records_history_on_mock_bus_and_settles_before_capture() {
    let edges = audio::EdgeSource::new([1000; 2]);
    let now = Rc::new(std::cell::Cell::new(0));
    let mut bus = testkit::MockBus::new(Box::new(HistoryDevice {
        edges: edges.clone(),
        now: now.clone(),
        journal: None,
    }));
    bus.tick(49);
    assert_eq!(edges.pending_edges(), 0);
    bus.tick(51);
    assert_eq!(now.get(), 100);
    assert_eq!(edges.pending_edges(), 1);
    let (mut machine, edges, now) = history_machine(&[0xEB, 0xFE], None);
    let mut observation =
        Observation::bind(&policy_without_displays(), &HashMap::new(), &mut []).unwrap();
    observation.add_audio_source(VClock::ZERO, Box::new(edges));
    machine.run_bounded(Some(100)).unwrap();
    assert_eq!(now.get(), 0);
    observation
        .capture_audio(&mut machine, VClock(100))
        .unwrap();
    assert_eq!(now.get(), 100);
    assert_eq!(
        observation.captured_audio().unwrap().frames(),
        [[1000; 2], [1000; 2], [2000; 2], [2000; 2]]
    );
}

#[test]
fn future_audio_boundary_is_rejected_before_device_sync_or_source_access() {
    let (mut machine, edges, now) = history_machine(&[0xEB, 0xFE], None);
    let mut observation = Observation::default();
    observation.add_audio_source(VClock::ZERO, Box::new(edges));
    machine.run_bounded(Some(100)).unwrap();
    assert_eq!(
        observation.capture_audio(&mut machine, VClock(101)),
        Err(AudioError::FutureTime)
    );
    assert_eq!(now.get(), 0);
    observation
        .capture_audio(&mut machine, VClock(100))
        .unwrap();
    assert_eq!(now.get(), 100);
}

#[test]
fn second_audio_source_added_through_facade_weights_its_first_partial_interval() {
    let mut rig =
        assemble_with_observation(&crate::at_vga_text(vec![0xEB, 0xFE]), &policy()).unwrap();
    rig.add_audio_source(Box::new(Constant));
    rig.run_session(&RunPolicy::natural(Some(37), None))
        .unwrap();
    let retained = rig.observation.captured_audio().unwrap().frames().as_ptr();
    assert_eq!(
        retained,
        rig.observation.captured_audio().unwrap().frames().as_ptr()
    );
    rig.add_audio_source(Box::new(Constant));
    rig.run_session(&RunPolicy::natural(Some(75), None))
        .unwrap();
    assert_eq!(
        rig.observation.captured_audio().unwrap().frames(),
        [[321, -123], [487, -186], [642, -246]]
    );
}

#[test]
fn machine_error_has_priority_over_audio_rewind_error() {
    let mut rig =
        assemble_with_observation(&crate::at_vga_text(vec![0xEB, 0xFE]), &policy()).unwrap();
    rig.machine = history_machine(&[0xEA, 0, 0, 0, 0x30], None).0;
    rig.observation.audio.borrow_mut().mixer = Some(Mixer::new(VClock(1000)));
    let result = rig.run_session(&RunPolicy::natural(Some(25), None));
    assert!(
        matches!(result, Err(crate::RunError::Machine(_))),
        "{result:?}"
    );
    assert_eq!(rig.observation.captured_audio().unwrap().frame_count(), 0);
}

#[test]
fn persistence_failure_is_checked_before_audio_rewind_error() {
    let temp = crate::test_support::TempDir::new("audio-boundary-journal");
    let (mut handle, media) = crate::PersistenceHandle::mount(
        0,
        &temp.path().join("audio.upfj"),
        &[0; 512],
        512,
        0,
        None,
    )
    .unwrap();
    handle.fail_output();
    let (mut machine, _, _) = history_machine(&[0xEB, 0xFE], Some(media));
    let observation =
        Observation::bind(&policy_without_displays(), &HashMap::new(), &mut []).unwrap();
    observation.audio.borrow_mut().mixer = Some(Mixer::new(VClock(1000)));
    assert!(matches!(
        crate::run_with_persistence(
            &mut machine,
            &observation,
            &RunPolicy::natural(Some(25), None),
            &[handle]
        ),
        Err(crate::RunError::Persistence(_))
    ));
    assert_eq!(observation.captured_audio().unwrap().frame_count(), 0);
    assert_eq!(machine.clock(), VClock(25));
}

#[test]
fn speaker_history_stays_bounded_without_capture_on_direct_and_session_runs() {
    for direct in [true, false] {
        let pit = devices::pit8253::SharedPit8253::default();
        let sys = devices::SysControl::new().with_pit(pit.clone());
        let edges = audio::EdgeSource::new([0; 2]);
        let writer = edges.clone();
        pit.observe_speaker(move |at, high| {
            writer.record_level(at, [if high { 16384 } else { 0 }; 2]);
        });
        let mut cpu = cpu::X86JitCpu::new_real16_bare(
            0x10000,
            0,
            0,
            cpu::x86jit_backend::RealSegments::flat(0),
        )
        .unwrap();
        cpu.map_ram(0, 0x10000).unwrap();
        cpu.write_mem(0, &[0xB0, 2, 0xE6, 0x61, 0x34, 2, 0xEB, 0xFA])
            .unwrap();
        let mut devices: Vec<Box<dyn Device>> = vec![Box::new(pit), Box::new(sys)];
        let index = HashMap::from([(DeviceId::Pit, 0), (DeviceId::SysControl, 1)]);
        let mut observation =
            Observation::bind(&ObservationPolicy::default(), &index, &mut devices).unwrap();
        observation.add_audio_source(VClock::ZERO, Box::new(edges.clone()));
        let mut bus = bus::Bus::new(
            devices,
            bus::DecodeMap {
                ports: vec![
                    bus::PortRange {
                        ports: 0x40..=0x43,
                        device: 0,
                        width: bus::PortWidth::Byte,
                    },
                    bus::PortRange {
                        ports: 0x61..=0x61,
                        device: 1,
                        width: bus::PortWidth::Byte,
                    },
                ],
                mmio: Vec::new(),
            },
        );
        bus.add_access_coupling(1, 0).unwrap();
        let machine = ::machine::Machine::new(
            cpu,
            bus,
            Box::new(chipset::NoInterruptController),
            Vec::new(),
            None,
        )
        .with_cpu_rate(
            std::num::NonZeroU64::new(100).unwrap(),
            std::num::NonZeroU64::new(1).unwrap(),
        );
        let mut rig = crate::AssembledMachine {
            floppy_names: Default::default(),
            machine,
            observation,
            device_index: index,
            mouse_target: None,
            a20_line: Default::default(),
            persistence: Vec::new(),
            unlisted_firmware: Vec::new(),
            option_roms: Vec::new(),
        };
        for cap in (1..=10).map(|chunk| chunk * 100_000) {
            if direct {
                rig.machine.run_bounded(Some(cap)).unwrap();
            } else {
                rig.run_session(&RunPolicy::natural(Some(cap), None))
                    .unwrap();
            }
            assert!(
                edges.pending_edges() <= 26,
                "direct={direct}, edges={}",
                edges.pending_edges()
            );
        }
        assert!(rig.observation.captured_audio().is_none());
        rig.capture_audio().unwrap();
        rig.observation.configure_output(policy().output);
        rig.machine.run_bounded(Some(1_100_000)).unwrap();
        assert!(edges.pending_edges() > 26);
        rig.capture_audio().unwrap();
        assert!(edges.pending_edges() <= 26);
        assert!(rig.observation.captured_audio().unwrap().frame_count() > 0);
        rig.observation.configure_output(OutputConfig::default());
        rig.observation.subscribe_audio(Box::new(NullAudioSink));
        rig.machine.run_bounded(Some(1_200_000)).unwrap();
        assert!(edges.pending_edges() <= 26);
    }
}
