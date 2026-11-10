// SPDX-License-Identifier: AGPL-3.0-or-later
//! Optional host-window text presenter; see `backlog/docs/design/spec.md` §9.4.
//!
//! Rasterisation is pure: pixels are `0x00RRGGBB` u32 values, matching softbuffer.
//! CP437 glyphs come from VileR's Ultimate Oldschool PC Font Pack v2.2,
//! <http://int10h.org/oldschool-pc-fonts/>, CC BY-SA 4.0. The original .FON files
//! and attribution are in `assets/fonts/`; they are parsed at load time.
//!
//! IBM PC Technical Reference, Options and Adapters, Monochrome Display and
//! Printer Adapter, Programming Considerations (attribute byte table), and IBM
//! VGA Technical Reference, Alphanumeric Modes, describe the text effects.
//! MDA uses grey `AAAAAA` on black, white `FFFFFF` for foreground intensity,
//! and dark grey `555555` for background intensity (white behind reverse video).
//! Reverse intensity leaves its black foreground black and its grey background
//! unchanged; only background intensity makes reverse paper white.
//! Underline occupies zero-based scanline 12 of the 14-line MDA cell. The cursor
//! uses the BIOS default shape: MDA rows 11–12, VGA rows 13–14 (zero-based).
//! TextModeState has no cursor start/end registers, so the programmed shape is
//! unavailable. Phosphor response and aspect correction are absent.
//! Colour uses the standard 16-entry CGA/VGA text palette. Blink is drawn steady:
//! there is no host clock, pacing, graphics mode or custom font.
//! Physical keyboard events enter a host FIFO. Immediately before each fixed
//! run chunk it transfers bytes to the 8042, retaining bytes under backpressure.
//! Events are quantised to chunk boundaries, never injected inside a chunk.
//! Repeated host presses emit make codes again (typematic); releases emit breaks.
//! Captured physical Shift and guest LED NumLock select IBM artificial Shift
//! wrappers for E0 navigation/Divide (IBM PS/2 Technical Reference, Set 1 Scan-Code
//! Tables and Set/Reset Status Indicators). Events are encoded only when the
//! 8042 can accept their first byte at a boundary, using its current LED state.
//! Repeats use plain base makes; wrappers surround first make and final break.
//! Focus loss releases all held keys; unheld releases and stopped-machine input
//! are ignored. macOS winit 0.30 does not deliver CapsLock key events.
//! See `backlog/docs/design/observation.md` § Host input for timing and coverage.
//! Only the binary owns a window; library tests and frame sinks stay headless.

/// Attribution carried by binaries that embed the original font files.
pub const FONT_NOTICE: &str = concat!(
    "Ultimate Oldschool PC Font Pack v2.2\n",
    "(c) 2016-2020 VileR\n",
    "CC BY-SA 4.0: https://creativecommons.org/licenses/by-sa/4.0/\n",
    "source: http://int10h.org/oldschool-pc-fonts/\n",
    "font files unmodified",
);

mod font;
mod keyboard;
mod raster;

pub use font::{FontAtlas, FontError, MDA_FONT, VGA_FONT};
pub use keyboard::{set1_scancodes, HostKeyboard};
pub use raster::{PixelFrame, RasterError, Rasterizer};

use std::{
    cell::{Cell, Ref, RefCell},
    num::NonZeroU16,
    rc::Rc,
};
use video::{Frame, FrameSink};

/// A single-threaded subscriber retaining only the most recent rasterised frame.
/// Clones share pixels with the window without lending the sink the machine.
#[derive(Clone)]
pub struct WindowSink {
    rasterizer: Rc<Rasterizer>,
    latest: Rc<RefCell<Option<PixelFrame>>>,
    error: Rc<Cell<Option<RasterError>>>,
}

impl WindowSink {
    /// Parse the bundled fonts and prepare an integer-scaled subscriber.
    pub fn new(scale: NonZeroU16) -> Result<Self, FontError> {
        Ok(Self {
            rasterizer: Rc::new(Rasterizer::bundled(scale)?),
            latest: Rc::new(RefCell::new(None)),
            error: Rc::new(Cell::new(None)),
        })
    }

    /// Borrow the last successfully rasterised frame, without copying its pixels.
    pub fn frame(&self) -> Ref<'_, Option<PixelFrame>> {
        self.latest.borrow()
    }

    /// The latest presentation failure; a successful frame clears it.
    pub fn error(&self) -> Option<RasterError> {
        self.error.get()
    }
}

impl FrameSink for WindowSink {
    fn present(&mut self, frame: &Frame) {
        let Frame::Text(grid) = frame;
        match self.rasterizer.rasterize(grid) {
            Ok(frame) => {
                *self.latest.borrow_mut() = Some(frame);
                self.error.set(None);
            }
            Err(error) => self.error.set(Some(error)),
        }
    }
}
