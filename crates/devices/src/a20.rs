// SPDX-License-Identifier: AGPL-3.0-or-later
//! Board A20 enable sources: IBM PC AT Technical Reference, Keyboard Controller,
//! Output Port; IBM Personal System/2 Hardware Interface Technical Reference,
//! "System Control Port A (Hex 0092)". See
//! `backlog/docs/design/machine-config.md` §A20 line and System Control Port A.
//!
//! ASSUMED board policy: either source enables A20 (logical OR). With the
//! 8042's enabled power-on source, firmware using only port 92h cannot disable
//! effective A20. SeaBIOS uses that path (system.c, handle_152400/handle_152402).
//! QEMU instead uses last-writer-wins (pc.c, handle_a20_line_change), with its
//! comment "XXX: add logic to handle multiple A20 line sources". The generic
//! ISA board must choose OR versus last-writer-wins and test SeaBIOS INT 15h
//! AX=2400h/2402h; today's OR default is not firmware qualification.
//!
//! Assembly forwards this line to CPU physical addressing at port boundaries.
//! Devices retain source latches in their snapshots; DMA bypasses the CPU gate.

use std::{cell::Cell, rc::Rc};

/// Shared motherboard A20 line, with both sources initially disabled.
///
/// Clones observe the same state. Attaching an 8042 enables its power-on source
/// and counts one transition: assembled 8042 machines start at change_count=1.
/// Attaching port 92h leaves its source disabled. A machine with no A20 source
/// reports disabled but does not attach this line to the CPU, retaining flat
/// addressing. Machine synchronization samples the current count at attachment; see
/// `backlog/docs/design/machine-config.md` §A20 line and System Control Port A.
/// The counter observes effective transitions, including reset/restore; it is
/// diagnostic history, not part of device snapshots.
#[derive(Clone, Debug, Default)]
pub struct A20Line(Rc<State>);

#[derive(Debug, Default)]
struct State {
    keyboard: Cell<bool>,
    fast: Cell<bool>,
    changes: Cell<u64>,
}

impl A20Line {
    /// Effective OR of the two motherboard enable sources (ASSUMED policy).
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.0.keyboard.get() || self.0.fast.get()
    }

    /// Number of effective transitions, wrapping at the integer limit.
    #[must_use]
    pub fn change_count(&self) -> u64 {
        self.0.changes.get()
    }

    /// The two enable sources as a host diagnostic sees them: the 8042
    /// output port's A20 bit, then port 92h bit 1 (System Control Port A).
    /// Reading them changes nothing.
    #[must_use]
    pub fn sources(&self) -> (bool, bool) {
        (self.0.keyboard.get(), self.0.fast.get())
    }

    pub(crate) fn set_keyboard(&self, enabled: bool) {
        self.set(&self.0.keyboard, enabled);
    }

    pub(crate) fn set_fast(&self, enabled: bool) {
        self.set(&self.0.fast, enabled);
    }

    fn set(&self, source: &Cell<bool>, enabled: bool) {
        let before = self.enabled();
        source.set(enabled);
        if self.enabled() != before {
            self.0.changes.set(self.change_count().wrapping_add(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_show_each_enable_and_the_line_is_their_or() {
        let line = A20Line::default();
        assert_eq!(line.sources(), (false, false));
        line.set_fast(true);
        assert_eq!((line.sources(), line.enabled()), ((false, true), true));
        line.set_keyboard(true);
        line.set_fast(false);
        assert_eq!((line.sources(), line.enabled()), ((true, false), true));
    }
}
