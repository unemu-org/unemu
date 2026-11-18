// SPDX-License-Identifier: AGPL-3.0-or-later
//! The PC core behind the front-end contract; see
//! `backlog/docs/design/core-contract.md` §4 and `backlog/docs/design/spec.md`
//! §9.4.
//!
//! [`PcMachine`] wraps an assembled PC ([`assembly::AssembledMachine`]) and
//! drives it as a [`unemu_core_api::Machine`]; [`PcCore`] is the PC's entry in
//! a front end's list of cores. Everything below the contract that is PC
//! vocabulary lives here: scan codes and the 8042's back-pressure
//! ([`keyboard`]), the pointing device's packets ([`mouse`]), the conversion
//! of a tick deadline to a retired-instruction cap ([`pacing`]) and the text
//! rasteriser with its fonts ([`raster`], [`font`]). The machine is assembled
//! by the PC provider, which hands this crate the assembled machine; creating
//! one from a stored config through [`PcCore`] is a later stage.
//!
//! Rasterisation is pure: pixels are `0x00RRGGBB` u32 values.
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
//! or indexed graphics and a captured palette.
//!
//! Keys arrive as USB HID keyboard usages and enter a FIFO. Immediately before
//! each bounded run chunk it transfers bytes to the 8042, retaining bytes under
//! backpressure. Events are quantised to chunk boundaries, never injected
//! inside a chunk. Repeated presses emit make codes again (typematic);
//! releases emit breaks. Physical Shift and guest LED NumLock select IBM
//! artificial Shift wrappers for E0 navigation/Divide (IBM PS/2 Technical
//! Reference, Set 1 Scan-Code Tables and Set/Reset Status Indicators). Events
//! are encoded only when the 8042 can accept their first byte at a boundary,
//! using its current LED state. Repeats use plain base makes; wrappers
//! surround first make and final break. Releasing everything (focus loss)
//! releases all held keys; unheld releases are ignored.
//!
//! Pointer motion arrives in PS/2 counts, positive Y up. Left/right/middle use
//! packet bits 0/1/2 (IBM PS/2 Hardware Interface Technical Reference, Common
//! Interfaces, Pointing Device, Data Reporting). Motion coalesces between
//! button transitions and enters through `inject_mouse` immediately after
//! keyboard delivery at each run boundary. Button transitions wait for empty
//! mouse output and keep later changes queued; keyboard readiness is
//! independent. The device owns overflow and motion coalescing.
//!
//! See `backlog/docs/design/observation.md` § Host input for timing and coverage.

#![forbid(unsafe_code)]

/// Attribution carried by binaries that embed the original font files.
pub const FONT_NOTICE: &str = concat!(
    "Ultimate Oldschool PC Font Pack v2.2\n",
    "(c) 2016-2020 VileR\n",
    "CC BY-SA 4.0: https://creativecommons.org/licenses/by-sa/4.0/\n",
    "source: http://int10h.org/oldschool-pc-fonts/\n",
    "font files unmodified",
);

mod core;
pub mod font;
pub mod keyboard;
mod machine;
pub mod mouse;
pub mod pacing;
pub mod raster;

pub use crate::core::PcCore;
pub use font::{FontAtlas, FontError, MDA_FONT, VGA_FONT};
pub use keyboard::{set1_scancodes, HostKeyboard};
#[cfg(feature = "test-support")]
pub use machine::InputLogEntry;
pub use machine::{PcMachine, RASTER_SCALE};
pub use mouse::{MouseInput, PcMouse};
pub use raster::{PixelFrame, RasterError, Rasterizer};

use std::{
    cell::{Cell, Ref, RefCell},
    num::NonZeroU16,
    rc::Rc,
};
use video::{Frame, FrameSink};

/// A single-threaded subscriber retaining only the most recent rasterised frame.
/// Clones share pixels with their owner without lending the sink the machine.
#[derive(Clone)]
pub struct RasterSink {
    rasterizer: Rc<Rasterizer>,
    latest: Rc<RefCell<Option<PixelFrame>>>,
    error: Rc<Cell<Option<RasterError>>>,
}

impl RasterSink {
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

impl FrameSink for RasterSink {
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
