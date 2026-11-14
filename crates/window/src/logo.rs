// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-only brand presentation for the desktop shell; no guest time is read.
//!
//! See `backlog/docs/architecture.md` § Observation and presentation for the boundary
//! between UI repaint timing and deterministic machine execution.

use image::{codecs::gif::GifDecoder, AnimationDecoder};
use std::{io::Cursor, time::Duration};

const ICON: &[u8] = include_bytes!("../../../assets/brand/unemupc98-logo-512.png");
const ANIMATION: &[u8] = include_bytes!("../../../assets/brand/unemupc98-logo.gif");

pub(super) fn icon() -> image::ImageResult<egui::IconData> {
    let image = image::load_from_memory_with_format(ICON, image::ImageFormat::Png)?.into_rgba8();
    Ok(egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

fn frames() -> image::ImageResult<Vec<image::Frame>> {
    let mut frames = GifDecoder::new(Cursor::new(ANIMATION))?
        .into_frames()
        .collect_frames()?;
    key_uniform_border(&mut frames);
    Ok(frames)
}

fn key_uniform_border(frames: &mut [image::Frame]) -> Option<image::Rgba<u8>> {
    let colour = *frames.first()?.buffer().get_pixel_checked(0, 0)?;
    let uniform = frames.iter().all(|frame| {
        let buffer = frame.buffer();
        let (width, height) = buffer.dimensions();
        width > 0
            && height > 0
            && (0..width).all(|x| {
                *buffer.get_pixel(x, 0) == colour && *buffer.get_pixel(x, height - 1) == colour
            })
            && (0..height).all(|y| {
                *buffer.get_pixel(0, y) == colour && *buffer.get_pixel(width - 1, y) == colour
            })
    });
    if !uniform {
        return None;
    }
    for frame in frames {
        let buffer = frame.buffer_mut();
        let (width, height) = buffer.dimensions();
        let mut pending = Vec::new();
        for x in 0..width {
            pending.push((x, 0));
            pending.push((x, height - 1));
        }
        for y in 0..height {
            pending.push((0, y));
            pending.push((width - 1, y));
        }
        let mut visited = vec![false; width as usize * height as usize];
        while let Some((x, y)) = pending.pop() {
            let index = y as usize * width as usize + x as usize;
            if visited[index] || *buffer.get_pixel(x, y) != colour {
                continue;
            }
            visited[index] = true;
            buffer.get_pixel_mut(x, y).0[3] = 0;
            if x > 0 {
                pending.push((x - 1, y));
            }
            if y > 0 {
                pending.push((x, y - 1));
            }
            if x + 1 < width {
                pending.push((x + 1, y));
            }
            if y + 1 < height {
                pending.push((x, y + 1));
            }
        }
    }
    Some(colour)
}

fn delay(frame: &image::Frame) -> Duration {
    let (numerator, denominator) = frame.delay().numer_denom_ms();
    Duration::from_nanos(u64::from(numerator) * 1_000_000 / u64::from(denominator))
}

struct AnimationClock {
    delays: Vec<Duration>,
    cycle: Duration,
}

impl AnimationClock {
    fn new(delays: Vec<Duration>) -> Self {
        let cycle = delays.iter().sum();
        Self { delays, cycle }
    }

    fn frame_at(&self, elapsed: Duration, reduced_motion: bool) -> (usize, Option<Duration>) {
        if reduced_motion || self.delays.len() < 2 || self.cycle.is_zero() {
            return (0, None);
        }
        let mut phase = elapsed.as_nanos() % self.cycle.as_nanos();
        for (index, delay) in self.delays.iter().enumerate() {
            if phase < delay.as_nanos() {
                return (index, Some(*delay - Duration::from_nanos(phase as u64)));
            }
            phase -= delay.as_nanos();
        }
        (0, None)
    }
}

pub(super) struct Logo {
    textures: Vec<egui::TextureHandle>,
    clock: AnimationClock,
    epoch: Option<f64>,
}

impl Logo {
    pub fn load(ctx: &egui::Context) -> image::ImageResult<Self> {
        let frames = frames()?;
        if frames.is_empty() || frames.iter().any(|frame| delay(frame).is_zero()) {
            return Err(image::ImageError::IoError(std::io::Error::other(
                "embedded logo must have frames with nonzero delays",
            )));
        }
        let clock = AnimationClock::new(frames.iter().map(delay).collect());
        let textures = frames
            .into_iter()
            .enumerate()
            .map(|(index, frame)| {
                let buffer = frame.into_buffer();
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [buffer.width() as usize, buffer.height() as usize],
                    buffer.as_raw(),
                );
                ctx.load_texture(
                    format!("welcome-logo-{index}"),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            })
            .collect();
        Ok(Self {
            textures,
            clock,
            epoch: None,
        })
    }

    pub fn pause(&mut self) {
        self.epoch = None;
    }

    pub fn welcome(&mut self, ui: &mut egui::Ui, reduced_motion: bool, visible: bool) {
        let size = ui
            .available_height()
            .min(ui.available_width())
            .clamp(0.0, 128.0);
        let frame = if visible && ui.is_visible() && size > 0.0 && !reduced_motion {
            let now = ui.input(|input| input.time);
            let epoch = *self.epoch.get_or_insert(now);
            let elapsed = Duration::from_secs_f64((now - epoch).max(0.0));
            let (frame, next) = self.clock.frame_at(elapsed, reduced_motion);
            if let Some(next) = next {
                ui.ctx().request_repaint_after(next);
            }
            frame
        } else {
            self.pause();
            0
        };
        ui.add(egui::Image::new(&self.textures[frame]).fit_to_exact_size(egui::vec2(size, size)));
    }

    pub fn about(&self, ui: &mut egui::Ui) {
        ui.add(egui::Image::new(&self.textures[0]).fit_to_exact_size(egui::vec2(96.0, 96.0)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_fill_preserves_an_enclosed_region_of_the_same_colour() {
        let background = image::Rgba([12, 24, 36, 255]);
        let outline = image::Rgba([13, 24, 36, 255]);
        let mut buffer = image::RgbaImage::from_pixel(5, 5, background);
        for x in 1..4 {
            for y in 1..4 {
                if x != 2 || y != 2 {
                    buffer.put_pixel(x, y, outline);
                }
            }
        }
        let mut frames = [image::Frame::new(buffer)];
        assert_eq!(key_uniform_border(&mut frames), Some(background));
        assert_eq!(*frames[0].buffer().get_pixel(2, 2), background);
        assert_eq!(*frames[0].buffer().get_pixel(1, 1), outline);
        assert_eq!(frames[0].buffer().get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn uniform_border_keys_only_the_exact_colour_across_frames() {
        let background = image::Rgba([12, 24, 36, 255]);
        let edge = image::Rgba([13, 24, 36, 255]);
        let mut buffer = image::RgbaImage::from_pixel(5, 5, background);
        buffer.put_pixel(2, 2, edge);
        let original = image::Frame::new(buffer);
        let mut frames = vec![original.clone(), original.clone()];
        assert_eq!(key_uniform_border(&mut frames), Some(background));
        for frame in &frames {
            assert_eq!(*frame.buffer().get_pixel(2, 2), edge);
            assert_eq!(
                *frame.buffer().get_pixel(1, 1),
                image::Rgba([12, 24, 36, 0])
            );
            assert_eq!(frame.buffer().get_pixel(0, 0).0[3], 0);
        }
        let mut frames = vec![original.clone(), original];
        frames[1].buffer_mut().put_pixel(0, 1, edge);
        let before = frames.clone();
        assert_eq!(key_uniform_border(&mut frames), None);
        for (actual, expected) in frames.iter().zip(&before) {
            assert_eq!(actual.buffer(), expected.buffer());
        }
    }

    #[test]
    fn embedded_animation_border_key_is_reported_without_visual_inspection() {
        let mut decoded = GifDecoder::new(Cursor::new(ANIMATION))
            .unwrap()
            .into_frames()
            .collect_frames()
            .unwrap();
        let colour = image::Rgba([4, 2, 37, 255]);
        assert_eq!(key_uniform_border(&mut decoded), Some(colour));
        let enclosed = decoded[0]
            .buffer()
            .enumerate_pixels()
            .filter(|(x, y, pixel)| {
                (116..=395).contains(x) && (122..=389).contains(y) && **pixel == colour
            })
            .count();
        assert!(enclosed > 30_000, "enclosed opaque pixels: {enclosed}");
        assert_eq!(decoded[0].buffer().get_pixel(0, 0).0[3], 0);
        let displayed = frames().unwrap();
        for (actual, expected) in displayed.iter().zip(&decoded) {
            assert_eq!(actual.buffer(), expected.buffer());
        }
    }

    #[test]
    fn embedded_gif_has_multiple_frames_with_nonzero_delays() {
        let frames = frames().unwrap();
        assert!(frames.len() > 1);
        for frame in &frames {
            assert_eq!(
                (frame.buffer().width(), frame.buffer().height()),
                (512, 512)
            );
            assert!(!delay(frame).is_zero());
        }
    }

    #[test]
    fn animation_clock_uses_frame_delays_and_wraps() {
        let clock = AnimationClock::new([20, 70, 30].map(Duration::from_millis).to_vec());
        for (elapsed, frame, remaining) in [
            (0, 0, 20),
            (19, 0, 1),
            (20, 1, 70),
            (50, 1, 40),
            (89, 1, 1),
            (90, 2, 30),
            (119, 2, 1),
            (120, 0, 20),
            (260, 1, 70),
        ] {
            assert_eq!(
                clock.frame_at(Duration::from_millis(elapsed), false),
                (frame, Some(Duration::from_millis(remaining))),
                "elapsed {elapsed} ms"
            );
        }
    }

    #[test]
    fn reduced_motion_pins_first_frame_without_a_repaint_deadline() {
        let clock = AnimationClock::new([20, 70, 30].map(Duration::from_millis).to_vec());
        for elapsed in [0, 20, 90, 120, 260, u64::MAX] {
            assert_eq!(
                clock.frame_at(Duration::from_millis(elapsed), true),
                (0, None)
            );
        }
    }

    #[test]
    fn welcome_reuses_textures_and_pauses_when_hidden_or_reduced() {
        let ctx = egui::Context::default();
        let mut logo = Logo::load(&ctx).unwrap();
        let ids: Vec<_> = logo.textures.iter().map(egui::TextureHandle::id).collect();
        for (time, reduced, visible, ui_visible) in [
            (1.0, false, true, true),
            (1.1, false, true, true),
            (2.0, false, false, true),
            (3.0, false, true, true),
            (4.0, false, true, false),
            (5.0, true, true, true),
            (6.0, false, true, true),
        ] {
            let output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    if !ui_visible {
                        ui.set_invisible();
                    }
                    logo.welcome(ui, reduced, visible);
                },
            );
            let active = !reduced && visible && ui_visible;
            assert_eq!(logo.epoch.is_some(), active);
            if time > 1.0 {
                assert!(output
                    .textures_delta
                    .set
                    .iter()
                    .all(|(id, _)| !ids.contains(id)));
            }
            output.drop_without_applying_deltas();
        }
        logo.pause();
        assert!(logo.epoch.is_none());
        let output = ctx.run_ui(Default::default(), |ui| logo.about(ui));
        assert!(logo.epoch.is_none());
        output.drop_without_applying_deltas();
    }

    #[test]
    fn embedded_icon_decodes_to_512_square_rgba() {
        let icon = icon().unwrap();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);
    }
}
