// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host playback of the mixed PCM seam; no hardware is modelled here.
//!
//! See `backlog/docs/design/audio.md` §Host output. Only the window explicitly
//! opens a device. Playback consumes completed virtual-time frames and cannot
//! change the scheduler, PIT clock or host-clock pacing policy.

use audio::{AudioBlock, AudioSink, StereoFrame, FRAMES_PER_SECOND};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, SampleFormat, SizedSample, SupportedStreamConfig, SupportedStreamConfigRange,
};
use std::{
    error::Error,
    io,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex,
    },
};

const CAPACITY: usize = FRAMES_PER_SECOND as usize / 5;
const TARGET: usize = FRAMES_PER_SECOND as usize / 20;
const MARGIN: usize = FRAMES_PER_SECOND as usize / 50;

struct Ring {
    frames: Box<[StereoFrame]>,
    head: usize,
    len: usize,
    primed: bool,
}

impl Ring {
    fn new(capacity: usize) -> Self {
        Self {
            frames: vec![[0; 2]; capacity].into_boxed_slice(),
            head: 0,
            len: 0,
            primed: false,
        }
    }

    fn discard(&mut self, count: usize) {
        self.head = (self.head + count) % self.frames.len();
        self.len -= count;
    }

    fn push(&mut self, frames: &[StereoFrame]) {
        let capacity = self.frames.len();
        let frames = &frames[frames.len().saturating_sub(capacity)..];
        self.discard((self.len + frames.len()).saturating_sub(capacity));
        let tail = (self.head + self.len) % capacity;
        let first = frames.len().min(capacity - tail);
        self.frames[tail..tail + first].copy_from_slice(&frames[..first]);
        self.frames[..frames.len() - first].copy_from_slice(&frames[first..]);
        self.len += frames.len();
        if self.len > (TARGET + MARGIN).min(capacity) {
            self.discard(self.len - TARGET.min(capacity));
        }
    }

    fn read(&mut self, output: &mut [StereoFrame]) -> usize {
        if !self.primed && self.len < TARGET.min(self.frames.len()) {
            return 0;
        }
        self.primed = true;
        let count = output.len().min(self.len);
        let first = count.min(self.frames.len() - self.head);
        output[..first].copy_from_slice(&self.frames[self.head..self.head + first]);
        output[first..count].copy_from_slice(&self.frames[..count - first]);
        self.discard(count);
        if count < output.len() {
            self.primed = false;
        }
        count
    }
}

/// These notifications leave the cpal stream usable; other errors are fatal.
fn transient_error_bit(kind: cpal::ErrorKind) -> Option<u8> {
    match kind {
        cpal::ErrorKind::Xrun => Some(1),
        cpal::ErrorKind::DeviceChanged => Some(2),
        cpal::ErrorKind::RealtimeDenied => Some(4),
        _ => None,
    }
}

fn record_stream_error(kind: cpal::ErrorKind, active: &AtomicBool, notices: &AtomicU8) {
    if let Some(bit) = transient_error_bit(kind) {
        notices.fetch_or(bit, Ordering::Relaxed);
    } else {
        active.store(false, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub(super) struct WindowAudioSink {
    ring: Arc<Mutex<Ring>>,
    active: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
}

impl WindowAudioSink {
    fn new(capacity: usize) -> Self {
        Self {
            ring: Arc::new(Mutex::new(Ring::new(capacity))),
            active: Arc::new(AtomicBool::new(false)),
            muted: Arc::new(AtomicBool::new(false)),
        }
    }

    fn read<T: SizedSample + FromSample<i16>>(&self, output: &mut [T], channels: usize) {
        output.fill(T::from_sample(0));
        if !self.enabled() {
            return;
        }
        let mut scratch = [[0; 2]; 1024];
        for chunk in output.chunks_mut(channels * scratch.len()) {
            let count = chunk.len() / channels;
            // Only two bounded PCM copies hold the mutex. Conversion and extra
            // host channels must not lengthen the producer's critical section.
            let copied = match self.ring.lock() {
                Ok(mut ring) => ring.read(&mut scratch[..count]),
                Err(_) => return,
            };
            for (frame, samples) in chunk.chunks_exact_mut(channels).zip(&scratch[..copied]) {
                frame[0] = T::from_sample(samples[0]);
                frame[1] = T::from_sample(samples[1]);
            }
            if copied < count {
                break;
            }
        }
    }

    fn push(&self, frames: &[StereoFrame]) {
        if self.enabled() {
            if let Ok(mut ring) = self.ring.lock() {
                ring.push(frames);
            }
        }
    }
}

impl AudioSink for WindowAudioSink {
    fn present(&mut self, block: AudioBlock<'_>) {
        self.push(block.frames());
    }

    fn enabled(&self) -> bool {
        self.active.load(Ordering::Relaxed) && !self.muted.load(Ordering::Relaxed)
    }
}

fn select_config(ranges: &[SupportedStreamConfigRange]) -> Option<SupportedStreamConfig> {
    [SampleFormat::I16, SampleFormat::F32]
        .into_iter()
        .find_map(|format| {
            ranges.iter().find_map(|range| {
                (range.channels() >= 2 && range.sample_format() == format)
                    .then(|| range.try_with_sample_rate(FRAMES_PER_SECOND))
                    .flatten()
            })
        })
}

pub(super) struct HostAudio {
    sink: WindowAudioSink,
    stream: Option<cpal::Stream>,
    attempted: bool,
    reported: bool,
    notices: Arc<AtomicU8>,
    reported_notices: u8,
    started: bool,
}

impl HostAudio {
    pub(super) fn new() -> Self {
        Self {
            sink: WindowAudioSink::new(CAPACITY),
            stream: None,
            attempted: false,
            reported: false,
            notices: Arc::new(AtomicU8::new(0)),
            reported_notices: 0,
            started: false,
        }
    }

    pub(super) fn sink(&self) -> WindowAudioSink {
        self.sink.clone()
    }

    pub(super) fn available(&self) -> bool {
        self.sink.active.load(Ordering::Relaxed)
    }

    pub(super) fn muted(&self) -> bool {
        self.sink.muted.load(Ordering::Relaxed)
    }

    pub(super) fn toggle_mute(&mut self) {
        if !self.available() {
            return;
        }
        self.sink.muted.store(!self.muted(), Ordering::Relaxed);
        if let Ok(mut ring) = self.sink.ring.lock() {
            ring.len = 0;
            ring.primed = false;
        }
    }

    pub(super) fn open(&mut self, warnings: &mut impl io::Write) {
        if self.attempted {
            return;
        }
        self.attempted = true;
        if let Err(error) = self.start() {
            self.sink.active.store(false, Ordering::Relaxed);
            let _ = writeln!(warnings, "audio disabled: {error}");
            self.reported = true;
        }
    }

    fn start(&mut self) -> Result<(), Box<dyn Error>> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no default output device")?;
        let ranges: Vec<_> = device.supported_output_configs()?.collect();
        let config = select_config(&ranges)
            .ok_or("48 kHz output with at least two channels (i16/f32) is unavailable")?;
        self.sink.active.store(true, Ordering::Relaxed);
        let stream = match config.sample_format() {
            SampleFormat::I16 => self.build_stream::<i16>(&device, config)?,
            _ => self.build_stream::<f32>(&device, config)?,
        };
        self.stream = Some(stream);
        Ok(())
    }

    fn build_stream<T: SizedSample + FromSample<i16>>(
        &self,
        device: &cpal::Device,
        config: SupportedStreamConfig,
    ) -> Result<cpal::Stream, cpal::Error> {
        let sink = self.sink.clone();
        let active = self.sink.active.clone();
        let notices = self.notices.clone();
        let channels = usize::from(config.channels());
        device.build_output_stream(
            config.config(),
            move |output: &mut [T], _| sink.read(output, channels),
            // Reporting and dropping the failed stream belong to the event loop,
            // so the callback neither allocates nor writes to stderr.
            move |error| record_stream_error(error.kind(), &active, &notices),
            None,
        )
    }

    pub(super) fn check_failure(&mut self, warnings: &mut impl io::Write) -> bool {
        let notices = self.notices.load(Ordering::Relaxed);
        for (bit, message) in [
            (1, "underrun or overrun"),
            (2, "device rerouted"),
            (4, "realtime scheduling denied"),
        ] {
            if notices & bit != 0 && self.reported_notices & bit == 0 {
                let _ = writeln!(warnings, "audio warning: {message}");
                self.reported_notices |= bit;
            }
        }
        if self.available() && !self.started {
            let ready = self.sink.ring.lock().is_ok_and(|ring| ring.len >= TARGET);
            if ready {
                if let Some(stream) = &self.stream {
                    if stream.play().is_err() {
                        self.sink.active.store(false, Ordering::Relaxed);
                    } else {
                        self.started = true;
                    }
                }
            }
        }
        if self.attempted && !self.reported && !self.available() {
            self.stream = None;
            let _ = writeln!(warnings, "audio disabled: output stream failed");
            self.reported = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests;
