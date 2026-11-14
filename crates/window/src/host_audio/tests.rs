// SPDX-License-Identifier: AGPL-3.0-or-later
//! Playback tests use only memory; they never open a host device.

use super::*;
use cpu::Cpu;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

const CAPACITY: usize = FRAMES_PER_SECOND as usize / 5;
const TARGET: usize = FRAMES_PER_SECOND as usize / 20;
const MARGIN: usize = FRAMES_PER_SECOND as usize / 50;

struct CountingAllocator;
thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| {
            if let Some(n) = count.get() {
                count.set(Some(n + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
// Counting is opt-in and thread-local; all other tests still allocate through
// System without contributing to this test's counter.
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn active_sink(capacity: usize) -> WindowAudioSink {
    let sink = WindowAudioSink::new(capacity, FRAMES_PER_SECOND);
    sink.active.store(true, Ordering::Relaxed);
    sink.ring.lock().unwrap().primed = true;
    sink
}

#[test]
fn underrun_fills_every_sample_with_silence() {
    let sink = active_sink(3);
    sink.push(&[[12, -34]]);
    let mut output = [99i16; 6];
    sink.read(&mut output, 2);
    assert_eq!(output, [12, -34, 0, 0, 0, 0]);
}

#[test]
fn overrun_drops_oldest_complete_frames_in_order() {
    let sink = active_sink(3);
    sink.push(&[[1, -1], [2, -2]]);
    sink.push(&[[3, -3], [4, -4], [5, -5], [6, -6]]);
    let mut output = [0i16; 6];
    sink.read(&mut output, 2);
    assert_eq!(output, [4, -4, 5, -5, 6, -6]);
}

#[test]
fn wraparound_preserves_exact_stereo_order() {
    let sink = active_sink(3);
    sink.push(&[[1, 2], [3, 4]]);
    let mut first = [0i16; 2];
    sink.read(&mut first, 2);
    assert_eq!(first, [1, 2]);
    sink.push(&[[5, 6], [7, 8]]);
    let mut output = [0i16; 6];
    sink.read(&mut output, 2);
    assert_eq!(output, [3, 4, 5, 6, 7, 8]);
}

#[test]
fn callback_does_not_allocate_for_data_or_silence() {
    let sink = active_sink(3);
    sink.push(&[[1, 2], [3, 4]]);
    let mut integer = [0i16; 8];
    let mut float = [0.0f32; 8];
    ALLOCATIONS.with(|n| n.set(Some(0)));
    sink.read(&mut integer, 2);
    sink.push(&[[16384, -16384]; 3]);
    sink.read(&mut float, 2);
    sink.read(&mut integer, 2);
    let count = ALLOCATIONS.with(|n| n.replace(None));
    assert_eq!(count, Some(0));
    assert_eq!(integer, [0i16; 8]);
    assert_eq!(float, [0.5, -0.5, 0.5, -0.5, 0.5, -0.5, 0.0, 0.0]);
}

#[test]
fn sink_adapter_preserves_i16_and_normalizes_f32() {
    // One complete frame ends at PIT clock 25; obtain the clock type through
    // assembly so the frontend needs no additional device dependency.
    let machine = assembly::assemble(&assembly::at_boot(vec![0xF4])).unwrap();
    let start = machine.machine.clock();
    let frames = [[i16::MIN, i16::MAX], [-16384, 16384]];
    let mut end = start;
    end.0 += 50;
    let block = AudioBlock::new(start, end, &frames).unwrap();
    let mut sink = active_sink(3);
    sink.present(block);
    let mut integer = [0i16; 4];
    sink.read(&mut integer, 2);
    assert_eq!(integer, [-32768, 32767, -16384, 16384]);
    sink.present(block);
    let mut float = [0.0f32; 4];
    sink.read(&mut float, 2);
    assert_eq!(float, [-1.0, 32767.0 / 32768.0, -0.5, 0.5]);
}

fn range(channels: u16, rate: u32, format: SampleFormat) -> SupportedStreamConfigRange {
    SupportedStreamConfigRange::new(
        channels,
        rate,
        rate,
        cpal::SupportedBufferSize::Unknown,
        format,
    )
}

#[test]
fn config_prefers_i16_then_f32_at_48_khz_with_stereo_or_more() {
    let float = range(2, 48000, SampleFormat::F32);
    let integer = range(2, 48000, SampleFormat::I16);
    assert_eq!(
        select_config(&[float, integer], None)
            .unwrap()
            .sample_format(),
        SampleFormat::I16
    );
    assert_eq!(
        select_config(&[float], None).unwrap().sample_format(),
        SampleFormat::F32
    );
    assert!(select_config(
        &[
            range(2, 48000, SampleFormat::U16),
            range(1, 48000, SampleFormat::F32)
        ],
        None,
    )
    .is_none());
}

#[test]
fn construction_is_inert_without_a_device_and_mute_discards_pending_audio() {
    let mut host = HostAudio::new();
    assert!(!host.attempted);
    assert!(host.stream.is_none());
    assert!(!host.sink.enabled());
    host.sink.push(&[[1, 2]]);
    assert_eq!(host.sink.ring.lock().unwrap().len, 0);
    host.sink.active.store(true, Ordering::Relaxed);
    host.sink.push(&[[3, 4]]);
    host.toggle_mute();
    assert!(!host.sink.enabled());
    host.toggle_mute();
    let mut output = [99i16; 2];
    host.sink.read(&mut output, 2);
    assert_eq!(output, [0, 0]);
}

#[test]
fn stream_failure_disables_sink_and_reports_once_on_host_thread() {
    let mut host = HostAudio::new();
    host.attempted = true;
    host.sink.active.store(true, Ordering::Relaxed);
    host.sink.push(&[[1, 2]]);
    // The real error callback performs only this atomic store.
    host.sink.active.store(false, Ordering::Relaxed);
    let mut warnings = Vec::new();
    host.check_failure(&mut warnings);
    host.check_failure(&mut warnings);
    assert_eq!(warnings, b"audio disabled: output stream failed\n");
    assert!(!host.sink.enabled());
    let mut output = [99i16; 2];
    host.sink.read(&mut output, 2);
    assert_eq!(output, [0, 0]);
}

fn presenter() -> crate::Presenter {
    let mut rom = vec![0u8; 0x8000];
    rom[..2].copy_from_slice(&[0xEB, 0xFE]);
    let mut config = assembly::bios(rom, None);
    config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
    crate::Presenter::new(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
        &mut io::sink(),
    )
    .unwrap()
}

#[test]
fn presenter_wiring_is_inert_until_enabled_and_captures_each_run_boundary() {
    for rate in [48000, 44100, 16000] {
        let mut silent = presenter();
        let mut playing = presenter();
        playing.audio.sink.configure(rate);
        assert!(!playing.audio.attempted);
        assert!(playing.audio.stream.is_none());
        assert!(!playing.audio.sink.enabled());
        for host in [&mut silent, &mut playing] {
            host.assembled
                .add_audio_source(Box::new(audio::EdgeSource::new([1234, -2345])));
        }
        // Simulate a consumer without enumerating or opening a physical device.
        playing.audio.sink.active.store(true, Ordering::Relaxed);
        for millis in [20, 40, 60] {
            silent
                .advance_fully(std::time::Duration::from_millis(millis))
                .unwrap();
            playing
                .advance_fully(std::time::Duration::from_millis(millis))
                .unwrap();
            let clock = playing.assembled.machine.clock();
            assert_eq!(silent.assembled.machine.clock(), clock);
            assert_eq!(
                silent.assembled.machine.virtual_time(),
                playing.assembled.machine.virtual_time()
            );
            assert_eq!(
                silent.assembled.machine.cpu().retired(),
                playing.assembled.machine.cpu().retired()
            );
            let ring = playing.audio.sink.ring.lock().unwrap();
            let input_frames = audio::completed_frames(clock);
            let expected = (input_frames - 1) * u64::from(rate) / 48000 + 1;
            assert_eq!(ring.len, expected as usize);
            assert_eq!(silent.audio.sink.ring.lock().unwrap().len, 0);
        }
        let mut output = [0i16; 2];
        playing.audio.sink.read(&mut output, 2);
        assert_eq!(output, [1234, -2345]);
    }
}

#[test]
fn mute_chord_is_host_only_and_f12_reaches_the_guest() {
    use winit::{
        event::ElementState::{Pressed, Released},
        keyboard::KeyCode::*,
    };
    let mut host = presenter();
    assert!(host.title().contains("no audio"));
    assert!(!host.title().contains("ctrl+alt+m"));
    host.audio.sink.active.store(true, Ordering::Relaxed);
    host.key_event(ControlLeft, Pressed);
    host.key_event(AltRight, Pressed);
    let pending = host.keyboard.pending_events();
    host.key_event(KeyM, Pressed);
    host.key_event(KeyM, Pressed);
    assert!(host.audio.muted());
    assert!(host.title().contains("audio muted"));
    host.key_event(KeyM, Released);
    assert_eq!(host.keyboard.pending_events(), pending);
    host.key_event(KeyM, Pressed);
    host.key_event(KeyM, Released);
    assert!(!host.audio.muted());
    host.key_event(F12, Pressed);
    host.key_event(F12, Released);
    assert_eq!(host.keyboard.pending_events(), pending + 2);
    host.audio.sink.active.store(false, Ordering::Relaxed);
    host.key_event(KeyM, Pressed);
    host.key_event(KeyM, Released);
    assert_eq!(host.keyboard.pending_events(), pending + 4);
}

#[test]
fn held_guest_m_is_not_stolen_when_modifiers_arrive_later() {
    use winit::{
        event::ElementState::{Pressed, Released},
        keyboard::KeyCode::*,
    };
    let mut host = presenter();
    host.audio.sink.active.store(true, Ordering::Relaxed);
    host.key_event(KeyM, Pressed);
    host.key_event(ControlRight, Pressed);
    host.key_event(AltLeft, Pressed);
    host.key_event(KeyM, Pressed);
    host.key_event(KeyM, Released);
    assert!(!host.audio.muted());
    assert_eq!(host.keyboard.pending_events(), 5);
    host.focus_changed(false);
    assert!(!host.mute_modifiers.contains(&true));
    assert!(!host.mute_key_held);
}

#[test]
fn transient_cpal_errors_keep_stream_active_and_log_each_kind_once() {
    use cpal::ErrorKind::*;
    let mut host = HostAudio::new();
    host.attempted = true;
    host.sink.active.store(true, Ordering::Relaxed);
    let mut warnings = Vec::new();
    for _ in 0..2 {
        for kind in [Xrun, DeviceChanged, RealtimeDenied] {
            assert!(transient_error_bit(kind).is_some());
            record_stream_error(kind, &host.sink.active, &host.notices);
            host.check_failure(&mut warnings);
            assert!(host.available());
        }
    }
    assert_eq!(String::from_utf8(warnings).unwrap().lines().count(), 3);
    for kind in [
        DeviceBusy,
        DeviceNotAvailable,
        HostUnavailable,
        InvalidInput,
        PermissionDenied,
        ResourceExhausted,
        StreamInvalidated,
        UnsupportedConfig,
        UnsupportedOperation,
        BackendError,
        Other,
    ] {
        assert_eq!(transient_error_bit(kind), None);
        host.sink.active.store(true, Ordering::Relaxed);
        record_stream_error(kind, &host.sink.active, &host.notices);
        assert!(!host.available());
    }
}

#[test]
fn prefill_and_latency_trim_keep_recent_audio_near_fifty_ms() {
    let sink = WindowAudioSink::new(CAPACITY, FRAMES_PER_SECOND);
    sink.active.store(true, Ordering::Relaxed);
    let mut output = [99i16; 2];
    sink.push(&vec![[1, -1]; TARGET - 1]);
    sink.read(&mut output, 2);
    assert_eq!(output, [0, 0]);
    assert_eq!(sink.ring.lock().unwrap().len, TARGET - 1);
    sink.push(&[[2, -2]]);
    sink.read(&mut output, 2);
    assert_eq!(output, [1, -1]);
    sink.push(&vec![[3, -3]; TARGET + MARGIN + 1]);
    assert_eq!(sink.ring.lock().unwrap().len, TARGET);
    sink.read(&mut output, 2);
    assert_eq!(output, [3, -3]);
    let mut drain = vec![0i16; TARGET * 2];
    sink.read(&mut drain, 2);
    sink.push(&[[4, -4]]);
    sink.read(&mut output, 2);
    assert_eq!(output, [0, 0], "an underrun rearms prefill");
}

#[test]
fn multichannel_devices_keep_stereo_first_and_zero_extra_channels() {
    let config = select_config(&[range(6, 48000, SampleFormat::F32)], None).unwrap();
    assert_eq!(config.channels(), 6);
    let sink = active_sink(3);
    sink.push(&[[i16::MIN, 16384], [0, i16::MAX]]);
    let mut output = [99.0f32; 18];
    sink.read(&mut output, 6);
    assert_eq!(&output[..6], &[-1.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        &output[6..12],
        &[0.0, 32767.0 / 32768.0, 0.0, 0.0, 0.0, 0.0]
    );
    assert_eq!(&output[12..], &[0.0; 6]);
}

#[test]
fn presenter_reports_latched_audio_errors_once_and_recovers() {
    let mut host = presenter();
    host.audio.sink.active.store(true, Ordering::Relaxed);
    let edges = audio::EdgeSource::new([100, -100]);
    host.assembled.add_audio_source(Box::new(edges.clone()));
    let mut future = host.assembled.machine.clock();
    future.0 += 1;
    edges.record_level(future, [200, -200]);
    edges.record_level(host.assembled.machine.clock(), [300, -300]);
    let mut warnings = Vec::new();
    host.advance_with_warnings(std::time::Duration::from_millis(20), &mut warnings)
        .unwrap();
    assert!(host.running);
    assert!(host.error.is_none());
    assert!(host.audio.available());
    assert!(host.audio.sink.ring.lock().unwrap().len > 0);
    // The mixer has now observed this time, so another old device edge is late.
    edges.record_level(future, [400, -400]);
    host.advance_with_warnings(std::time::Duration::from_millis(40), &mut warnings)
        .unwrap();
    assert!(host.running);
    assert_eq!(String::from_utf8(warnings).unwrap().lines().count(), 1);
}

fn speaker_presenter() -> crate::Presenter {
    // Program channel 2 and repeatedly toggle speaker data while GATE stays high.
    let program = [
        0xFA, 0xB0, 0xB6, 0xE6, 0x43, 0xB0, 5, 0xE6, 0x42, 0xB0, 0, 0xE6, 0x42, 0xB0, 3, 0xE6,
        0x61, 0x34, 2, 0xEB, 0xFA,
    ];
    let mut rom = vec![0u8; 0x8000];
    rom[..program.len()].copy_from_slice(&program);
    let mut config = assembly::bios(rom, None);
    config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
    crate::Presenter::new(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
        &mut io::sink(),
    )
    .unwrap()
}

#[test]
fn unmute_refreshes_trimmed_speaker_history_before_the_next_run() {
    let mut actual = speaker_presenter();
    let mut reference = speaker_presenter();
    for host in [&mut actual, &mut reference] {
        host.audio.sink.active.store(true, Ordering::Relaxed);
        host.audio.toggle_mute();
        host.advance_fully(std::time::Duration::from_millis(20))
            .unwrap();
        assert_eq!(host.audio.sink.ring.lock().unwrap().len, 0);
        host.audio.toggle_mute();
    }
    // This independent explicit flush is the expected output-enable boundary.
    reference.assembled.capture_audio().unwrap();
    for host in [&mut actual, &mut reference] {
        host.advance_fully(std::time::Duration::from_millis(40))
            .unwrap();
    }
    let snapshot = |host: &crate::Presenter| {
        let mut ring = host.audio.sink.ring.lock().unwrap();
        ring.primed = true;
        let mut frames = vec![[0; 2]; ring.len];
        ring.read(&mut frames);
        frames
    };
    let actual = snapshot(&actual);
    assert!(!actual.is_empty());
    assert!(actual.iter().any(|frame| frame[0] > 0 && frame[0] < 16384));
    assert_eq!(actual, snapshot(&reference));
}

#[test]
fn config_uses_default_after_preferring_48_khz() {
    let default = range(2, 16000, SampleFormat::F32).with_sample_rate(16000);
    let ranges = [
        range(2, 44100, SampleFormat::I16),
        range(2, 16000, SampleFormat::F32),
    ];
    let selected = select_config(&ranges, Some(&default)).unwrap();
    assert_eq!(selected.sample_rate(), 16000);
    assert_eq!(selected.sample_format(), SampleFormat::F32);
    let selected = select_config(&[range(2, 48000, SampleFormat::F32)], Some(&default)).unwrap();
    assert_eq!(selected.sample_rate(), 48000);
}

#[test]
fn config_falls_back_to_nearest_rate_then_format_channels_and_higher_rate() {
    let ranges = [
        range(2, 16000, SampleFormat::I16),
        range(2, 44100, SampleFormat::F32),
        range(6, 44100, SampleFormat::I16),
        range(2, 44100, SampleFormat::I16),
    ];
    for default in [
        None,
        Some(range(1, 32000, SampleFormat::I16).with_sample_rate(32000)),
        Some(range(2, 32000, SampleFormat::U16).with_sample_rate(32000)),
    ] {
        let selected = select_config(&ranges, default.as_ref()).unwrap();
        assert_eq!(selected.sample_rate(), 44100);
        assert_eq!(selected.sample_format(), SampleFormat::I16);
        assert_eq!(selected.channels(), 2);
    }
    let continuous = SupportedStreamConfigRange::new(
        2,
        22050,
        44100,
        cpal::SupportedBufferSize::Unknown,
        SampleFormat::F32,
    );
    assert_eq!(
        select_config(&[continuous], None).unwrap().sample_rate(),
        44100
    );
    assert_eq!(
        select_config(
            &[
                range(2, 32000, SampleFormat::I16),
                range(2, 64000, SampleFormat::I16),
            ],
            None
        )
        .unwrap()
        .sample_rate(),
        64000
    );
    assert!(select_config(&[], None).is_none());
    assert!(select_config(&[range(1, 44100, SampleFormat::F32)], None).is_none());
}

fn tone(frames: usize) -> Vec<StereoFrame> {
    (0..frames)
        .map(|index| {
            let phase = std::f64::consts::TAU * 1000.0 * index as f64 / 48000.0;
            [
                (12000.0 * phase.sin()).round() as i16,
                (9000.0 * phase.cos()).round() as i16,
            ]
        })
        .collect()
}

fn irregular_blocks(input: &[StereoFrame], mut consume: impl FnMut(&[StereoFrame])) {
    let mut remaining = input;
    let mut seed = 7u32;
    consume(&[]);
    while !remaining.is_empty() {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let count = (1 + seed as usize % 2048).min(remaining.len());
        consume(&remaining[..count]);
        remaining = &remaining[count..];
    }
}

fn check_tone(rate: u32) {
    let input = tone(48000 * 10);
    let mut converter = RateConverter::new(rate);
    let mut output = Vec::new();
    let mut consumed = 0u64;
    irregular_blocks(&input, |block| {
        converter.process(block, |frame| output.push(frame));
        consumed += block.len() as u64;
        let numerator = consumed * u64::from(rate);
        assert!((numerator / 48000..=numerator.div_ceil(48000)).contains(&(output.len() as u64)));
    });
    assert_eq!(output.len(), rate as usize * 10);
    for (index, frame) in output.iter().enumerate() {
        let phase = std::f64::consts::TAU * 1000.0 * index as f64 / f64::from(rate);
        for (sample, expected) in frame
            .iter()
            .zip([12000.0 * phase.sin(), 9000.0 * phase.cos()])
        {
            assert!((f64::from(*sample) - expected).abs() <= 28.0);
        }
    }
}

#[test]
fn tone_at_44100_has_exact_duration_and_preserves_stereo_frequency() {
    check_tone(44100);
}

#[test]
fn tone_at_16000_has_exact_duration_and_preserves_stereo_frequency() {
    check_tone(16000);
}

#[test]
fn conversion_preserves_constants_and_extreme_samples() {
    for rate in [16000, 44100, 96000] {
        for frame in [[12345, -23456], [i16::MIN, i16::MAX], [0, 0]] {
            let mut converter = RateConverter::new(rate);
            let mut output = Vec::new();
            irregular_blocks(&vec![frame; 48000], |block| {
                converter.process(block, |frame| output.push(frame));
            });
            assert!(!output.is_empty());
            assert!(output.iter().all(|value| *value == frame));
        }
    }
}

#[test]
fn conversion_carries_fractional_phase_across_blocks() {
    let input = tone(48000);
    for rate in [16000, 44100, 96000] {
        let mut whole = Vec::new();
        RateConverter::new(rate).process(&input, |frame| whole.push(frame));
        let mut split = Vec::new();
        let mut converter = RateConverter::new(rate);
        irregular_blocks(&input, |block| {
            converter.process(block, |frame| split.push(frame))
        });
        assert_eq!(whole, split);
        let mut single_frames = Vec::new();
        let mut converter = RateConverter::new(rate);
        for frame in &input {
            converter.process(std::slice::from_ref(frame), |frame| {
                single_frames.push(frame)
            });
        }
        assert_eq!(whole, single_frames);
    }
}

#[test]
fn producer_converts_before_ring_write_without_allocations() {
    for rate in [16000, 44100] {
        let sink = active_sink(3);
        let subscribed = sink.clone();
        sink.configure(rate);
        let input = tone(480);
        let mut expected = Vec::new();
        RateConverter::new(rate).process(&input, |frame| expected.push(frame));
        ALLOCATIONS.with(|n| n.set(Some(0)));
        irregular_blocks(&input, |block| subscribed.push(block));
        let count = ALLOCATIONS.with(|n| n.replace(None));
        assert_eq!(count, Some(0));
        let mut ring = sink.ring.lock().unwrap();
        assert_eq!(ring.len, rate as usize / 100);
        assert_eq!(ring.len, expected.len());
        ring.primed = true;
        let mut actual = vec![[0; 2]; expected.len()];
        ring.read(&mut actual);
        assert_eq!(actual, expected);
    }
}

#[test]
fn ring_capacity_prefill_and_trim_use_device_rate_frames() {
    for rate in [16000, 44100] {
        let mut ring = Ring::new(rate as usize / 5, rate);
        let target = rate as usize / 20;
        let threshold = rate as usize * 70 / 1000;
        assert_eq!(ring.frames.len(), rate as usize * 200 / 1000);
        let mut output = [[0; 2]; 1];
        ring.push(&vec![[1, -1]; target - 1]);
        assert_eq!(ring.read(&mut output), 0);
        ring.push(&[[1, -1]]);
        assert_eq!(ring.read(&mut output), 1);
        ring.push(&vec![[2, -2]; threshold - ring.len]);
        assert_eq!(ring.len, threshold);
        ring.push(&[[3, -3]]);
        assert_eq!(ring.len, target);
        let mut drain = vec![[0; 2]; target + 1];
        assert_eq!(ring.read(&mut drain), target);
        ring.push(&[[4, -4]]);
        assert_eq!(ring.read(&mut output), 0);
    }
}

#[test]
fn mute_resets_converter_history_before_playback_resumes() {
    let mut host = HostAudio::new();
    host.sink.configure(44100);
    host.sink.active.store(true, Ordering::Relaxed);
    host.sink.push(&[[i16::MIN, i16::MAX]; 17]);
    host.toggle_mute();
    host.sink.push(&[[123, -123]; 9]);
    host.toggle_mute();
    host.sink.push(&[[1000, -1000]; 480]);
    let mut ring = host.sink.ring.lock().unwrap();
    assert_eq!(ring.len, 441);
    ring.primed = true;
    let mut output = vec![[0; 2]; ring.len];
    ring.read(&mut output);
    assert!(output.iter().all(|frame| *frame == [1000, -1000]));
}

#[test]
fn speed_mute_discards_samples_and_converter_history_before_fresh_prefill() {
    let mut host = HostAudio::new();
    host.sink.active.store(true, Ordering::Relaxed);
    host.sink.configure(44_100);
    host.sink.push(&vec![[1234, -1234]; TARGET]);
    assert!(host.sink.ring.lock().unwrap().len > 0);
    host.set_speed_muted(true);
    assert!(host.muted());
    assert_eq!(host.sink.ring.lock().unwrap().len, 0);
    host.sink.push(&vec![[1234, -1234]; TARGET]);
    host.set_speed_muted(false);
    assert!(!host.muted());
    assert!(host.sink.converter.lock().unwrap().previous.is_none());
    let mut output = [99i16; 2];
    host.sink.read(&mut output, 2);
    assert_eq!(output, [0, 0]);
    host.sink.push(&[[42, -42]]);
    host.sink.read(&mut output, 2);
    assert_eq!(output, [0, 0]);
    host.sink.push(&vec![[42, -42]; TARGET]);
    host.sink.read(&mut output, 2);
    assert_eq!(output, [42, -42]);
}

#[test]
fn speed_return_to_normal_preserves_the_users_mute_choice() {
    let mut host = HostAudio::new();
    host.sink.active.store(true, Ordering::Relaxed);
    host.toggle_mute();
    host.set_speed_muted(true);
    host.set_speed_muted(false);
    assert!(host.muted());
    host.toggle_mute();
    assert!(!host.muted());
    host.set_speed_muted(true);
    host.toggle_mute();
    host.toggle_mute();
    assert!(host.muted());
    host.set_speed_muted(false);
    assert!(!host.muted());
}

#[test]
fn speed_mute_labels_explain_the_user_preference_toggle() {
    let mut host = speaker_presenter();
    host.audio.sink.active.store(true, Ordering::Relaxed);
    host.select_speed(crate::Speed::Double, std::time::Duration::ZERO);
    assert_eq!(host.audio.status_text(), "Audio muted (speed)");
    assert!(host.title().contains("user mute off"));
    assert!(!host.title().contains(" to unmute"));
    assert_eq!(host.audio.mute_label(), "User mute: off (speed muted)");
    host.audio.toggle_mute();
    assert!(host.audio.muted());
    assert_eq!(host.audio.status_text(), "Audio muted (speed + user)");
    assert!(host.title().contains("user mute on"));
    assert!(!host.title().contains(" to unmute"));
    assert_eq!(host.audio.mute_label(), "User mute: on (speed muted)");
    host.select_speed(crate::Speed::Normal, std::time::Duration::ZERO);
    assert_eq!(host.audio.status_text(), "Audio muted (user)");
    assert!(host.audio.muted());
    host.select_speed(crate::Speed::Double, std::time::Duration::ZERO);
    host.audio.toggle_mute();
    assert_eq!(host.audio.status_text(), "Audio muted (speed)");
    host.select_speed(crate::Speed::Normal, std::time::Duration::ZERO);
    assert!(!host.audio.muted());
    assert_eq!(host.audio.status_text(), "Audio on");
}
