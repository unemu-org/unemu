// SPDX-License-Identifier: AGPL-3.0-or-later
//! Contract boundary checks without knowledge of any platform's hardware.
use recipe::template::{compose, Facts, VerifiedUse};
use std::{collections::BTreeMap, sync::Mutex};
use unemu_core_api::{
    test_support::{check_prepared, MemoryStorage, ScriptedBindingCore},
    BindingFacts, BindingItem, BindingKind, BindingSlot, BindingUse, ConfigDocument, Content,
    ContentSource, Core, CoreDescription, Error, ErrorKind, PreparedConfig,
};

struct Source(BTreeMap<([u8; 32], u64), Vec<u8>>);
impl ContentSource for Source {
    fn open(&self, digest: &[u8; 32], size: u64) -> Result<Content, Error> {
        self.0
            .get(&(*digest, size))
            .cloned()
            .map(Content::Bytes)
            .ok_or_else(|| Error::new(ErrorKind::Content, "not an allowed identity"))
    }
}
struct Observed {
    calls: Mutex<Vec<Vec<BindingUse>>>,
}
impl Core for Observed {
    fn description(&self) -> &CoreDescription {
        ScriptedBindingCore.description()
    }
    fn validate(&self, p: &str, c: ConfigDocument<'_>) -> Result<unemu_core_api::Validated, Error> {
        ScriptedBindingCore.validate(p, c)
    }
    fn create(
        &self,
        r: &unemu_core_api::CreateRequest<'_>,
    ) -> Result<Box<dyn unemu_core_api::Machine>, Error> {
        ScriptedBindingCore.create(r)
    }
    fn binding_slots(&self, p: &str, c: ConfigDocument<'_>) -> Result<Vec<BindingSlot>, Error> {
        ScriptedBindingCore.binding_slots(p, c)
    }
    fn compose(
        &self,
        p: &str,
        c: ConfigDocument<'_>,
        facts: BindingFacts<'_>,
    ) -> Result<PreparedConfig, Error> {
        self.calls.lock().unwrap().push(facts.uses.to_vec());
        ScriptedBindingCore.compose(p, c, facts)
    }
}
fn facts(reverse: bool) -> Facts {
    let mut packages: Vec<_> = ["bios", "media"]
        .into_iter()
        .enumerate()
        .map(|(i, id)| {
            let bytes = vec![i as u8 + 1; 16];
            let digest = image_store::Digest::of(&bytes);
            let hash = std::array::from_fn(|i| {
                u8::from_str_radix(&digest.hex()[i * 2..i * 2 + 2], 16).unwrap()
            });
            let mut items = vec![
                BindingItem::new(
                    "selected",
                    if i == 0 {
                        BindingKind::FirmwareRom
                    } else {
                        BindingKind::Medium
                    },
                    16,
                    hash,
                )
                .with_roles(vec!["system".into(), "other".into()]),
                BindingItem::new("unselected", BindingKind::Medium, 16, hash),
            ];
            if reverse {
                items.reverse();
                items.iter_mut().for_each(|i| i.roles.reverse());
            }
            VerifiedUse {
                facts: BindingUse::new(id, format!("package-{id}"), items),
                content: Box::new(Source([((hash, 16), bytes)].into())),
            }
        })
        .collect();
    if reverse {
        packages.reverse();
    }
    Facts::new(packages)
}
const CONFIG: &[u8] = b"a bios selected firmware system 16 -\nb media selected medium - 16 -\nc - system zero - 512 -\n";
fn document(bytes: &[u8]) -> ConfigDocument<'_> {
    ConfigDocument::new("application/x-scripted-bindings", bytes)
}

#[test]
fn discovery_refuses_disk_and_use_mismatches_before_composition() {
    let core = Observed {
        calls: Mutex::new(Vec::new()),
    };
    let facts = facts(false);
    for (bytes, disks, expected) in [
        (CONFIG.to_vec(), vec![], "missing disk"),
        (
            CONFIG.to_vec(),
            vec![("system".into(), 512), ("extra".into(), 512)],
            "unused disk",
        ),
        (
            CONFIG.to_vec(),
            vec![("system".into(), 1024)],
            "size differs",
        ),
        (
            b"a bios selected firmware system 16 -\nc - system zero - 512 -\n".to_vec(),
            vec![("system".into(), 512)],
            "unused use: media",
        ),
        (
            b"a unknown selected firmware system 16 -\nc - system zero - 512 -\n".to_vec(),
            vec![("system".into(), 512)],
            "undeclared use: unknown",
        ),
    ] {
        let error = compose(
            &core as &dyn Core,
            "scripted",
            document(&bytes),
            &disks,
            &facts,
        )
        .unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    assert!(core.calls.lock().unwrap().is_empty());
}

#[test]
fn composition_passes_exact_facts_and_is_independent_of_their_order() {
    let core = Observed {
        calls: Mutex::new(Vec::new()),
    };
    let mut results = Vec::new();
    for reverse in [false, true] {
        let facts = facts(reverse);
        let (prepared, _) = compose(
            &core as &dyn Core,
            "scripted",
            document(CONFIG),
            &[("system".into(), 512)],
            &facts,
        )
        .unwrap();
        assert_eq!(core.calls.lock().unwrap().last(), Some(&facts.uses));
        check_prepared(&ScriptedBindingCore as &dyn Core, "scripted", &prepared).unwrap();
        let content = Source(
            prepared
                .validated
                .content
                .iter()
                .map(|item| {
                    let content = facts.open(&item.sha256, item.size).unwrap();
                    let mut bytes = vec![0; item.size as usize];
                    unemu_core_api::ContentReader::read_at(&content, 0, &mut bytes).unwrap();
                    ((item.sha256, item.size), bytes)
                })
                .collect(),
        );
        let request = unemu_core_api::CreateRequest::new(
            "scripted",
            prepared.config.as_document(),
            &content,
            std::rc::Rc::new(MemoryStorage::default()),
        );
        ScriptedBindingCore.create(&request).unwrap();
        results.push(prepared);
    }
    assert_eq!(results[0], results[1]);
}
