// SPDX-License-Identifier: AGPL-3.0-or-later
//! The list of cores: which are usable, which are refused and why, and the
//! About lines.

use super::*;
use unemu_core_api::{CreateRequest, Error, ErrorKind, Machine, PlatformDescription};
use unemu_test_core::TestCore;

/// A core that states `implements` and can do nothing else.
struct Stating(CoreDescription);

impl Core for Stating {
    fn description(&self) -> &CoreDescription {
        &self.0
    }

    fn create(&self, _request: &CreateRequest<'_>) -> Result<Box<dyn Machine>, Error> {
        Err(Error::new(ErrorKind::Unsupported, "a scripted description"))
    }
}

static OLDER_MINOR: Stating = Stating(CoreDescription::new(
    ContractVersion::new(0, 4),
    "older",
    "Older core",
    "1.4.0",
));
static NEXT_MINOR: Stating = Stating(CoreDescription::new(
    ContractVersion::new(0, 7),
    "next",
    "Next core",
    "0.9.1",
));
static OTHER_MAJOR: Stating = Stating(CoreDescription::new(
    ContractVersion::new(1, 2),
    "major",
    "Major core",
    "2.0.0",
));
static SAME: Stating = Stating(CoreDescription::new(
    ContractVersion::new(0, 6),
    "same",
    "Same core",
    "0.1.0",
));

static LIST: [&dyn Core; 5] = [&OLDER_MINOR, &TestCore, &NEXT_MINOR, &SAME, &OTHER_MAJOR];

fn ids(cores: impl Iterator<Item = &'static dyn Core>) -> Vec<&'static str> {
    cores.map(|core| core.description().id).collect()
}

/// The programs are built against contract 0.6; the scripted cores above
/// are written for that.
#[test]
fn the_tests_assume_the_contract_this_program_drives_is_0_6() {
    assert_eq!(CONTRACT, ContractVersion::new(0, 6));
}

#[test]
fn a_core_that_states_another_contract_is_refused_with_the_reason() {
    let cores = Cores::new(&LIST);
    let refused: Vec<_> = cores
        .refused()
        .iter()
        .map(|(description, reason)| (description.id, reason.as_str()))
        .collect();
    assert_eq!(
        refused,
        [
            (
                "older",
                "core Older core implements contract 0.4; this program needs 0.6"
            ),
            (
                "next",
                "core Next core implements contract 0.7; this program needs 0.6"
            ),
            (
                "major",
                "core Major core implements contract 1.2; this program needs 0.6"
            ),
        ]
    );
}

#[test]
fn usable_cores_keep_the_order_of_the_list() {
    let cores = Cores::new(&LIST);
    assert_eq!(ids(cores.usable()), ["unemu-test-core", "same"]);
}

#[test]
fn a_refused_core_is_not_found_by_its_id() {
    let cores = Cores::new(&LIST);
    for id in ["older", "next", "major", "nobody"] {
        assert!(cores.by_id(id).is_none(), "{id}");
    }
    for id in ["unemu-test-core", "same"] {
        assert_eq!(cores.by_id(id).map(|core| core.description().id), Some(id));
    }
}

#[test]
fn the_about_lines_list_every_core_in_order_and_say_which_are_unusable() {
    let test_core = TestCore.description().version;
    assert_eq!(
        Cores::new(&LIST).about_lines(),
        [
            "core older 1.4.0 (Older core), contract 0.4, unusable: core Older core implements contract 0.4; this program needs 0.6".to_owned(),
            format!("core unemu-test-core {test_core} (unemu test core), contract 0.6"),
            "core next 0.9.1 (Next core), contract 0.7, unusable: core Next core implements contract 0.7; this program needs 0.6".to_owned(),
            "core same 0.1.0 (Same core), contract 0.6".to_owned(),
            "core major 2.0.0 (Major core), contract 1.2, unusable: core Major core implements contract 1.2; this program needs 0.6".to_owned(),
        ]
    );
}

#[test]
fn an_empty_list_lists_nothing() {
    let cores = Cores::new(&[]);
    assert_eq!(cores.usable().count(), 0);
    assert!(cores.refused().is_empty());
    assert!(cores.about_lines().is_empty());
}

/// Within one major after 0, a core of an older minor is still driven: the
/// rule is the contract's own.
#[test]
fn the_check_is_the_contract_s_rule() {
    let older = CoreDescription::new(ContractVersion::new(1, 0), "a", "A", "1.0.0");
    assert_eq!(refusal(ContractVersion::new(1, 3), &older), None);
    assert_eq!(
        refusal(ContractVersion::new(2, 0), &older).as_deref(),
        Some("core A implements contract 1.0; this program needs 2.0")
    );
}

static TEST_PLATFORM: [PlatformDescription; 1] = [PlatformDescription::new(
    unemu_test_core::PLATFORM,
    "Test machine",
)];
/// A core of another contract that states the test core's platform.
static OLD_TEST: Stating = Stating(
    CoreDescription::new(
        ContractVersion::new(0, 4),
        "old-test",
        "Old test core",
        "0.1.0",
    )
    .with_platforms(&TEST_PLATFORM),
);
/// A second usable core of the test core's platform.
static SECOND_TEST: Stating = Stating(
    CoreDescription::new(
        ContractVersion::new(0, 6),
        "second-test",
        "Second test core",
        "0.1.0",
    )
    .with_platforms(&TEST_PLATFORM),
);

#[test]
fn a_platform_is_found_in_the_first_usable_core_that_provides_it() {
    static PLATFORMS: [&dyn Core; 4] = [&OLD_TEST, &SAME, &TestCore, &SECOND_TEST];
    let cores = Cores::new(&PLATFORMS);
    // The refused core is never asked; the first usable one answers.
    let found = cores.for_platform(unemu_test_core::PLATFORM).unwrap();
    assert_eq!(found.description().id, unemu_test_core::CORE_ID);
    // The id is compared as it is: no alias and no other case.
    for platform in ["Unemu-Test", "unemu-test ", "nes", ""] {
        assert!(cores.for_platform(platform).is_none(), "{platform:?}");
    }
    // Only a refused core provides it: none.
    static REFUSED_ONLY: [&dyn Core; 2] = [&OLD_TEST, &SAME];
    assert!(Cores::new(&REFUSED_ONLY)
        .for_platform(unemu_test_core::PLATFORM)
        .is_none());
}
