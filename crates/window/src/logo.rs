// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-only brand presentation for the desktop shell; no guest time is read.
//!
//! See `backlog/docs/architecture.md` § Observation and presentation for the boundary
//! between UI repaint timing and deterministic machine execution.

use image::{codecs::gif::GifDecoder, AnimationDecoder};
use std::io::Cursor;

const ICON: &[u8] = include_bytes!("../../../assets/brand/unemupc-logo-512.png");
const ANIMATION: &[u8] = include_bytes!("../../../assets/brand/unemupc-logo.gif");

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

/// The About dialog's logo: the first frame of the embedded animation.
pub(super) struct Logo {
    texture: egui::TextureHandle,
}

impl Logo {
    pub fn load(ctx: &egui::Context) -> image::ImageResult<Self> {
        let Some(frame) = frames()?.into_iter().next() else {
            return Err(image::ImageError::IoError(std::io::Error::other(
                "embedded logo has no frames",
            )));
        };
        let buffer = frame.into_buffer();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [buffer.width() as usize, buffer.height() as usize],
            buffer.as_raw(),
        );
        Ok(Self {
            texture: ctx.load_texture("about-logo", image, egui::TextureOptions::LINEAR),
        })
    }

    pub fn about(&self, ui: &mut egui::Ui) {
        ui.add(egui::Image::new(&self.texture).fit_to_exact_size(egui::vec2(96.0, 96.0)));
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
    fn embedded_gif_frames_are_512_square() {
        let frames = frames().unwrap();
        assert!(!frames.is_empty());
        for frame in &frames {
            assert_eq!(
                (frame.buffer().width(), frame.buffer().height()),
                (512, 512)
            );
        }
    }

    #[test]
    fn about_logo_loads_one_texture_and_draws() {
        let ctx = egui::Context::default();
        let logo = Logo::load(&ctx).unwrap();
        let output = ctx.run_ui(Default::default(), |ui| logo.about(ui));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn embedded_icon_decodes_to_512_square_rgba() {
        let icon = icon().unwrap();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);
    }
}
