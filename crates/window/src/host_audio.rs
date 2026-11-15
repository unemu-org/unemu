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

/// Linear interpolation carries an exact rational position across input blocks.
/// Only the producer uses this state; see audio.md §Host output for quality limits.
struct RateConverter {
    rate: u32,
    previous: Option<StereoFrame>,
    phase: u64,
}

impl RateConverter {
    fn new(rate: u32) -> Self {
        Self {
            rate,
            previous: None,
            phase: 0,
        }
    }

    fn process(&mut self, frames: &[StereoFrame], mut emit: impl FnMut(StereoFrame)) {
        let rate = u64::from(self.rate);
        for &current in frames {
            if let Some(previous) = self.previous {
                while self.phase <= rate {
                    let frame = std::array::from_fn(|channel| {
                        let left = i64::from(previous[channel]);
                        let right = i64::from(current[channel]);
                        (left + (right - left) * self.phase as i64 / i64::from(self.rate)) as i16
                    });
                    emit(frame);
                    self.phase += u64::from(FRAMES_PER_SECOND);
                }
                self.phase -= rate;
            } else {
                emit(current);
                self.phase = u64::from(FRAMES_PER_SECOND);
            }
            self.previous = Some(current);
        }
    }
}

struct Ring {
    frames: Box<[StereoFrame]>,
    head: usize,
    len: usize,
    primed: bool,
    target: usize,
    trim_threshold: usize,
}

impl Ring {
    fn new(capacity: usize, rate: u32) -> Self {
        Self {
            frames: vec![[0; 2]; capacity].into_boxed_slice(),
            head: 0,
            len: 0,
            primed: false,
            target: (rate as usize / 20).min(capacity),
            trim_threshold: (rate as usize * 70 / 1000).min(capacity),
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
        if self.len > self.trim_threshold {
            self.discard(self.len - self.target);
        }
    }

    fn read(&mut self, output: &mut [StereoFrame]) -> usize {
        if !self.primed && self.len < self.target {
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
    converter: Arc<Mutex<RateConverter>>,
    active: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
}

impl WindowAudioSink {
    fn new(capacity: usize, rate: u32) -> Self {
        Self {
            ring: Arc::new(Mutex::new(Ring::new(capacity, rate))),
            converter: Arc::new(Mutex::new(RateConverter::new(rate))),
            active: Arc::new(AtomicBool::new(false)),
            muted: Arc::new(AtomicBool::new(false)),
        }
    }

    fn configure(&self, rate: u32) {
        if let Ok(mut converter) = self.converter.lock() {
            *converter = RateConverter::new(rate);
            if let Ok(mut ring) = self.ring.lock() {
                *ring = Ring::new(rate as usize / 5, rate);
            }
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
        if !self.enabled() {
            return;
        }
        if let Ok(mut converter) = self.converter.lock() {
            if converter.rate == FRAMES_PER_SECOND {
                if let Ok(mut ring) = self.ring.lock() {
                    ring.push(frames);
                }
                return;
            }
            let mut scratch = [[0; 2]; 1024];
            let mut len = 0;
            converter.process(frames, |frame| {
                scratch[len] = frame;
                len += 1;
                if len == scratch.len() {
                    if let Ok(mut ring) = self.ring.lock() {
                        ring.push(&scratch);
                    }
                    len = 0;
                }
            });
            if let Ok(mut ring) = self.ring.lock() {
                ring.push(&scratch[..len]);
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

fn select_config(
    ranges: &[SupportedStreamConfigRange],
    default: Option<&SupportedStreamConfig>,
) -> Option<SupportedStreamConfig> {
    let supported = |channels, rate, format| {
        channels >= 2 && rate > 0 && matches!(format, SampleFormat::I16 | SampleFormat::F32)
    };
    [SampleFormat::I16, SampleFormat::F32]
        .into_iter()
        .find_map(|format| {
            ranges.iter().find_map(|range| {
                (range.channels() >= 2 && range.sample_format() == format)
                    .then(|| range.try_with_sample_rate(FRAMES_PER_SECOND))
                    .flatten()
            })
        })
        .or_else(|| {
            default
                .filter(|config| {
                    supported(
                        config.channels(),
                        config.sample_rate(),
                        config.sample_format(),
                    )
                })
                .cloned()
        })
        .or_else(|| {
            ranges
                .iter()
                .filter(|range| {
                    supported(
                        range.channels(),
                        range.min_sample_rate(),
                        range.sample_format(),
                    )
                })
                .map(|range| {
                    range.with_sample_rate(
                        FRAMES_PER_SECOND.clamp(range.min_sample_rate(), range.max_sample_rate()),
                    )
                })
                .min_by_key(|config| {
                    (
                        config.sample_rate().abs_diff(FRAMES_PER_SECOND),
                        u8::from(config.sample_format() != SampleFormat::I16),
                        config.channels(),
                        std::cmp::Reverse(config.sample_rate()),
                    )
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
    user_muted: bool,
    speed_muted: bool,
}

impl HostAudio {
    pub(super) fn new() -> Self {
        Self {
            sink: WindowAudioSink::new(FRAMES_PER_SECOND as usize / 5, FRAMES_PER_SECOND),
            stream: None,
            attempted: false,
            reported: false,
            notices: Arc::new(AtomicU8::new(0)),
            reported_notices: 0,
            started: false,
            user_muted: false,
            speed_muted: false,
        }
    }

    pub(super) fn sink(&self) -> WindowAudioSink {
        self.sink.clone()
    }

    #[cfg(test)]
    pub(super) fn activate_for_test(&self) {
        self.sink.active.store(true, Ordering::Relaxed);
    }

    /// Everything the host callback would play next, ignoring the prefill wait.
    #[cfg(test)]
    pub(super) fn drain_for_test(&self) -> Vec<StereoFrame> {
        let mut ring = self.sink.ring.lock().unwrap();
        let mut frames = vec![[0; 2]; ring.len];
        ring.primed = true;
        let count = ring.read(&mut frames);
        frames.truncate(count);
        frames
    }

    pub(super) fn available(&self) -> bool {
        self.sink.active.load(Ordering::Relaxed)
    }

    pub(super) fn muted(&self) -> bool {
        self.sink.muted.load(Ordering::Relaxed)
    }

    /// Whether the guest's sound reaches the speakers: a device is active and
    /// neither mute holds it back.
    pub(super) fn audible(&self) -> bool {
        self.available() && !self.speed_muted && !self.user_muted
    }

    pub(super) fn status_text(&self) -> &'static str {
        if !self.available() {
            "Audio unavailable"
        } else if self.speed_muted && self.user_muted {
            "Audio muted (speed + user)"
        } else if self.speed_muted {
            "Audio muted (speed)"
        } else if self.user_muted {
            "Audio muted (user)"
        } else {
            "Audio on"
        }
    }

    pub(super) fn mute_label(&self) -> &'static str {
        if self.speed_muted && self.user_muted {
            "User mute: on (speed muted)"
        } else if self.speed_muted {
            "User mute: off (speed muted)"
        } else if self.user_muted {
            "Unmute audio"
        } else {
            "Mute audio"
        }
    }

    pub(super) fn toggle_mute(&mut self) {
        if !self.available() {
            return;
        }
        self.user_muted = !self.user_muted;
        self.update_mute();
    }

    pub(super) fn set_speed_muted(&mut self, muted: bool) {
        self.speed_muted = muted;
        self.update_mute();
    }

    fn update_mute(&mut self) {
        let muted = self.user_muted || self.speed_muted;
        if self.muted() == muted {
            return;
        }
        // Keep callbacks silent until old samples and interpolation history are gone.
        self.sink.muted.store(true, Ordering::Relaxed);
        if let Ok(mut converter) = self.sink.converter.lock() {
            *converter = RateConverter::new(converter.rate);
            if let Ok(mut ring) = self.sink.ring.lock() {
                ring.len = 0;
                ring.primed = false;
            }
        }
        self.sink.muted.store(muted, Ordering::Relaxed);
    }

    pub(super) fn open(&mut self, warnings: &mut impl io::Write) {
        if self.attempted {
            return;
        }
        self.attempted = true;
        if let Err(error) = self.start(warnings) {
            self.sink.active.store(false, Ordering::Relaxed);
            let _ = writeln!(warnings, "audio disabled: {error}");
            self.reported = true;
        }
    }

    fn start(&mut self, warnings: &mut impl io::Write) -> Result<(), Box<dyn Error>> {
        // The shell tests start real machines, whose BIOS plays its POST
        // melody: on a developer's machine that came out of the speakers at
        // random moments during a test run. Tests behave as a host without
        // an output device does.
        if cfg!(test) {
            return Err("host audio output is not opened in tests".into());
        }
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no default output device")?;
        let ranges: Vec<_> = device.supported_output_configs()?.collect();
        let default = device.default_output_config().ok();
        let config = select_config(&ranges, default.as_ref())
            .ok_or("output with at least two channels (i16/f32) is unavailable")?;
        let rate = config.sample_rate();
        self.sink.configure(rate);
        self.sink.active.store(true, Ordering::Relaxed);
        let stream = match config.sample_format() {
            SampleFormat::I16 => self.build_stream::<i16>(&device, config)?,
            _ => self.build_stream::<f32>(&device, config)?,
        };
        self.stream = Some(stream);
        if rate != FRAMES_PER_SECOND {
            let name = device
                .description()
                .map(|description| description.name().to_owned())
                .unwrap_or_else(|_| "default output device".to_owned());
            let _ = writeln!(
                warnings,
                "audio: converting {FRAMES_PER_SECOND} Hz to {rate} Hz for {name}"
            );
        }
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
            let ready = self
                .sink
                .ring
                .lock()
                .is_ok_and(|ring| ring.len >= ring.target);
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
