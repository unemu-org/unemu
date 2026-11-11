// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host failure isolation on MockBus; NEC µPD765 WRITE DATA, Multi-Track and
//! Status Register 1 (Overrun). See persistent-media.md §Host errors and determinism.
//! This drives a test-local media adapter; the production assembly adapter is
//! covered by persistence.rs::failed_output_preserves_complete_guest_prefix_and_final_media.

use device::{DmaOp, DmaOutcome, InMemoryMedia, SectorMedia, SectorWrite, TracePolicy};
use devices::{Drive, FloppyGeometry, Upd765};
use media::{JournalMedia, SinkStatus};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use testkit::MockBus;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "media-device-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("test directory: {error}"),
            }
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

struct JournalAdapter(JournalMedia);
impl SectorMedia for JournalAdapter {
    fn sector_count(&self) -> u64 {
        (self.0.image().len() / 512) as u64
    }
    fn read_sector(&self, lba: u64) -> [u8; 512] {
        self.0.read_sector(lba)
    }
    fn write_batch(&mut self, sectors: &[SectorWrite]) {
        self.0
            .write_batch(&sectors.iter().map(|s| (s.lba, s.bytes)).collect::<Vec<_>>());
    }
    fn write_protected(&self) -> bool {
        self.0.write_protected()
    }
}

fn command(bus: &mut MockBus, bytes: &[u8]) {
    for &byte in bytes {
        bus.port_write(0x3F5, 1, byte.into()).unwrap();
    }
}
fn result(bus: &mut MockBus, count: usize) -> Vec<u8> {
    (0..count)
        .map(|_| bus.port_read(0x3F5, 1).unwrap() as u8)
        .collect()
}
fn ready(bus: &mut MockBus) {
    bus.port_write(0x3F2, 1, 0x1C).unwrap();
    for _ in 0..4 {
        command(bus, &[8]);
        result(bus, 2);
    }
    bus.device_mut()
        .set_trace_policy(TracePolicy::Ring { capacity: 4096 });
}
fn compare(a: &mut MockBus, b: &mut MockBus) {
    assert_eq!(a.device().save(), b.device().save());
    assert_eq!(a.device().irq_pending(), b.device().irq_pending());
    assert_eq!(a.clock(), b.clock());
    assert_eq!(a.device().trace_dropped(), 0);
    assert_eq!(b.device().trace_dropped(), 0);
    assert_eq!(a.device_mut().take_trace(), b.device_mut().take_trace());
}

#[test]
fn failing_sink_on_mock_bus_keeps_dma_results_irq_padding_and_reset_state() {
    let temp = Temp::new();
    let directory = &temp.0;
    let base = vec![0; FloppyGeometry::DD360.image_bytes()];
    let status = SinkStatus::default();
    let journal = JournalMedia::open(
        &directory.join("disk.upfj"),
        &base,
        base.len() as u64,
        status.clone(),
    )
    .unwrap()
    .with_failing_output();
    let drive = |medium| {
        Box::new(Upd765::with_drives([
            Some(Drive::with_media(FloppyGeometry::DD360, medium)),
            None,
            None,
            None,
        ]))
    };
    let mut a = MockBus::new(drive(Box::new(JournalAdapter(journal))));
    let mut b = MockBus::new(drive(Box::new(InMemoryMedia::new(base, 720, false))));
    ready(&mut a);
    ready(&mut b);
    for (opcode, supplied, tc, value, expected) in [
        (5, 17, true, 0x37, [0, 0]),
        (0x85, 512 + 17, false, 0xA9, [0x44, 0x10]),
    ] {
        for bus in [&mut a, &mut b] {
            command(bus, &[opcode, 0, 0, 0, 2, 2, 2, 0x1B, 0xFF]);
            let request = bus.device_mut().take_dma_request().unwrap();
            assert_eq!(request.channel, 2);
            assert!(matches!(request.op, DmaOp::FromMemory { .. }));
            bus.device_mut().dma_complete(DmaOutcome::FromMemory {
                bytes: vec![value; supplied],
                terminal_count: tc,
            });
            assert!(bus.device().irq_pending());
            assert_eq!(&result(bus, 7)[..2], &expected);
            assert!(!bus.device().irq_pending());
        }
        compare(&mut a, &mut b);
    }
    let failure = status.failure().unwrap();
    assert_eq!(failure.last_confirmed_sequence, 0);
    a.device_mut().reset();
    b.device_mut().reset();
    ready(&mut a);
    ready(&mut b);
    compare(&mut a, &mut b);
    for (head, sector, supplied) in [(0, 2, 512), (1, 1, 17)] {
        for bus in [&mut a, &mut b] {
            command(bus, &[6, head << 2, 0, head, sector, 2, sector, 0x1B, 0xFF]);
            let DmaOp::ToMemory(bytes) = bus.device_mut().take_dma_request().unwrap().op else {
                panic!("READ DATA must supply media bytes");
            };
            let mut expected = vec![0; 512];
            expected[..supplied].fill(0xA9);
            assert_eq!(bytes, expected);
            bus.device_mut().dma_complete(DmaOutcome::ToMemory {
                transferred: 512,
                terminal_count: true,
            });
            assert_eq!(&result(bus, 7)[..2], &[head << 2, 0]);
        }
        compare(&mut a, &mut b);
    }
    assert_eq!(status.failure(), Some(failure));
    assert_eq!(
        std::fs::metadata(directory.join("disk.upfj"))
            .unwrap()
            .len(),
        media::HEADER_LEN as u64
    );
    drop(a);
    drop(b);
}
