// SPDX-License-Identifier: AGPL-3.0-or-later
//! The list of cores: which are usable, which are refused and why, and the
//! About lines.

use super::*;
use unemu_core_api::{CreateRequest, Error, ErrorKind, Machine};
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
    ContractVersion::new(0, 1),
    "older",
    "Older core",
    "1.4.0",
));
static NEXT_MINOR: Stating = Stating(CoreDescription::new(
    ContractVersion::new(0, 3),
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
    ContractVersion::new(0, 2),
    "same",
    "Same core",
    "0.1.0",
));

static LIST: [&dyn Core; 5] = [&OLDER_MINOR, &TestCore, &NEXT_MINOR, &SAME, &OTHER_MAJOR];

fn ids(cores: impl Iterator<Item = &'static dyn Core>) -> Vec<&'static str> {
    cores.map(|core| core.description().id).collect()
}

/// The programs are built against contract 0.2; the scripted cores above
/// are written for that.
#[test]
fn the_tests_assume_the_contract_this_program_drives_is_0_2() {
    assert_eq!(CONTRACT, ContractVersion::new(0, 2));
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
                "core Older core implements contract 0.1; this program needs 0.2"
            ),
            (
                "next",
                "core Next core implements contract 0.3; this program needs 0.2"
            ),
            (
                "major",
                "core Major core implements contract 1.2; this program needs 0.2"
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
            "core older 1.4.0 (Older core), contract 0.1, unusable: core Older core implements contract 0.1; this program needs 0.2".to_owned(),
            format!("core unemu-test-core {test_core} (unemu test core), contract 0.2"),
            "core next 0.9.1 (Next core), contract 0.3, unusable: core Next core implements contract 0.3; this program needs 0.2".to_owned(),
            "core same 0.1.0 (Same core), contract 0.2".to_owned(),
            "core major 2.0.0 (Major core), contract 1.2, unusable: core Major core implements contract 1.2; this program needs 0.2".to_owned(),
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
