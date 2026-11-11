// SPDX-License-Identifier: AGPL-3.0-or-later
//! Public lowering entry points; see machine-files § Errors.
//! Lowering checks image integrity and records pin provenance where a mismatch is detected.

use crate::{ImageReader, LowerError, MachineFile, Origins};

/// Lower a complete document, retaining the defining pin's source in mismatches.
/// Resolve inheritance first with [`crate::resolve`]. Assembly owns hardware
/// validation; image reads and hashing remain in the lowering implementation.
pub fn lower<R: ImageReader>(
    document: &MachineFile,
    origins: &Origins,
    reader: &mut R,
) -> Result<assembly::MachineConfig, LowerError> {
    lower_with_options(
        document,
        origins,
        reader,
        assembly::AssemblyOptions::default(),
    )
}

/// Lower with run-only firmware permissions while retaining source diagnostics.
/// The option relaxes catalogue membership alone, never format or compatibility.
pub fn lower_with_options<R: ImageReader>(
    document: &MachineFile,
    origins: &Origins,
    reader: &mut R,
    options: assembly::AssemblyOptions,
) -> Result<assembly::MachineConfig, LowerError> {
    crate::lower::lower(document, origins, reader, options)
}
