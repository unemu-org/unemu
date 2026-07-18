// SPDX-License-Identifier: AGPL-3.0-or-later
//! CPU configuration (§2, §3.1).

use cpu::x86jit_backend::RealSegments;

/// The modelled CPU chip.
///
/// A **marker** only: it identifies which processor the machine models for
/// documentation and golden identity. It carries no behavioural toggle and no
/// clock rate — timing is the virtual clock's concern
/// ([`crate::ClockConfig`]), and there is exactly one modelled real-mode core.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum CpuChip {
    /// Intel 80286.
    #[default]
    I80286,
}

/// Where the CPU starts executing after reset (§2.1).
///
/// The two forms capture *intent*, not different mechanisms — both fix a
/// real-mode `CS:IP`. A [`ResetSpec::ResetVector`] is the architectural power-on
/// vector, where a masked BIOS ROM ([`crate::LoadImage`] of kind
/// [`crate::ImageKind::Rom`]) supplies the first instruction; a
/// [`ResetSpec::Entry`] points straight at the entry of a blob loaded into RAM
/// (a bare boot blob or a diagnostic image).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResetSpec {
    /// The architectural reset vector; code is fetched from a ROM mapped there.
    ResetVector {
        /// Reset code-segment selector (real mode: base = selector << 4).
        cs: u16,
        /// Reset instruction pointer within the segment.
        ip: u16,
    },
    /// A direct entry into a RAM-loaded blob.
    Entry {
        /// Entry code-segment selector.
        cs: u16,
        /// Entry instruction pointer within the segment.
        ip: u16,
    },
}

impl ResetSpec {
    /// The standard PC/AT reset vector `F000:FFF0`.
    pub const fn reset_vector() -> Self {
        Self::ResetVector {
            cs: 0xF000,
            ip: 0xFFF0,
        }
    }

    /// The reset code-segment selector, whichever form this is.
    pub fn cs(&self) -> u16 {
        match self {
            Self::ResetVector { cs, .. } | Self::Entry { cs, .. } => *cs,
        }
    }

    /// The reset instruction pointer, whichever form this is.
    pub fn ip(&self) -> u16 {
        match self {
            Self::ResetVector { ip, .. } | Self::Entry { ip, .. } => *ip,
        }
    }
}

impl Default for ResetSpec {
    fn default() -> Self {
        Self::reset_vector()
    }
}

/// The real-mode data segments and stack pointer a guest starts with.
///
/// Mirrors [`cpu::x86jit_backend::RealSegments`] as owned, comparable data (the
/// CPU-core type is not [`Eq`], and the whole config must be). The reset `CS:IP`
/// is separate, in [`ResetSpec`]; this groups DS/ES/SS and the initial SP. A
/// [`From`] conversion into the CPU-core type is provided for the assembler.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct SegmentSeed {
    /// Data segment selector.
    pub ds: u16,
    /// Extra segment selector.
    pub es: u16,
    /// Stack segment selector.
    pub ss: u16,
    /// Initial stack pointer (offset within SS).
    pub sp: u16,
}

impl SegmentSeed {
    /// All three data segments equal to `sel` with the CPU-core default stack
    /// top — the flat low-memory layout a boot blob uses.
    pub fn flat(sel: u16) -> Self {
        Self {
            ds: sel,
            es: sel,
            ss: sel,
            sp: RealSegments::DEFAULT_SP,
        }
    }
}

impl From<SegmentSeed> for RealSegments {
    fn from(s: SegmentSeed) -> Self {
        RealSegments {
            ds: s.ds,
            es: s.es,
            ss: s.ss,
            sp: s.sp,
        }
    }
}

/// The CPU section of a [`crate::MachineConfig`] (§2).
///
/// Minimal today — a marker chip, a reset entry, and the seeded segments — but
/// the home for future CPU options that must not force a config restructure.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CpuConfig {
    /// The modelled processor (marker only).
    pub chip: CpuChip,
    /// Where execution begins after reset.
    pub reset: ResetSpec,
    /// The real-mode data segments and stack the guest starts with.
    pub segments: SegmentSeed,
}
