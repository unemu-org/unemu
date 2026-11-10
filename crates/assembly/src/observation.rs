// SPDX-License-Identifier: AGPL-3.0-or-later
//! Typed output routing, resolved before guest execution.
//!
//! The facade observes card-owned state and semantic emissions, never decodes
//! registers or replays accesses. See `backlog/docs/design/observation.md`,
//! §Typed capabilities and route ownership and §Sinks, bounds and lifecycle.
//! VGA Technical Reference, Alphanumeric Modes, and PC16550D datasheet,
//! Transmitter Holding Register, define the two producer contracts.

use std::{cell::RefCell, collections::HashMap};

use bus::Bus;
use cpu::Cpu;
use device::{ByteCapture, ByteOverflow, ByteStreamKind, Device};
use video::{render_text_mode, CaptureFrameSink, Frame, FrameSink};

use crate::{AssembledMachine, ConfigError, DeviceId, FrameSinkKind, OutputConfig};

/// An owned display at a completed machine service boundary.
///
/// Both counters come from the same machine as the card. Virtual time includes
/// halted PIT clocks; retired counts only executed instructions. See
/// `backlog/docs/design/observation.md`, §Deterministic boundaries and assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayCapture {
    /// Virtual time in instruction equivalents, including halted time.
    pub virtual_time: u64,
    /// Instructions retired by the CPU at this boundary.
    pub retired: u64,
    /// One rendered frame, shared by capture and presentation consumers.
    pub frame: Frame,
}

impl AssembledMachine {
    /// Capture a selected card with this machine's execution counters.
    /// Call after a completed service boundary, including an error stop.
    /// This presents the frame to subscribers and replaces the configured sink's
    /// last frame, without advancing devices or executing the CPU.
    pub fn capture_text(&self, device: DeviceId) -> Result<DisplayCapture, ObservationError> {
        Ok(DisplayCapture {
            virtual_time: self.machine.virtual_time(),
            retired: self.machine.cpu().retired(),
            frame: self.observation.capture_text(self.machine.bus(), device)?,
        })
    }

    /// Capture all selected displays from this machine at the same boundary.
    pub fn capture_displays(&self) -> Result<Vec<(DeviceId, DisplayCapture)>, ObservationError> {
        self.observation
            .routes()
            .iter()
            .filter(|route| route.output == OutputKind::TextScanout)
            .map(|route| {
                self.capture_text(route.device)
                    .map(|capture| (route.device, capture))
            })
            .collect()
    }
}

/// The requested capability, independent of the device's role or port address.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum OutputKind {
    /// Passive, borrowed text memory and scanout scalars.
    TextScanout,
    /// Accepted bytes with the specified stream semantics.
    Bytes(ByteStreamKind),
}

/// One explicitly selected output and its capture bound.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ObservationRequest {
    /// Stable role to resolve at assembly time.
    pub device: DeviceId,
    /// Capability the configured device must support.
    pub output: OutputKind,
    /// Maximum total stream bytes per capture epoch; ignored for text scanout.
    /// Drains release memory without renewing the budget.
    /// Overflow fails observation until a new epoch, without stopping hardware.
    pub byte_limit: usize,
}

/// Run-scoped observation choices, separate from guest hardware identity.
///
/// The default selects no routes and allocates no stream history. This policy
/// does not select CPU cooperation size or schedule capture boundaries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObservationPolicy {
    /// Access diagnostics for every device and the open bus, independent of routes.
    /// Defaults to Off, including when semantic output capture is requested.
    pub trace: device::TracePolicy,
    /// Outputs to resolve and bind before guest execution.
    pub routes: Vec<ObservationRequest>,
}

/// An output whose device capability was checked during assembly.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ObservationRoute {
    /// Stable device role requested by the host.
    pub device: DeviceId,
    /// Validated output channel.
    pub output: OutputKind,
    /// Resolved bus slot, independent of device ordering in other presets.
    pub device_index: usize,
}

/// A failed observation must not masquerade as an empty frame or stream.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ObservationError {
    /// The caller did not request this output at assembly time.
    #[error("observation route is not bound: {device:?} {output:?}")]
    RouteNotBound {
        /// Requested role.
        device: DeviceId,
        /// Requested channel.
        output: OutputKind,
    },
    /// A bus no longer exposes the capability validated during assembly.
    #[error("text scanout is unavailable for bound device {0:?}")]
    ScanoutUnavailable(DeviceId),
    /// Captured bytes exceeded the route's explicit limit.
    #[error("observation overflow for {device:?} {kind:?}: {source}")]
    ByteOverflow {
        /// Producing role.
        device: DeviceId,
        /// Stream whose capture is incomplete.
        kind: ByteStreamKind,
        /// Latched capture failure.
        source: ByteOverflow,
    },
}

/// Assembly-owned routes, bounded byte collectors and per-display subscribers.
///
/// Each display has an optional last-frame capture sink and external boxed
/// `FrameSink` subscribers, delivered in that order and then subscription order.
///
/// Render once at a completed service boundary, then distribute the owned
/// frames/batches to consumers. There is no device drain or output downcast.
/// Hardware resets keep collectors installed; the host explicitly starts a new
/// capture epoch. See `backlog/docs/design/observation.md`, §Typed capabilities
/// and route ownership and §Sinks, bounds and lifecycle.
#[derive(Default)]
pub struct Observation {
    routes: Vec<ObservationRoute>,
    streams: HashMap<(DeviceId, ByteStreamKind), ByteCapture>,
    displays: HashMap<DeviceId, DisplaySubscribers>,
}

#[derive(Default)]
struct DisplaySubscribers {
    capture: RefCell<Option<CaptureFrameSink>>,
    sinks: RefCell<Vec<Box<dyn FrameSink>>>,
}

impl Observation {
    /// Resolve routes against a manually assembled device set before execution.
    /// This supports inline machines using the same collectors as config presets.
    /// Presentation starts Headless; call [`Self::configure_output`] to select sinks.
    /// Missing roles, invalid slots and unsupported capabilities are config errors.
    pub fn bind(
        policy: &ObservationPolicy,
        device_index: &HashMap<DeviceId, usize>,
        devices: &mut [Box<dyn Device>],
    ) -> Result<Self, ConfigError> {
        let mut observation = Self::default();
        for request in &policy.routes {
            let device = request.device;
            let output = request.output;
            if observation
                .routes
                .iter()
                .any(|route| route.device == device && route.output == output)
            {
                return Err(ConfigError::DuplicateObservationRoute { device, output });
            }
            let index = device_index
                .get(&device)
                .copied()
                .ok_or(ConfigError::UnsupportedObservationOutput { device, output })?;
            let producer_device = devices
                .get_mut(index)
                .ok_or(ConfigError::UnsupportedObservationOutput { device, output })?;
            match output {
                OutputKind::TextScanout => {
                    if producer_device.text_scanout().is_none() {
                        return Err(ConfigError::UnsupportedObservationOutput { device, output });
                    }
                }
                OutputKind::Bytes(kind) => {
                    let producer = producer_device
                        .byte_output()
                        .filter(|producer| producer.kind() == kind)
                        .ok_or(ConfigError::UnsupportedObservationOutput { device, output })?;
                    let capture = ByteCapture::new(request.byte_limit);
                    producer.bind_sink(Box::new(capture.clone()));
                    observation.streams.insert((device, kind), capture);
                }
            }
            observation.routes.push(ObservationRoute {
                device,
                output,
                device_index: index,
            });
        }
        Ok(observation)
    }

    /// Select presentation for all bound text routes, including inline hosts
    /// that use [`Self::bind`]. Reconfiguration clears configured last frames
    /// and preserves external subscribers; it does not add observation routes.
    pub fn configure_output(&mut self, output: OutputConfig) {
        for route in &self.routes {
            if route.output == OutputKind::TextScanout {
                let subscribers = self.displays.entry(route.device).or_default();
                *subscribers.capture.get_mut() = match output.frame_sink {
                    FrameSinkKind::Headless => None,
                    FrameSinkKind::Capture => Some(CaptureFrameSink::new()),
                };
            }
        }
    }

    /// Subscribe a host presenter to a bound display, without lending it the
    /// machine. Sinks must return promptly, use no host timing in execution and
    /// never call back into the machine. They receive only the captured frame,
    /// after the configured capture sink, in subscription order.
    pub fn subscribe(
        &mut self,
        device: DeviceId,
        sink: Box<dyn FrameSink>,
    ) -> Result<(), ObservationError> {
        self.text_route(device)?;
        self.displays
            .entry(device)
            .or_default()
            .sinks
            .get_mut()
            .push(sink);
        Ok(())
    }

    /// Number of presentation subscribers, including the configured capture sink.
    ///
    /// Do not call this from inside a sink's `present`: the subscriber list is
    /// borrowed while frames are delivered.
    #[must_use]
    pub fn subscriber_count(&self, device: DeviceId) -> usize {
        self.displays.get(&device).map_or(0, |subscribers| {
            subscribers.sinks.borrow().len() + usize::from(subscribers.capture.borrow().is_some())
        })
    }

    /// Last frame delivered to the configured capture sink; absent in headless
    /// mode or before the first capture. Each display retains at most one frame.
    #[must_use]
    pub fn captured_frame(&self, device: DeviceId) -> Option<Frame> {
        self.displays
            .get(&device)?
            .capture
            .borrow()
            .as_ref()?
            .last()
            .cloned()
    }

    pub(crate) fn text_route(
        &self,
        device: DeviceId,
    ) -> Result<&ObservationRoute, ObservationError> {
        let output = OutputKind::TextScanout;
        self.routes
            .iter()
            .find(|route| route.device == device && route.output == output)
            .ok_or(ObservationError::RouteNotBound { device, output })
    }

    /// The stable role/output table resolved during assembly, in request order.
    #[must_use]
    pub fn routes(&self) -> &[ObservationRoute] {
        &self.routes
    }

    /// Render one selected display and fan out the owned frame to its capture
    /// sink, then external subscribers in subscription order. Failed captures
    /// feed no sink. This does not execute or synchronize hardware.
    pub(crate) fn capture_text(
        &self,
        bus: &Bus,
        device: DeviceId,
    ) -> Result<Frame, ObservationError> {
        let route = self.text_route(device)?;
        let state = bus
            .device(route.device_index)
            .and_then(Device::text_scanout)
            .ok_or(ObservationError::ScanoutUnavailable(device))?;
        let frame = Frame::Text(render_text_mode(&state));
        if let Some(subscribers) = self.displays.get(&device) {
            if let Some(capture) = &mut *subscribers.capture.borrow_mut() {
                capture.present(&frame);
            }
            for sink in &mut *subscribers.sinks.borrow_mut() {
                sink.present(&frame);
            }
        }
        Ok(frame)
    }

    /// Drain one route's complete byte batch for distribution by the host.
    /// Overflow remains a failure on every drain until a new capture epoch.
    pub fn drain_bytes(
        &self,
        device: DeviceId,
        kind: ByteStreamKind,
    ) -> Result<Vec<u8>, ObservationError> {
        self.streams
            .get(&(device, kind))
            .ok_or(ObservationError::RouteNotBound {
                device,
                output: OutputKind::Bytes(kind),
            })?
            .drain()
            .map_err(|source| ObservationError::ByteOverflow {
                device,
                kind,
                source,
            })
    }

    /// Start a fresh capture epoch without resetting guest hardware.
    /// Configured last-frame sinks are cleared; external subscribers own their lifecycle.
    pub fn start_epoch(&self) {
        for subscribers in self.displays.values() {
            let mut capture = subscribers.capture.borrow_mut();
            if capture.is_some() {
                *capture = Some(CaptureFrameSink::new());
            }
        }
        for capture in self.streams.values() {
            capture.start_epoch();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        assemble, assemble_with_observation, AssemblyError, DeviceConfig, DeviceKind, PortWindow,
        PostPortCfg,
    };

    fn request(device: DeviceId, output: OutputKind, byte_limit: usize) -> ObservationRequest {
        ObservationRequest {
            device,
            output,
            byte_limit,
        }
    }

    #[test]
    fn display_fanout_updates_capture_before_external_subscription_order() {
        use std::rc::{Rc, Weak};

        struct OrderedSink {
            observation: Weak<Observation>,
            log: Rc<RefCell<Vec<(u8, Frame)>>>,
            id: u8,
        }
        impl FrameSink for OrderedSink {
            fn present(&mut self, frame: &Frame) {
                let observation = self.observation.upgrade().unwrap();
                assert_eq!(
                    observation.captured_frame(DeviceId::Vga).as_ref(),
                    Some(frame)
                );
                self.log.borrow_mut().push((self.id, frame.clone()));
            }
        }

        let log = Rc::new(RefCell::new(Vec::new()));
        let mut devices: Vec<Box<dyn Device>> = vec![Box::new(devices::VgaText::new())];
        let observation = Rc::new_cyclic(|weak| {
            let mut observation = Observation::bind(
                &ObservationPolicy {
                    routes: vec![request(DeviceId::Vga, OutputKind::TextScanout, 0)],
                    ..Default::default()
                },
                &HashMap::from([(DeviceId::Vga, 0)]),
                &mut devices,
            )
            .unwrap();
            observation.configure_output(OutputConfig {
                frame_sink: FrameSinkKind::Capture,
            });
            for id in [1, 2] {
                observation
                    .subscribe(
                        DeviceId::Vga,
                        Box::new(OrderedSink {
                            observation: weak.clone(),
                            log: log.clone(),
                            id,
                        }),
                    )
                    .unwrap();
            }
            observation
        });
        let mut bus = Bus::new(devices, bus::DecodeMap::default());
        let mut expected = Vec::new();
        for word in [0x0741, 0x7042] {
            bus.device_mut(0)
                .unwrap()
                .mmio_write(0xB8000, 2, word)
                .unwrap();
            let frame = observation.capture_text(&bus, DeviceId::Vga).unwrap();
            expected.extend([(1, frame.clone()), (2, frame)]);
        }
        assert_eq!(*log.borrow(), expected);
        assert_eq!(observation.subscriber_count(DeviceId::Vga), 3);
    }

    #[test]
    fn display_subscription_rejects_bytes_only_route() {
        struct NoFrames;
        impl FrameSink for NoFrames {
            fn present(&mut self, _: &Frame) {
                panic!("byte routes cannot emit frames");
            }
        }
        let config = crate::at_boot(vec![0xF4]);
        let mut assembled = assemble_with_observation(
            &config,
            &ObservationPolicy {
                routes: vec![request(
                    DeviceId::Uart,
                    OutputKind::Bytes(ByteStreamKind::SerialTx),
                    8,
                )],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            assembled
                .observation
                .subscribe(DeviceId::Uart, Box::new(NoFrames)),
            Err(ObservationError::RouteNotBound {
                device: DeviceId::Uart,
                output: OutputKind::TextScanout
            })
        );
        assert_eq!(assembled.observation.subscriber_count(DeviceId::Uart), 0);
    }

    #[test]
    fn failed_display_captures_preserve_sinks_without_delivery() {
        use std::rc::Rc;

        struct Recorder(Rc<RefCell<Vec<Frame>>>);
        impl FrameSink for Recorder {
            fn present(&mut self, frame: &Frame) {
                self.0.borrow_mut().push(frame.clone());
            }
        }
        let mut config = crate::at_vga_text(vec![0xF4]);
        config.output.frame_sink = FrameSinkKind::Capture;
        let mut assembled = assemble_with_observation(
            &config,
            &ObservationPolicy {
                routes: vec![request(DeviceId::Vga, OutputKind::TextScanout, 0)],
                ..Default::default()
            },
        )
        .unwrap();
        let frames = Rc::new(RefCell::new(Vec::new()));
        assembled
            .observation
            .subscribe(DeviceId::Vga, Box::new(Recorder(frames.clone())))
            .unwrap();
        let frame = assembled.capture_text(DeviceId::Vga).unwrap().frame;
        let index = assembled.device_index[&DeviceId::Vga];
        *assembled.machine.bus_mut().device_mut(index).unwrap() = Box::new(devices::I8042::new());
        assert_eq!(
            assembled.capture_text(DeviceId::Vga),
            Err(ObservationError::ScanoutUnavailable(DeviceId::Vga))
        );
        assert_eq!(
            assembled.capture_text(DeviceId::Mda),
            Err(ObservationError::RouteNotBound {
                device: DeviceId::Mda,
                output: OutputKind::TextScanout,
            })
        );
        assert_eq!(
            assembled.observation.captured_frame(DeviceId::Vga),
            Some(frame.clone())
        );
        assert_eq!(*frames.borrow(), vec![frame]);
    }

    #[test]
    fn output_selection_routes_subscribers_and_resets_capture_epochs() {
        use std::rc::Rc;

        struct Recorder(Rc<RefCell<Vec<Frame>>>);
        impl FrameSink for Recorder {
            fn present(&mut self, frame: &Frame) {
                self.0.borrow_mut().push(frame.clone());
            }
        }

        for kind in [FrameSinkKind::Headless, FrameSinkKind::Capture] {
            let mut config = crate::bios(vec![0; 0x8000], None);
            config.output.frame_sink = kind;
            config.cpu.reset = crate::ResetSpec::Entry { cs: 0xF800, ip: 0 };
            config.images[0].source = crate::ImageSource::Inline({
                let mut rom = vec![0; 0x8000];
                rom[..4].copy_from_slice(&[0x90, 0x90, 0xFB, 0xF4]);
                rom
            });
            config.board = crate::BoardModel::Bare;
            config.devices.reverse();
            let policy = ObservationPolicy {
                routes: vec![
                    request(DeviceId::Mda, OutputKind::TextScanout, 0),
                    request(DeviceId::Vga, OutputKind::TextScanout, 0),
                ],
                ..Default::default()
            };
            let mut assembled = assemble_with_observation(&config, &policy).unwrap();
            let count = usize::from(kind == FrameSinkKind::Capture);
            for device in [DeviceId::Mda, DeviceId::Vga] {
                assert_eq!(assembled.observation.subscriber_count(device), count);
                assert_eq!(assembled.observation.captured_frame(device), None);
            }
            let frames = Rc::new(RefCell::new(Vec::new()));
            assert!(matches!(
                assembled
                    .observation
                    .subscribe(DeviceId::Keyboard, Box::new(Recorder(frames.clone()))),
                Err(ObservationError::RouteNotBound { .. })
            ));
            assembled
                .observation
                .subscribe(DeviceId::Mda, Box::new(Recorder(frames.clone())))
                .unwrap();
            assert_eq!(
                assembled.observation.subscriber_count(DeviceId::Mda),
                count + 1
            );
            assembled.machine.set_idle_limit(Some(20));
            assert_eq!(
                assembled.machine.run_bounded(Some(7)).unwrap(),
                machine::RunOutcome::RetiredCapReached
            );
            assert_eq!(assembled.machine.cpu().retired(), 4);
            assert_eq!(assembled.machine.virtual_time(), 7);
            let index = assembled.device_index[&DeviceId::Mda];
            assembled
                .machine
                .bus_mut()
                .mmio_write(0xB0000, 2, 0x7041)
                .unwrap();
            let saved = assembled.machine.bus().device(index).unwrap().save();
            let first = assembled.capture_text(DeviceId::Mda).unwrap();
            assert_eq!((first.virtual_time, first.retired), (7, 4));
            assembled.machine.run_bounded(Some(9)).unwrap();
            assembled
                .machine
                .bus_mut()
                .mmio_write(0xB0000, 2, 0x0742)
                .unwrap();
            let final_captures = assembled.capture_displays().unwrap();
            assert_eq!(final_captures[0].0, DeviceId::Mda);
            assert_eq!(final_captures[1].0, DeviceId::Vga);
            assert_ne!(first.frame, final_captures[0].1.frame);
            assert_eq!(
                *frames.borrow(),
                vec![first.frame, final_captures[0].1.frame.clone()]
            );
            for (device, capture) in final_captures {
                assert_eq!(capture.virtual_time, 9);
                assert_eq!(capture.retired, 4);
                assert_eq!(
                    assembled.observation.captured_frame(device),
                    (kind == FrameSinkKind::Capture).then_some(capture.frame)
                );
            }
            assembled
                .machine
                .bus_mut()
                .device_mut(index)
                .unwrap()
                .restore(&saved)
                .unwrap();
            assert_eq!(
                assembled.capture_text(DeviceId::Mda).unwrap().frame,
                frames.borrow()[0]
            );
            assembled.observation.start_epoch();
            for device in [DeviceId::Mda, DeviceId::Vga] {
                assert_eq!(assembled.observation.captured_frame(device), None);
            }
            assert_eq!(
                assembled.observation.subscriber_count(DeviceId::Mda),
                count + 1
            );
            assert_eq!(frames.borrow().len(), 3);
        }
    }

    #[test]
    fn bios_mda_and_vga_have_independent_crtc_and_status_state() {
        let config = crate::bios(vec![0; 0x8000], None);
        let policy = ObservationPolicy {
            trace: device::TracePolicy::Off,
            routes: vec![
                request(DeviceId::Mda, OutputKind::TextScanout, 0),
                request(DeviceId::Vga, OutputKind::TextScanout, 0),
            ],
        };
        let mut assembled = assemble_with_observation(&config, &policy).unwrap();
        let bus = assembled.machine.bus_mut();
        bus.mmio_write(0xB0000, 4, 0x70420741).unwrap();
        for (index, value) in [(13, 1), (15, 3), (10, 0x20)] {
            bus.port_write(0x3B4, 1, index).unwrap();
            bus.port_write(0x3B5, 1, value).unwrap();
        }
        let Frame::Text(mda) = assembled.capture_text(DeviceId::Mda).unwrap().frame;
        let Frame::Text(vga) = assembled.capture_text(DeviceId::Vga).unwrap().frame;
        assert_eq!(
            mda.cell(0, 0),
            Some(&video::TextCell {
                glyph: b'B',
                attr: 0x70,
                style: video::TextStyle::Monochrome {
                    effect: video::MonochromeEffect::Reverse,
                    intense: false,
                    blink: false,
                    background_intense: false
                }
            })
        );
        assert_eq!(
            mda.cursor(),
            video::Cursor {
                col: 2,
                row: 0,
                visible: false
            }
        );
        assert_eq!(
            vga.cell(0, 0),
            Some(&video::TextCell {
                glyph: 0,
                attr: 0,
                style: video::TextStyle::Colour {
                    foreground: 0,
                    background: 0,
                    blink: false
                }
            })
        );
        assert_eq!(
            vga.cursor(),
            video::Cursor {
                col: 0,
                row: 0,
                visible: true
            }
        );
        let bus = assembled.machine.bus_mut();
        for status in [0xF6, 0xF7, 0xFE, 0xFF] {
            assert_eq!(bus.port_read(0x3BA, 1).unwrap(), status);
        }
        assert_eq!(bus.port_read(0x3DA, 1).unwrap(), 0);
        for port in [0x3BC, 0x3BD, 0x3BE] {
            assert_eq!(bus.port_read(port, 1).unwrap(), 0xFF);
        }
    }

    #[test]
    fn bios_mda_guest_store_reaches_scanout_instead_of_ram() {
        let mut config = crate::bios(vec![0; 0x8000], None);
        let card = config
            .memory
            .mmio
            .iter()
            .find(|w| w.base == 0xB0000)
            .expect("the monochrome aperture must be card-owned MMIO")
            .device;
        assert_eq!(card, DeviceId::Mda);
        config.cpu.reset = crate::ResetSpec::Entry { cs: 0, ip: 0x7C00 };
        config.cpu.segments = crate::SegmentSeed::flat(0);
        config.cpu.segments.ds = 0xB000;
        config.images.push(crate::LoadImage {
            base: 0x7C00,
            source: crate::ImageSource::Inline(vec![0xFA, 0xB8, b'A', 7, 0xA3, 0, 0, 0xF4]),
            kind: crate::ImageKind::RamLoad,
            provides: Vec::new(),
        });
        let policy = ObservationPolicy {
            trace: device::TracePolicy::Ring { capacity: 16 },
            routes: vec![request(card, OutputKind::TextScanout, 0)],
        };
        let mut assembled = assemble_with_observation(&config, &policy).unwrap();
        assert_eq!(
            assembled.machine.run_bounded(Some(100)).unwrap(),
            machine::RunOutcome::Halted
        );
        let capture = assembled.capture_text(card).unwrap();
        let Frame::Text(grid) = capture.frame;
        assert_eq!(
            grid.cell(0, 0),
            Some(&video::TextCell {
                glyph: b'A',
                attr: 7,
                style: video::TextStyle::Monochrome {
                    effect: video::MonochromeEffect::Normal,
                    intense: false,
                    blink: false,
                    background_intense: false
                }
            })
        );
        let mut bytes = [0; 2];
        assembled
            .machine
            .cpu()
            .read_mem(0xB0000, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0, 0], "guest MMIO must not mutate CPU backing RAM");
        let index = assembled.device_index[&card];
        assert!(assembled
            .machine
            .bus_mut()
            .device_mut(index)
            .unwrap()
            .take_trace()
            .iter()
            .any(|event| matches!(
                event,
                device::TraceEvent::Mmio {
                    addr: 0xB0000,
                    dir: device::AccessDir::Write,
                    value: 0x0741,
                    ..
                }
            )));
    }

    #[test]
    fn observation_routes_follow_device_ids_and_capture_one_running_machine() {
        let program = vec![
            0xBA, 0xF8, 0x03, 0xB0, b'A', 0xEE, 0xBA, 0x90, 0x01, 0xB0, 0xFF, 0xEE, 0xB8, b'B',
            0x1F, 0xA3, 0, 0, 0xF4,
        ];
        let mut config = crate::at_vga_text(program);
        config.devices.push(DeviceConfig::new(
            DeviceId::Post,
            DeviceKind::Post(PostPortCfg { port: 0x190 }),
            vec![PortWindow::single(0x190)],
        ));
        config.devices.reverse();
        let policy = ObservationPolicy {
            trace: device::TracePolicy::Off,
            routes: vec![
                request(DeviceId::Vga, OutputKind::TextScanout, 0),
                request(
                    DeviceId::Post,
                    OutputKind::Bytes(ByteStreamKind::PostCheckpoints),
                    8,
                ),
                request(
                    DeviceId::Uart,
                    OutputKind::Bytes(ByteStreamKind::SerialTx),
                    8,
                ),
            ],
        };
        let mut assembled = assemble_with_observation(&config, &policy).unwrap();
        assert_eq!(assembled.observation.routes().len(), 3);
        for route in assembled.observation.routes() {
            assert_eq!(route.device_index, assembled.device_index[&route.device]);
        }
        assembled.machine.run().unwrap();
        let retired = assembled.machine.cpu().retired();
        let time = assembled.machine.virtual_time();
        let capture = assembled.capture_text(DeviceId::Vga).unwrap();
        assert_eq!(capture.virtual_time, time);
        assert_eq!(capture.retired, retired);
        assert_eq!(assembled.capture_text(DeviceId::Vga).unwrap(), capture);
        let frame = capture.frame;
        let Frame::Text(grid) = &frame;
        assert_eq!(
            grid.cell(0, 0).unwrap(),
            &video::TextCell {
                glyph: b'B',
                attr: 0x1F,
                style: video::TextStyle::Colour {
                    foreground: 15,
                    background: 1,
                    blink: false
                },
            }
        );
        assert_eq!((grid.cols(), grid.rows()), (80, 25));
        let mut sink = CaptureFrameSink::new();
        sink.present(&frame);
        assert_eq!(sink.last(), Some(&frame));
        assert_eq!(
            assembled.capture_displays().unwrap(),
            vec![(
                DeviceId::Vga,
                DisplayCapture {
                    virtual_time: time,
                    retired,
                    frame
                }
            )]
        );
        assert_eq!(assembled.machine.cpu().retired(), retired);
        assert_eq!(assembled.machine.virtual_time(), time);
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Uart, ByteStreamKind::SerialTx)
                .unwrap(),
            b"A"
        );
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Post, ByteStreamKind::PostCheckpoints)
                .unwrap(),
            [0xFF]
        );
        assert!(assembled
            .observation
            .drain_bytes(DeviceId::Uart, ByteStreamKind::SerialTx)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stream_facade_preserves_divisor_banking_byte_cycles_and_reset_history() {
        let mut config = crate::at_boot(vec![0xF4]);
        config.devices.push(DeviceConfig::new(
            DeviceId::Post,
            DeviceKind::Post(PostPortCfg { port: 0x190 }),
            vec![PortWindow::single(0x190)],
        ));
        let policy = ObservationPolicy {
            trace: device::TracePolicy::Ring { capacity: 100 },
            routes: vec![
                request(
                    DeviceId::Uart,
                    OutputKind::Bytes(ByteStreamKind::SerialTx),
                    4,
                ),
                request(
                    DeviceId::Post,
                    OutputKind::Bytes(ByteStreamKind::PostCheckpoints),
                    5,
                ),
            ],
        };
        let mut assembled = assemble_with_observation(&config, &policy).unwrap();
        let bus = assembled.machine.bus_mut();
        bus.port_write(0x3FB, 1, 0x83).unwrap();
        bus.port_write(0x3F8, 2, 0x0142).unwrap();
        bus.port_write(0x3FB, 1, 3).unwrap();
        bus.port_write(0x3F8, 2, 0x004F).unwrap();
        bus.port_write(0x3F8, 1, u32::from(b'K')).unwrap();
        for byte in [0, 0xFF] {
            bus.port_write(0x3F8, 1, byte).unwrap();
        }
        for value in [0x1234, 0x1234, 0, 0xFF] {
            bus.port_write(0x190, 4, value).unwrap();
        }
        assert_eq!(bus.port_read(0x190, 2).unwrap(), 0xFFFF);
        for device in [DeviceId::Uart, DeviceId::Post] {
            let dev = bus.device_mut(assembled.device_index[&device]).unwrap();
            let state = dev.save();
            dev.take_trace();
            dev.reset();
            dev.restore(&state).unwrap();
        }
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Uart, ByteStreamKind::SerialTx)
                .unwrap(),
            b"OK\0\xff"
        );
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Post, ByteStreamKind::PostCheckpoints)
                .unwrap(),
            [0x34, 0x34, 0, 0xFF]
        );
        assembled
            .machine
            .bus_mut()
            .port_write(0x190, 1, 42)
            .unwrap();
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Post, ByteStreamKind::PostCheckpoints)
                .unwrap(),
            [42]
        );
        assembled
            .machine
            .bus_mut()
            .port_write(0x190, 1, 43)
            .unwrap();
        assert!(matches!(
            assembled
                .observation
                .drain_bytes(DeviceId::Post, ByteStreamKind::PostCheckpoints),
            Err(ObservationError::ByteOverflow { .. })
        ));
        assembled.observation.start_epoch();
        assembled.machine.bus_mut().port_write(0x190, 1, 7).unwrap();
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Post, ByteStreamKind::PostCheckpoints)
                .unwrap(),
            [7]
        );
    }

    #[test]
    fn display_capture_stamps_halted_virtual_time_separately_from_retired() {
        let config = crate::at_vga_text(vec![0xFB, 0xF4]);
        let policy = ObservationPolicy {
            trace: device::TracePolicy::Off,
            routes: vec![request(DeviceId::Vga, OutputKind::TextScanout, 0)],
        };
        let mut assembled = assemble_with_observation(&config, &policy).unwrap();
        crate::run(
            &mut assembled.machine,
            &assembled.observation,
            &crate::RunPolicy::natural(Some(10), Some(20)),
        )
        .unwrap();
        let capture = assembled.capture_text(DeviceId::Vga).unwrap();
        assert_eq!(capture.retired, assembled.machine.cpu().retired());
        assert_eq!(capture.virtual_time, assembled.machine.virtual_time());
        assert!(capture.virtual_time > capture.retired);
        assert_eq!(assembled.capture_text(DeviceId::Vga).unwrap(), capture);
        assert_eq!(
            assembled.capture_displays().unwrap(),
            vec![(DeviceId::Vga, capture)]
        );
    }

    #[test]
    fn observation_duplicate_route_is_a_configuration_error() {
        let config = crate::at_boot(vec![0xF4]);
        let route = request(
            DeviceId::Uart,
            OutputKind::Bytes(ByteStreamKind::SerialTx),
            8,
        );
        let policy = ObservationPolicy {
            trace: device::TracePolicy::Off,
            routes: vec![route, route],
        };
        assert!(matches!(assemble_with_observation(&config, &policy),
            Err(AssemblyError::Config(ConfigError::DuplicateObservationRoute { device: DeviceId::Uart, output }))
                if output == route.output));
    }

    #[test]
    fn observation_unrequested_output_is_explicit_and_defaults_allocate_no_collectors() {
        let assembled = assemble(&crate::at_boot(vec![0xF4])).unwrap();
        assert!(assembled.observation.routes().is_empty());
        assert!(assembled.observation.streams.is_empty());
        assert!(matches!(
            assembled.capture_text(DeviceId::Vga),
            Err(ObservationError::RouteNotBound { .. })
        ));
        assert!(matches!(
            assembled
                .observation
                .drain_bytes(DeviceId::Uart, ByteStreamKind::SerialTx),
            Err(ObservationError::RouteNotBound { .. })
        ));
        assert!(assembled
            .machine
            .bus()
            .device(assembled.device_index[&DeviceId::Pit])
            .unwrap()
            .text_scanout()
            .is_none());
    }

    #[test]
    fn observation_byte_overflow_fails_drains_until_explicit_epoch() {
        let config = crate::at_boot(vec![0xF4]);
        let policy = ObservationPolicy {
            trace: device::TracePolicy::Off,
            routes: vec![request(
                DeviceId::Uart,
                OutputKind::Bytes(ByteStreamKind::SerialTx),
                1,
            )],
        };
        let mut assembled = assemble_with_observation(&config, &policy).unwrap();
        for byte in [0, 0xFF] {
            assembled
                .machine
                .bus_mut()
                .port_write(0x3F8, 1, byte)
                .unwrap();
        }
        let expected = Err(ObservationError::ByteOverflow {
            device: DeviceId::Uart,
            kind: ByteStreamKind::SerialTx,
            source: ByteOverflow { limit: 1 },
        });
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Uart, ByteStreamKind::SerialTx),
            expected
        );
        assembled
            .machine
            .bus_mut()
            .device_mut(assembled.device_index[&DeviceId::Uart])
            .unwrap()
            .reset();
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Uart, ByteStreamKind::SerialTx),
            expected
        );
        assembled.observation.start_epoch();
        assembled
            .machine
            .bus_mut()
            .port_write(0x3F8, 1, 42)
            .unwrap();
        assert_eq!(
            assembled
                .observation
                .drain_bytes(DeviceId::Uart, ByteStreamKind::SerialTx)
                .unwrap(),
            [42]
        );
    }
}
