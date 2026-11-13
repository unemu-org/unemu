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
//! its phase can later derive from virtual time. VGA frames carry guest fonts
//! or indexed graphics and a captured palette. Only the binary event loop reads
//! the host clock for pacing.
//! Physical keyboard events enter a host FIFO. Immediately before each bounded
//! run chunk it transfers bytes to the 8042, retaining bytes under backpressure.
//! Events are quantised to chunk boundaries, never injected inside a chunk.
//! The binary consumes Ctrl+Alt+F to insert the next A: medium and Ctrl+Alt+E
//! to eject it; Shift selects B:. Next cycles the image list without an empty
//! entry, and from empty selects the first image. Media events enter at the same
//! run boundaries; the title shows current file names or `empty`. See
//! `backlog/docs/design/observation.md` § Floppy host input.
//! Repeated host presses emit make codes again (typematic); releases emit breaks.
//! Captured physical Shift and guest LED NumLock select IBM artificial Shift
//! wrappers for E0 navigation/Divide (IBM PS/2 Technical Reference, Set 1 Scan-Code
//! Tables and Set/Reset Status Indicators). Events are encoded only when the
//! 8042 can accept their first byte at a boundary, using its current LED state.
//! Repeats use plain base makes; wrappers surround first make and final break.
//! Focus loss releases all held keys; unheld releases and stopped-machine input
//! are ignored. macOS winit 0.30 does not deliver CapsLock key events.
//! See `backlog/docs/design/observation.md` § Host input for timing and coverage.
//! An opt-in PS/2 mouse captures on a client-area click (that click is swallowed).
//! Ctrl+Alt+G, focus loss and suspension release capture and guest buttons.
//! The window tries Locked then Confined, hides the cursor and shows the release
//! chord in its title. winit 0.30 on macOS supports Locked by disassociating cursor
//! position from movement; Confined is unsupported. Failed grabs leave input free.
//! Raw DeviceEvent motion uses one host unit per PS/2 count, positive Y up, with
//! carried fractions. Left/right/middle use packet bits 0/1/2 (IBM PS/2 Hardware
//! Interface Technical Reference, Common Interfaces, Pointing Device, Data Reporting).
//! Motion coalesces between button transitions and enters through inject_mouse
//! immediately after keyboard delivery at each run boundary. Button transitions
//! wait for empty mouse output and keep later changes queued; keyboard readiness
//! is independent. No mouse means no capture or title changes. The device owns
//! overflow and motion coalescing. Application exit also restores the pointer.
//! Only the binary owns a window and cpal output stream; library tests and frame
//! sinks stay headless. Audio observations feed a 200 ms stereo ring at run
//! boundaries; callbacks use silence on underrun and drop oldest frames on
//! overrun. Playback never drives virtual time. Ctrl+Alt+M toggles host mute
//! only while a device is active. Playback prefills 50 ms and trims above 70 ms;
//! recoverable host/source errors leave the window running.
//! See `backlog/docs/design/audio.md` §Host output for formats and fallback.

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
mod mouse;
mod raster;

pub use font::{FontAtlas, FontError, MDA_FONT, VGA_FONT};
pub use keyboard::{set1_scancodes, HostKeyboard};
pub use mouse::{HostMouse, MouseInput};
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
        match self.rasterizer.rasterize_frame(frame) {
            Ok(frame) => {
                *self.latest.borrow_mut() = Some(frame);
                self.error.set(None);
            }
            Err(error) => self.error.set(Some(error)),
        }
    }
}
