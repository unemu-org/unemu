// SPDX-License-Identifier: AGPL-3.0-or-later
//! Code execution from the Lo-tech EMS page frame (§4.2).
//!
//! Lo-tech 2MB EMS Board schematic r02: each 16 KiB window of the 64 KiB
//! frame shows the SRAM page its HCT573 latch selects, and an unbacked window
//! (latch 80h–FFh or an empty chip socket) floats to FFh through the data-bus
//! pull-ups. The CPU core fetches instructions only from RAM and ROM regions,
//! never from a trapped MMIO region, so each window takes one of three forms:
//!
//! - a populated page that no other latch selects is held as CPU RAM, and
//!   that copy is authoritative while it is held;
//! - an unbacked window is CPU ROM filled with FFh: reads and fetches see
//!   FFh, stores and DMA writes are dropped, as on the floating bus;
//! - a page selected by two latches stays trapped, so the board serves both
//!   windows coherently; code cannot run from it.
//!
//! After every latch write the map writes held copies back into SRAM, retiles
//! the frame when a window changes form, and then loads each newly held page,
//! or the FFh fill of a newly unbacked window, through the CPU's
//! self-modifying-code-safe write path, so no translation of the previous
//! contents can run. Neighbouring windows of one form share a CPU region. The
//! core joins RAM and ROM regions for one operand or instruction, but not a
//! trapped region with anything else: an operand crossing between a trapped
//! region (a shared window, or another device's window beside the frame) and
//! a held or unbacked window stops the run with an unmapped-access CPU error.
//!
//! Device snapshots read SRAM, so held pages are written back whenever a run
//! returns to the host. A snapshot restore replaces SRAM; the next run then
//! drops the held copies unwritten and loads the restored pages. Latch writes
//! a host makes between runs also apply when the next run starts, but a host
//! bus write into a held window only reaches SRAM, which the CPU copy
//! replaces on its next write-back. ISA DMA follows the same CPU map: bytes
//! for a held window land in its CPU copy. See
//! `backlog/docs/architecture.md` §x86jit integration.

use cpu::{CpuError, X86JitCpu};
use devices::lotech_ems::LotechEmsFrame;
use machine::MemoryMapSync;

const WINDOW: u64 = 0x4000;
const WINDOWS: usize = 4;
/// The floating data bus of an unbacked window.
static UNBACKED: [u8; WINDOW as usize] = [0xFF; WINDOW as usize];

/// The CPU form of one frame window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Window {
    /// CPU RAM holding this SRAM page.
    Held(u8),
    /// CPU ROM filled with FFh.
    Unbacked,
    /// A trapped window the board serves.
    Shared,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Kind {
    Ram,
    Rom,
    Trap,
}

impl Window {
    fn kind(self) -> Kind {
        match self {
            Self::Held(_) => Kind::Ram,
            Self::Unbacked => Kind::Rom,
            Self::Shared => Kind::Trap,
        }
    }
}

/// One CPU region of the trapped run that contains the frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Tile {
    base: u64,
    end: u64,
    kind: Kind,
}

/// Keeps the CPU memory map of one EMS frame in step with its board.
pub(crate) struct EmsFrameMap {
    board: LotechEmsFrame,
    frame: u64,
    /// The trapped CPU region assembly mapped around the frame; neighbouring
    /// trapped windows of other devices stay trapped inside it.
    run: (u64, u64),
    /// What each window shows the CPU now. Assembly maps the frame trapped.
    windows: [Window; WINDOWS],
    /// The CPU regions now mapped over `run`, updated region by region.
    tiles: Vec<Tile>,
    /// Board change counters at the last complete apply; `None` before it.
    seen: Option<(u64, u64)>,
}

impl EmsFrameMap {
    /// Follow `board`, whose frame at physical `frame` lies inside the trapped
    /// CPU region `run` exactly as assembly mapped it.
    pub(crate) fn new(board: LotechEmsFrame, frame: u64, run: (u64, u64)) -> Self {
        Self {
            board,
            frame,
            run,
            windows: [Window::Shared; WINDOWS],
            tiles: vec![Tile {
                base: run.0,
                end: run.1,
                kind: Kind::Trap,
            }],
            seen: None,
        }
    }

    pub(crate) fn sync(
        &mut self,
        cpu: &mut X86JitCpu,
        event: MemoryMapSync,
    ) -> Result<(), CpuError> {
        match event {
            MemoryMapSync::Apply => self.apply(cpu),
            MemoryMapSync::Flush => self.flush(cpu),
        }
    }

    /// `seen` moves only after every step succeeded, so a failed apply is
    /// retried in full at the next port write or run start.
    fn apply(&mut self, cpu: &mut X86JitCpu) -> Result<(), CpuError> {
        let now = (self.board.latch_writes(), self.board.restores());
        if self.seen == Some(now) {
            return Ok(());
        }
        let restored = self.seen.is_some_and(|(_, restores)| restores != now.1);
        let latches = self.board.latches();
        let wanted: [Window; WINDOWS] = std::array::from_fn(|window| {
            let page = latches[window];
            if self.board.page(page).is_none() {
                Window::Unbacked
            } else if latches.iter().filter(|&&other| other == page).count() > 1 {
                Window::Shared
            } else {
                Window::Held(page)
            }
        });
        let tiles = self.tiles_for(&wanted);
        let retile = tiles != self.tiles;
        // Write back before anything moves: every changed window, and every
        // held window when regions are about to be re-tagged, so SRAM is
        // current even if a later step fails. A restore owns SRAM instead.
        if !restored {
            for (window, next) in wanted.into_iter().enumerate() {
                if let Window::Held(page) = self.windows[window] {
                    if retile || next != self.windows[window] {
                        self.write_back(cpu, window, page)?;
                    }
                }
            }
        }
        if retile {
            // The core's flat model keeps the backing bytes when a region is
            // unmapped and another is mapped over them, so a window that keeps
            // its form across a retile keeps its contents without a copy.
            while let Some(&tile) = self.tiles.last() {
                cpu.unmap(tile.base, tile.end - tile.base)?;
                self.tiles.pop();
            }
            for tile in tiles {
                let size = tile.end - tile.base;
                match tile.kind {
                    Kind::Ram => cpu.map_ram(tile.base, size)?,
                    Kind::Rom => cpu.map_rom_in_place(tile.base, size)?,
                    Kind::Trap => cpu.map_mmio(tile.base, size)?,
                }
                self.tiles.push(tile);
            }
        }
        for (window, next) in wanted.into_iter().enumerate() {
            let base = self.window_base(window);
            match next {
                Window::Held(page) if restored || next != self.windows[window] => {
                    if let Some(bytes) = self.board.page(page) {
                        cpu.write_mem(base, &bytes)?;
                    }
                }
                Window::Unbacked if next != self.windows[window] => {
                    cpu.write_mem(base, &UNBACKED)?;
                }
                _ => {}
            }
            self.windows[window] = next;
        }
        self.seen = Some(now);
        Ok(())
    }

    fn flush(&self, cpu: &X86JitCpu) -> Result<(), CpuError> {
        // Defensive: every run starts with an apply, so a restore made between
        // runs is applied before any flush. Were a flush to come first, the
        // held copies predate the restored SRAM and must not overwrite it.
        if self
            .seen
            .is_some_and(|(_, restores)| restores != self.board.restores())
        {
            return Ok(());
        }
        for (window, state) in self.windows.into_iter().enumerate() {
            if let Window::Held(page) = state {
                self.write_back(cpu, window, page)?;
            }
        }
        Ok(())
    }

    fn write_back(&self, cpu: &X86JitCpu, window: usize, page: u8) -> Result<(), CpuError> {
        if let Some(mut bytes) = self.board.page_mut(page) {
            cpu.read_mem(self.window_base(window), &mut bytes)?;
        }
        Ok(())
    }

    fn window_base(&self, window: usize) -> u64 {
        self.frame + window as u64 * WINDOW
    }

    fn tiles_for(&self, wanted: &[Window; WINDOWS]) -> Vec<Tile> {
        let mut tiles: Vec<Tile> = Vec::new();
        let mut extend = |base: u64, end: u64, kind: Kind| {
            if base == end {
                return;
            }
            match tiles.last_mut() {
                Some(last) if last.kind == kind => last.end = end,
                _ => tiles.push(Tile { base, end, kind }),
            }
        };
        extend(self.run.0, self.frame, Kind::Trap);
        for (window, state) in wanted.iter().enumerate() {
            let base = self.window_base(window);
            extend(base, base + WINDOW, state.kind());
        }
        extend(self.window_base(WINDOWS), self.run.1, Kind::Trap);
        tiles
    }
}

#[cfg(test)]
#[path = "ems_frame_tests.rs"]
mod tests;
