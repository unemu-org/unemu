// SPDX-License-Identifier: AGPL-3.0-or-later
//! The PC's entry in a front end's list of cores (core-contract.md §4).
//!
//! In this stage every PC is assembled by the PC provider and wrapped with
//! [`crate::PcMachine::from_assembled`]; [`Core::create`] cannot read a
//! stored config yet, so the platform lists no config type and no front end
//! offers it.

use crate::machine::PC_FEATURES;
use unemu_core_api::{
    ContractVersion, Core, CoreDescription, CreateRequest, Error, ErrorKind, Machine, Notice,
    PlatformDescription,
};

/// The platform id stored PC images name.
const PLATFORM: &str = "ibm-pc";

static PLATFORMS: [PlatformDescription; 1] =
    [PlatformDescription::new(PLATFORM, "IBM PC compatible").with_features(PC_FEATURES)];

static NOTICES: [Notice; 2] = [
    Notice::new("Nuked-OPL3", devices::OPL3_NOTICE),
    Notice::new("Ultimate Oldschool PC Font Pack", crate::FONT_NOTICE),
];

static DESCRIPTION: CoreDescription = CoreDescription::new(
    ContractVersion::new(0, 1),
    "unemupc",
    "unemupc",
    env!("CARGO_PKG_VERSION"),
)
.with_platforms(&PLATFORMS)
.with_notices(&NOTICES);

/// The PC core's entry in a front end's list of cores.
#[derive(Clone, Copy, Debug, Default)]
pub struct PcCore;

impl Core for PcCore {
    fn description(&self) -> &CoreDescription {
        &DESCRIPTION
    }

    /// Not in this stage: the PC provider assembles every PC itself.
    fn create(&self, _request: &CreateRequest<'_>) -> Result<Box<dyn Machine>, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "the PC core does not read stored configs yet; its provider starts PCs",
        ))
    }
}
