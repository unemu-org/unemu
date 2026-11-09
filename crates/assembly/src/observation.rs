// SPDX-License-Identifier: AGPL-3.0-or-later
//! Typed output routing, resolved before guest execution.
//!
//! The facade observes card-owned state and semantic emissions, never decodes
//! registers or replays accesses. See `backlog/docs/design/observation.md`,
//! §Typed capabilities and route ownership and §Sinks, bounds and lifecycle.
//! VGA Technical Reference, Alphanumeric Modes, and PC16550D datasheet,
//! Transmitter Holding Register, define the two producer contracts.

use std::collections::HashMap;

use bus::Bus;
use cpu::Cpu;
use device::{ByteCapture, ByteOverflow, ByteStreamKind, Device};
use video::{render_text_mode, Frame};

use crate::{AssembledMachine, ConfigError, DeviceId};

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
    /// Call after a completed service boundary, including an error stop; this
    /// passive query neither advances devices nor executes the CPU.
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

/// Assembly-owned routes and one bounded collector per requested stream.
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
}

impl Observation {
    /// Resolve routes against a manually assembled device set before execution.
    /// This supports inline machines using the same collectors as config presets.
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

    /// The stable role/output table resolved during assembly, in request order.
    #[must_use]
    pub fn routes(&self) -> &[ObservationRoute] {
        &self.routes
    }

    /// Capture one selected display into an owned frame before releasing the
    /// card borrow. Calling this does not execute or synchronize hardware.
    fn capture_text(&self, bus: &Bus, device: DeviceId) -> Result<Frame, ObservationError> {
        let output = OutputKind::TextScanout;
        let route = self
            .routes
            .iter()
            .find(|route| route.device == device && route.output == output)
            .ok_or(ObservationError::RouteNotBound { device, output })?;
        let state = bus
            .device(route.device_index)
            .and_then(Device::text_scanout)
            .ok_or(ObservationError::ScanoutUnavailable(device))?;
        Ok(Frame::Text(render_text_mode(&state)))
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

    /// Start a fresh run's stream capture without resetting guest hardware.
    pub fn start_epoch(&self) {
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
    use video::{CaptureFrameSink, FrameSink};

    fn request(device: DeviceId, output: OutputKind, byte_limit: usize) -> ObservationRequest {
        ObservationRequest {
            device,
            output,
            byte_limit,
        }
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
                attr: 0x1F
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
            routes: vec![request(DeviceId::Vga, OutputKind::TextScanout, 0)],
        };
        let mut assembled = assemble_with_observation(&config, &policy).unwrap();
        crate::run(
            &mut assembled.machine,
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
