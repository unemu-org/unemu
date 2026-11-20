// SPDX-License-Identifier: AGPL-3.0-or-later
//! The compiled-in PC factory through the front end's start from parts.
//! See `backlog/docs/design/core-contract.md` §14.2.5.

use core_launch::{ContentPlan, Parts, Prepared, StoragePlan};
use pc_config::{image::Digest, launch};
use std::rc::Rc;
use unemu_core_api::{
    test_support::MemoryStorage, ConfigDocumentBuf, Content, ContentSource, Error, ErrorKind,
    Machine, RunLimit, Storage,
};

#[derive(Clone)]
struct Firmware(Vec<u8>);
impl ContentSource for Firmware {
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error> {
        if *sha256 != Digest::of(&self.0).sha256() || size != self.0.len() as u64 {
            return Err(Error::new(ErrorKind::Content, "unlisted content"));
        }
        Ok(Content::Bytes(self.0.clone()))
    }
}
impl ContentPlan for Firmware {
    fn open(&self) -> Result<Box<dyn ContentSource>, Error> {
        Ok(Box::new(self.clone()))
    }
}
struct Records;
impl StoragePlan for Records {
    fn open(&self) -> Result<Rc<dyn Storage>, Error> {
        Ok(Rc::new(MemoryStorage::new()))
    }
}

#[test]
fn the_compiled_in_pc_starts_a_launch_document_from_parts_in_both_forms() {
    let cores = core_launch::Cores::new(platforms::cores());
    let core = cores.for_platform("ibm-pc").unwrap();
    let firmware = Firmware(vec![0xf4; 32768]);
    let digest = Digest::of(&firmware.0);
    let document = format!(
        r#"schema = 1
platform = "ibm-pc"
[machine]
schema = 1
[machine.board]
model = "ibm-5170"
[machine.firmware.bios]
image = "assets/bios.bin"
sha256 = "{hash}"
[[machine.card]]
kind = "mda"
[[firmware]]
key = "firmware.bios"
source = "assets/bios.bin"
blob = "{digest}"
[[content]]
digest = "{digest}"
sha256 = "{hash}"
size = 32768
"#,
        hash = digest.hex()
    );
    let config = ConfigDocumentBuf::new(launch::MEDIA_TYPE, document.into_bytes());
    core.validate("ibm-pc", config.as_document()).unwrap();
    let prepared = Prepared::from_parts(Parts {
        core,
        platform: "ibm-pc",
        title: "Test PC".into(),
        notes: vec![],
        config,
        content: Box::new(firmware),
        storage: Box::new(Records),
        options: vec![("allow-unlisted-firmware".into(), "true".into())],
    });
    let mut boxed = core_launch::start(&prepared).unwrap();
    assert_eq!(
        boxed.run(RunLimit::steps(10)).unwrap(),
        unemu_core_api::Stop::Halted
    );
    boxed.reset(unemu_core_api::ResetKind::Soft).unwrap();
    boxed.finish().unwrap();
    drop(boxed);
    let mut typed = core_launch::start_with(&prepared, |request| {
        unemu_pc_core::PcCore.create_machine(request)
    })
    .unwrap();
    assert!(typed
        .config()
        .devices
        .iter()
        .any(|device| device.id == assembly::DeviceId::Mda));
    typed.run(RunLimit::steps(10)).unwrap();
    typed.finish().unwrap();
}
