// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native SB16 PCM observation adapter (Creative Sound Blaster Series Hardware
//! Programming Guide, §3 PCM Data Formats). See `backlog/docs/design/audio.md`
//! §SB16 8-bit output and §SB16 16-bit output. Hardware owns deadlines; this
//! adapter owns history only.

use std::{cell::RefCell, num::NonZeroU64, rc::Rc};

use audio::{AudioError, AudioSource, RateSource, SampleArea, SampleBuffer, SamplePeriod};
use device::VClock;
use devices::{sb16::Output, Sb16};

struct Signal {
    reader: RateSource,
    samples: SampleBuffer,
    error: Option<AudioError>,
    enabled: bool,
}

struct Source(Rc<RefCell<Signal>>);

impl AudioSource for Source {
    fn set_output_enabled(&mut self, enabled: bool) {
        self.0.borrow_mut().enabled = enabled;
    }
    fn take_error(&mut self) -> Option<AudioError> {
        self.0.borrow_mut().error.take()
    }
    fn render(&mut self, start: VClock, end: VClock) -> SampleArea {
        self.0.borrow_mut().reader.render(start, end)
    }
    fn advance(&mut self, until: VClock) {
        self.0.borrow_mut().reader.advance(until);
    }
    fn release_before(&mut self, at: VClock) {
        self.0.borrow_mut().reader.release_before(at);
    }
    fn finished(&self) -> bool {
        self.0.borrow().reader.finished()
    }
}

fn signal(base: u16, dma8: u8, dma16: u8, irq: u8) -> (Sb16, Source) {
    let samples = SampleBuffer::default();
    let signal = Rc::new(RefCell::new(Signal {
        reader: RateSource::new(samples.clone()),
        samples,
        error: None,
        enabled: true,
    }));
    let writer = signal.clone();
    let mut device = Sb16::new(base, dma8).with_resources(dma16, irq);
    device.observe_output(move |event| {
        let mut signal = writer.borrow_mut();
        let (at, result) = match event {
            Output::Frame {
                at,
                numerator,
                denominator,
                pcm,
            } => {
                let result = match (NonZeroU64::new(numerator), NonZeroU64::new(denominator)) {
                    (Some(p), Some(q)) => {
                        signal
                            .samples
                            .push_segment(at, SamplePeriod::new(p, q), &[pcm])
                    }
                    _ => Err(AudioError::HistoryOrder),
                };
                (at, result)
            }
            Output::Pause(at) => (at, signal.samples.pause(at)),
        };
        if let Err(error) = result {
            signal.error.get_or_insert(error);
        }
        if !signal.enabled {
            // Retain the incomplete output frame even during a long run without
            // facade observations; enabling capture cannot recover discarded PCM.
            let frames = audio::completed_frames(at);
            let retain = (u128::from(frames) * u128::from(audio::CLOCKS_PER_FRAME_NUMERATOR))
                .div_ceil(u128::from(audio::CLOCKS_PER_FRAME_DENOMINATOR))
                as u64;
            signal.reader.release_before(VClock(retain));
        }
    });
    (device, Source(signal))
}

pub(crate) fn build(base: u16, dma8: u8, dma16: u8, irq: u8) -> (Sb16, Box<dyn AudioSource>) {
    let (device, source) = signal(base, dma8, dma16, irq);
    (device, Box::new(source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use device::Device;

    fn unity_signal() -> (Sb16, Source) {
        let (mut dsp, source) = signal(0x220, 1, 5, 5);
        for index in 0x30..=0x33 {
            dsp.port_write(0x224, 1, index).unwrap();
            dsp.port_write(0x225, 1, 0xF8).unwrap();
        }
        (dsp, source)
    }

    fn command(dsp: &mut Sb16, bytes: &[u8]) {
        for &byte in bytes {
            dsp.port_write(0x22C, 1, byte.into()).unwrap();
        }
    }

    fn dma_frame(dsp: &mut Sb16, at: u64, byte: u8) {
        dsp.tick(VClock(at));
        assert!(dsp.take_dma_request().is_some());
        dsp.dma_complete(device::DmaOutcome::FromMemory {
            bytes: vec![byte],
            terminal_count: false,
        });
    }

    #[test]
    fn paused_rate_change_holds_the_resumed_level_until_the_preserved_boundary() {
        let (mut dsp, mut source) = unity_signal();
        command(&mut dsp, &[0x41, 0x1F, 0x40, 0x14, 2, 0]);
        dma_frame(&mut dsp, 150, 0xFF);
        dsp.tick(VClock(180));
        command(&mut dsp, &[0xD0]);
        dsp.tick(VClock(200));
        command(&mut dsp, &[0x41, 0x3E, 0x80]);
        dsp.tick(VClock(400));
        command(&mut dsp, &[0xD4]);
        assert_eq!(source.render(VClock(400), VClock(519)), [32512 * 119; 2]);
        dma_frame(&mut dsp, 519, 0x81);
        assert_eq!(source.render(VClock(519), VClock(594)), [256 * 75; 2]);
        assert!(source.take_error().is_none());
    }

    #[test]
    fn chained_dma_blocks_preserve_the_full_last_sample_area() {
        let (mut dsp, mut source) = unity_signal();
        command(&mut dsp, &[0x41, 0x1F, 0x40, 0x14, 1, 0]);
        dma_frame(&mut dsp, 150, 0x80);
        dma_frame(&mut dsp, 299, 0x81);
        dsp.tick(VClock(310));
        dsp.port_read(0x22E, 1).unwrap();
        command(&mut dsp, &[0x14, 1, 0]);
        assert_eq!(source.render(VClock(299), VClock(448)), [256 * 149; 2]);
        dma_frame(&mut dsp, 448, 0xFF);
        assert_eq!(source.render(VClock(448), VClock(597)), [32512 * 149; 2]);
        assert!(source.take_error().is_none());
    }

    #[test]
    fn live_rate_change_preserves_the_held_sample_area() {
        let (mut dsp, mut source) = unity_signal();
        command(&mut dsp, &[0x41, 0x1F, 0x40, 0x14, 2, 0]);
        dma_frame(&mut dsp, 150, 0xFF);
        dsp.tick(VClock(200));
        command(&mut dsp, &[0x41, 0x3E, 0x80]);
        assert_eq!(source.render(VClock(200), VClock(299)), [32512 * 99; 2]);
        dma_frame(&mut dsp, 299, 0x81);
        assert_eq!(source.render(VClock(299), VClock(374)), [256 * 75; 2]);
        assert!(source.take_error().is_none());
    }

    #[test]
    fn same_clock_direct_dac_writes_replace_zero_duration_history() {
        let (mut dsp, mut source) = unity_signal();
        source.set_output_enabled(false);
        dsp.tick(VClock(7));
        for _ in 0..10000 {
            command(&mut dsp, &[0x10, 0xFF]);
        }
        assert_eq!(source.0.borrow().samples.buffered_segments(), 1);
        assert_eq!(source.0.borrow().samples.buffered_frames(), 1);
        assert_eq!(source.render(VClock(0), VClock(25)), [32512 * 18; 2]);
    }

    #[test]
    fn direct_level_pause_and_rate_frames_integrate_in_one_source() {
        let (mut dsp, mut source) = unity_signal();
        dsp.tick(VClock(7));
        command(&mut dsp, &[0x10, 0xFF]);
        dsp.tick(VClock(13));
        command(&mut dsp, &[0x10, 0]);
        assert_eq!(
            source.render(VClock(0), VClock(25)),
            [32512 * 6 - 32768 * 12; 2]
        );
        source.advance(VClock(25));
        source.release_before(VClock(25));
        dsp.tick(VClock(29));
        dsp.reset();
        assert_eq!(source.render(VClock(25), VClock(50)), [-32768 * 4; 2]);
        assert!(!source.finished());
        assert!(source.take_error().is_none());
    }

    #[test]
    fn no_consumer_history_is_bounded_and_late_history_errors_surface() {
        let (mut dsp, mut source) = unity_signal();
        source.set_output_enabled(false);
        for at in 1..10000 {
            dsp.tick(VClock(at));
            command(&mut dsp, &[0x10, at as u8]);
        }
        assert!(source.0.borrow().samples.buffered_segments() <= 26);
        assert!(source.0.borrow().samples.buffered_frames() <= 26);
        assert!(source.take_error().is_none());
        source.advance(VClock(20000));
        command(&mut dsp, &[0x10, 0]);
        assert_eq!(source.take_error(), Some(AudioError::HistoryOrder));
        dsp.tick(VClock(20001));
        command(&mut dsp, &[0x10, 0xFF]);
        assert_eq!(source.render(VClock(20001), VClock(20002)), [32512; 2]);
        assert!(source.take_error().is_none());
    }
}
