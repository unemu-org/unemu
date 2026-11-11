// SPDX-License-Identifier: AGPL-3.0-or-later
//! Emulator-only firmware configuration, not period hardware.
//!
//! QEMU Firmware Configuration (fw_cfg) Device specification, Guest-side
//! Hardware Interface and Firmware Configuration Items: 510h selects a 16-bit
//! little-endian key, 511h streams bytes, and selection rewinds the stream.
//! The file directory uses big-endian fields and 56-byte NUL-padded names.
//! See `backlog/docs/design/firmware-and-machine-classes.md` §3.6 and
//! `backlog/docs/design/machine-config.md` §Emulator-only firmware configuration.
//!
//! Only the traditional read interface is advertised (ID = 1). DMA at
//! 514h/518h is absent. Legacy data writes are deprecated by the specification
//! and ignored, including byte writes at 510h: QEMU's combined I/O interface
//! does not assemble two byte writes into a selector. Byte reads at either
//! port stream data, matching that combined interface. The bus must preserve
//! word writes at 510h; 511h remains byte-wide.
//!
//! A word or dword IN at 510h reaches this device as a size-2 read and returns
//! `UnmappedRegister`, stopping the run with a bus error under the project's
//! invalid-width policy; QEMU drops the invalid access and the guest continues.
//! A dword OUT at 510h is split into a word cycle that selects the low word and
//! an open-bus cycle at 512h, whereas QEMU rejects four-byte writes; SeaBIOS and
//! Linux use OUTW.
//!
//! Legacy items are empty (reads return zero), including boot menu 000Eh.
//! The architecture bit selects a separate, empty namespace; the deprecated
//! write-channel bit does not change which item is read. All file bytes are
//! fixed at construction. Snapshots retain selector, offset and virtual clock;
//! configuration and diagnostic traces stay with the receiving instance.

use device::{
    AccessDir, Device, DeviceError, NextEvent, TraceBuffer, TraceEvent, TracePolicy, VClock,
};

const NAME: &str = "fw-cfg";
const SNAPSHOT_VERSION: u8 = 1;
const FILE_FIRST: u16 = 0x20;

/// One immutable file in configuration order, starting at selector 0020h.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FwCfgFile {
    /// Nonempty ASCII name, at most 55 bytes with no embedded NUL.
    pub name: String,
    /// Guest-visible contents, supplied before execution.
    pub bytes: Vec<u8>,
}

/// A configuration that cannot be represented by the fw_cfg file directory.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FwCfgError {
    /// Selector keys must not reach the write-channel bit.
    #[error("too many fw_cfg files (maximum 16352)")]
    TooManyFiles,
    /// Directory names must fit, including a terminating NUL.
    #[error(
        "fw_cfg file names must be unique nonempty ASCII strings of at most 55 bytes without NUL"
    )]
    InvalidName,
    /// The directory has a 32-bit file-size field.
    #[error("fw_cfg file size exceeds 32 bits")]
    FileTooLarge,
}

/// QEMU-compatible, read-only firmware configuration I/O device.
pub struct FwCfg {
    files: Vec<FwCfgFile>,
    directory: Vec<u8>,
    selector: u16,
    offset: u64,
    now: VClock,
    trace: TraceBuffer,
}

impl Default for FwCfg {
    fn default() -> Self {
        Self::new()
    }
}

impl FwCfg {
    /// Create an empty file set with signature, ID and zero-valued legacy keys.
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            directory: vec![0; 4],
            selector: 0,
            offset: 0,
            now: VClock::ZERO,
            trace: TraceBuffer::default(),
        }
    }

    /// Check representability before assembly or allocation of the directory.
    pub fn validate_files(files: &[FwCfgFile]) -> Result<(), FwCfgError> {
        if files.len() > usize::from(0x4000 - FILE_FIRST) {
            return Err(FwCfgError::TooManyFiles);
        }
        let mut names = std::collections::BTreeSet::new();
        for file in files {
            if file.name.is_empty()
                || file.name.len() > 55
                || !file.name.is_ascii()
                || file.name.as_bytes().contains(&0)
                || !names.insert(&file.name)
            {
                return Err(FwCfgError::InvalidName);
            }
            if u32::try_from(file.bytes.len()).is_err() {
                return Err(FwCfgError::FileTooLarge);
            }
        }
        Ok(())
    }

    /// Freeze the named files and their directory in configuration order.
    pub fn with_files(files: Vec<FwCfgFile>) -> Result<Self, FwCfgError> {
        Self::validate_files(&files)?;
        let mut directory = Vec::with_capacity(4 + 64 * files.len());
        directory.extend_from_slice(&(files.len() as u32).to_be_bytes());
        for (index, file) in files.iter().enumerate() {
            directory.extend_from_slice(&(file.bytes.len() as u32).to_be_bytes());
            directory.extend_from_slice(&(FILE_FIRST + index as u16).to_be_bytes());
            directory.extend_from_slice(&[0; 2]);
            let mut name = [0; 56];
            name[..file.name.len()].copy_from_slice(file.name.as_bytes());
            directory.extend_from_slice(&name);
        }
        Ok(Self {
            files,
            directory,
            ..Self::default()
        })
    }

    fn read_byte(&mut self) -> u8 {
        let key = self.selector & !0x4000;
        let bytes: &[u8] = match key {
            0 => b"QEMU",
            1 => &[1, 0, 0, 0],
            0x19 => &self.directory,
            FILE_FIRST..=0x3FFF => self
                .files
                .get(usize::from(key - FILE_FIRST))
                .map_or(&[], |file| file.bytes.as_slice()),
            _ => &[],
        };
        let value = usize::try_from(self.offset)
            .ok()
            .and_then(|index| bytes.get(index))
            .copied()
            .unwrap_or(0);
        self.offset = self.offset.saturating_add(1);
        value
    }

    fn check(port: u16, size: u8, write: bool) -> Result<(), DeviceError> {
        if (port == 0x510 || port == 0x511) && (size == 1 || (write && port == 0x510 && size == 2))
        {
            Ok(())
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "port or width",
                offset: u64::from(port),
            })
        }
    }
}

impl Device for FwCfg {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        Self::check(port, size, false)?;
        let value = u32::from(self.read_byte());
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Read,
            value,
        });
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        Self::check(port, size, true)?;
        if size == 2 {
            self.selector = value as u16;
            self.offset = 0;
        }
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Write,
            value,
        });
        Ok(())
    }

    fn mmio_read(&mut self, addr: u64, _size: u8) -> Result<u64, DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }

    fn mmio_write(&mut self, addr: u64, _size: u8, _value: u64) -> Result<(), DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }

    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now >= self.now {
            self.now = now;
        }
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }
    fn irq_pending(&self) -> bool {
        false
    }

    fn reset(&mut self) {
        self.selector = 0;
        self.offset = 0;
        self.now = VClock::ZERO;
        self.trace.clear();
    }

    fn save(&self) -> Vec<u8> {
        let mut snapshot = vec![SNAPSHOT_VERSION];
        snapshot.extend_from_slice(&self.selector.to_le_bytes());
        snapshot.extend_from_slice(&self.offset.to_le_bytes());
        snapshot.extend_from_slice(&self.now.0.to_le_bytes());
        snapshot
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        if snapshot.len() != 19 || snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot format",
            });
        }
        self.selector = u16::from_le_bytes([snapshot[1], snapshot[2]]);
        self.offset = u64::from_le_bytes(std::array::from_fn(|i| snapshot[3 + i]));
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| {
            snapshot[11 + i]
        })));
        Ok(())
    }

    fn set_trace_policy(&mut self, policy: TracePolicy) {
        self.trace.set_policy(policy);
    }
    fn trace_dropped(&self) -> u64 {
        self.trace.dropped()
    }
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.trace.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    fn rig(files: Vec<FwCfgFile>) -> MockBus {
        MockBus::new(Box::new(FwCfg::with_files(files).unwrap()))
    }
    fn file(name: &str, bytes: &[u8]) -> FwCfgFile {
        FwCfgFile {
            name: name.into(),
            bytes: bytes.into(),
        }
    }
    fn select(bus: &mut MockBus, key: u16) {
        bus.port_write(0x510, 2, u32::from(key)).unwrap();
    }
    fn read(bus: &mut MockBus, len: usize) -> Vec<u8> {
        (0..len)
            .map(|_| bus.port_read(0x511, 1).unwrap() as u8)
            .collect()
    }

    #[test]
    fn signature_reads_qemu() {
        let mut bus = rig(vec![]);
        select(&mut bus, 0);
        assert_eq!(read(&mut bus, 5), b"QEMU\0");
    }

    #[test]
    fn id_advertises_only_traditional_interface() {
        let mut bus = rig(vec![]);
        select(&mut bus, 1);
        assert_eq!(read(&mut bus, 5), [1, 0, 0, 0, 0]);
    }

    fn directory_case(count: usize) {
        let files = [
            file("etc/sercon-port", &[0xF8, 3]),
            file("genroms/test", &[1, 2, 3]),
            file(&"x".repeat(55), &[]),
        ];
        let mut bus = rig(files[..count].into());
        select(&mut bus, 0x19);
        // Literal wire fields keep this expectation independent of the encoder.
        let mut expected = vec![0, 0, 0, count as u8];
        for (header, name) in [
            ([0, 0, 0, 2, 0, 0x20, 0, 0], "etc/sercon-port".to_string()),
            ([0, 0, 0, 3, 0, 0x21, 0, 0], "genroms/test".to_string()),
            ([0, 0, 0, 0, 0, 0x22, 0, 0], "x".repeat(55)),
        ]
        .into_iter()
        .take(count)
        {
            expected.extend(header);
            expected.extend(name.bytes());
            expected.resize(expected.len() + 56 - name.len(), 0);
        }
        assert_eq!(read(&mut bus, 4 + 64 * count), expected);
        assert_eq!(read(&mut bus, 2), [0, 0]);
        for (i, file) in files.iter().take(count).enumerate() {
            select(&mut bus, 0x20 + i as u16);
            assert_eq!(read(&mut bus, file.bytes.len()), file.bytes);
        }
    }
    #[test]
    fn directory_empty() {
        directory_case(0);
    }
    #[test]
    fn directory_one_file() {
        directory_case(1);
    }
    #[test]
    fn directory_three_files() {
        directory_case(3);
    }

    #[test]
    fn stream_advances_zero_fills_and_reselection_rewinds() {
        let mut bus = rig(vec![file("opt/test", b"abc")]);
        select(&mut bus, 0x20);
        assert_eq!(read(&mut bus, 1), b"a");
        assert_eq!(read(&mut bus, 4), b"bc\0\0");
        select(&mut bus, 0x20);
        assert_eq!(read(&mut bus, 3), b"abc");
    }

    #[test]
    fn unknown_and_legacy_keys_are_zero_without_aliasing_signature_or_id() {
        let mut bus = rig(vec![]);
        for key in (2..=0x18).chain([0x1A, 0x20, 0x3FFF, 0x8000, 0x8001, 0x8002, 0x8003, 0xFFFF]) {
            select(&mut bus, key);
            assert_eq!(read(&mut bus, 16), [0; 16], "key {key:04x}");
        }
        select(&mut bus, 0x4000);
        assert_eq!(read(&mut bus, 4), b"QEMU");
    }

    #[test]
    fn byte_writes_are_ignored_including_two_byte_selector_attempts() {
        let mut bus = rig(vec![]);
        select(&mut bus, 1);
        bus.port_write(0x510, 1, 0).unwrap();
        bus.port_write(0x511, 1, 0).unwrap();
        assert_eq!(read(&mut bus, 1), [1]);
        bus.port_write(0x511, 1, 0xAA).unwrap();
        assert_eq!(read(&mut bus, 1), [0]);
        select(&mut bus, 0);
        assert_eq!(bus.port_read(0x510, 1).unwrap(), u32::from(b'Q'));
        bus.port_write(0x511, 1, 0xFF).unwrap();
        assert_eq!(read(&mut bus, 3), b"EMU");
        select(&mut bus, 0x4000);
        bus.port_write(0x511, 1, 0xFF).unwrap();
        assert_eq!(read(&mut bus, 4), b"QEMU");
    }

    #[test]
    fn snapshot_round_trip_mid_file_and_reset() {
        let files = vec![file("opt/test", b"abc")];
        let mut bus = rig(files.clone());
        bus.tick(23);
        select(&mut bus, 0x20);
        assert_eq!(read(&mut bus, 1), b"a");
        let snapshot = bus.device().save();
        let mut restored = rig(files);
        restored.device_mut().restore(&snapshot).unwrap();
        assert_eq!(restored.device().save(), snapshot);
        assert_eq!(read(&mut restored, 3), b"bc\0");
        assert_eq!(read(&mut bus, 3), b"bc\0");
        restored.device_mut().reset();
        assert_eq!(read(&mut restored, 4), b"QEMU");
        for invalid in [vec![], vec![1; 18], vec![2; 19]] {
            let before = bus.device().save();
            assert!(bus.device_mut().restore(&invalid).is_err());
            assert_eq!(bus.device().save(), before);
        }
    }

    #[test]
    fn no_dma_mmio_irq_or_scheduled_work_and_invalid_widths_do_not_advance() {
        let mut bus = rig(vec![]);
        for port in [0x50F, 0x512, 0x514, 0x518] {
            assert!(bus.port_read(port, 1).is_err());
            assert!(bus.port_write(port, 1, 0).is_err());
        }
        for (port, size) in [(0x510, 4), (0x511, 2), (0x511, 4)] {
            assert!(bus.port_write(port, size, 0).is_err());
        }
        assert!(bus.port_read(0x510, 2).is_err());
        assert!(bus.mmio_read(0, 1).is_err());
        assert!(bus.mmio_write(0, 1, 0).is_err());
        assert!(!bus.device().irq_pending());
        assert_eq!(bus.device().next_event(VClock::ZERO), NextEvent::Dormant);
        assert_eq!(read(&mut bus, 4), b"QEMU");
    }

    #[test]
    fn file_names_are_checked_before_directory_encoding() {
        for name in ["", "a\0b", "é", &"x".repeat(56)] {
            assert_eq!(
                FwCfg::with_files(vec![file(name, &[])]).err(),
                Some(FwCfgError::InvalidName)
            );
        }
        assert_eq!(
            FwCfg::with_files(vec![file("same", &[]), file("same", &[])]).err(),
            Some(FwCfgError::InvalidName)
        );
        assert_eq!(
            FwCfg::with_files(vec![file("a", &[]); 16353]).err(),
            Some(FwCfgError::TooManyFiles)
        );
    }
}
