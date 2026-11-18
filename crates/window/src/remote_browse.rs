// SPDX-License-Identifier: AGPL-3.0-or-later
//! Remotes' model is `library_model::remote_browse`, re-exported here under
//! its old path. Its tests that read PC artifacts live beside the library
//! worker in `pc_library`; the one that pulls with the program's provider
//! list stays here.

pub(crate) use library_model::remote_browse::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pc_desktop::PcDesktop, registry_transfer::Accounts};
    use image_registry::{
        browse::MIN_APP_VERSION,
        credential_store::SessionStore,
        credentials::EnvCredentials,
        test_support::{MockRegistry, Policy},
        Reference,
    };
    use image_store::{media_type as mt, Descriptor, Digest, Manifest, Store};
    use std::sync::{Arc, Mutex};

    /// A synthetic artifact: its manifest and every blob it names.
    struct Image {
        bytes: Vec<u8>,
        digest: Digest,
        config: Vec<u8>,
        layers: Vec<Vec<u8>>,
    }

    impl Image {
        fn new(manifest: Manifest, config: Vec<u8>, layers: Vec<Vec<u8>>) -> Self {
            let bytes = manifest.encode().unwrap();
            Self {
                digest: Digest::of(&bytes),
                bytes,
                config,
                layers,
            }
        }

        /// Publish it to `registry` as `repository:tag`.
        fn seed(&self, registry: &MockRegistry, repository: &str, tag: &str) {
            registry.seed_blob(repository, &self.config);
            for layer in &self.layers {
                registry.seed_blob(repository, layer);
            }
            registry.seed_manifest(repository, tag, &self.bytes);
        }
    }

    fn blob(kind: &str, bytes: &[u8]) -> Descriptor {
        Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64)
    }

    /// [`metadata`] of a machine family's artifact.
    fn metadata_of(family: &str, label: &str) -> Vec<u8> {
        format!(
            r#"{{"schema":1,"platform":"{family}","titles":[{{"id":"t","title":"{label}","edition":"{label} edition","version":"{label} version"}}]}}"#
        )
        .into_bytes()
    }

    /// A PC machine image whose one payload holds `label`.
    fn machine(label: &str, annotations: &[(&str, &str)]) -> Image {
        machine_of(pc_image::PLATFORM, label, annotations)
    }

    /// A machine image of the machine family `family`.
    fn machine_of(family: &str, label: &str, annotations: &[(&str, &str)]) -> Image {
        let payload = label.as_bytes().to_vec();
        let config = format!(
            "schema = 1\nplatform = \"{family}\"\nblobs = [\"{}\"]\n",
            Digest::of(&payload)
        )
        .into_bytes();
        let mut manifest = Manifest::new(
            family,
            blob(mt::CONFIG, &config),
            vec![
                blob(mt::FIRMWARE, &payload),
                blob(mt::METADATA, &metadata_of(family, label)),
            ],
        )
        .unwrap();
        for (key, value) in annotations {
            manifest.annotations.insert((*key).into(), (*value).into());
        }
        Image::new(manifest, config, vec![payload, metadata_of(family, label)])
    }

    /// A store, preferences and the accounts that read them, each in its own
    /// directory.
    struct Setup {
        _temp: assembly::test_support::TempDir,
        store: Store,
        accounts: Accounts,
    }

    impl Setup {
        /// Preferences that list `entries` (TOML `[[registries]]` bodies).
        fn new(entries: &[String]) -> Self {
            let temp = assembly::test_support::TempDir::new("remote-browse");
            let store = Store::open(temp.path().join("store")).unwrap();
            let config = temp.path().join("config");
            std::fs::create_dir(&config).unwrap();
            let preferences = config.join("preferences.toml");
            let text: String = entries
                .iter()
                .map(|e| format!("[[registries]]\n{e}\n"))
                .collect();
            std::fs::write(&preferences, text).unwrap();
            let accounts = Accounts {
                path: Some(preferences.clone()),
                store: Arc::new(SessionStore::default()),
                env: EnvCredentials::default(),
                default_override: None,
            };
            Self {
                _temp: temp,
                store,
                accounts,
            }
        }

        /// Preferences that list the mock registry as a whole-registry entry.
        fn with(registry: &MockRegistry) -> Self {
            Self::new(&[plain(&registry.host())])
        }

        fn remotes(&self) -> Remotes {
            Remotes::open(self.accounts.clone())
        }

        /// Run one request as the worker would and hand its answers to `remotes`.
        fn answer(
            &self,
            remotes: &mut Remotes,
            request: Option<BrowseRequest>,
        ) -> Vec<BrowseEvent> {
            let events = Mutex::new(Vec::new());
            let pc = PcDesktop::new(&[], &[]);
            run(&self.store, request.expect("a request"), &[&pc], &|event| {
                events.lock().unwrap().push(event)
            });
            let events = events.into_inner().unwrap();
            for event in events.clone() {
                remotes.on_event(event);
            }
            events
        }
    }

    fn plain(host: &str) -> String {
        format!("host = \"{host}\"\nplain_http = true")
    }

    fn open_registry() -> Policy {
        Policy {
            anonymous_read: vec!["*".into()],
            ..Policy::default()
        }
    }

    /// `GET` requests for blobs, by digest.
    fn blob_requests(registry: &MockRegistry) -> Vec<String> {
        registry
            .log()
            .into_iter()
            .filter(|line| line.starts_with("GET ") && line.contains("/blobs/"))
            .filter_map(|line| line.rsplit('/').next().map(str::to_owned))
            .collect()
    }

    /// A tag this release cannot run is listed with the release it needs and
    /// cannot be pulled from the list; a pull by its digest that reaches the
    /// worker anyway is refused before any blob is fetched.
    #[test]
    fn a_tag_needing_a_newer_release_is_refused_before_any_blob() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        let future = machine("future", &[(MIN_APP_VERSION, "99.0.0")]);
        future.seed(&registry, "games/future", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let row = remotes
            .registry(&host)
            .unwrap()
            .repository("games/future")
            .unwrap()
            .representative_row()
            .unwrap()
            .clone();
        let pick = crate::remote_pull::Pick {
            host: &host,
            repository: "games/future",
            tag: "1.0.0",
            row: Some(&row),
            blocked: None,
            facts: &PcDesktop::facts(),
        };
        assert_eq!(
            crate::remote_pull::refusal(&pick).as_deref(),
            Some("Needs unemupc 99.0.0 or newer: Pull refuses it before fetching anything.")
        );
        registry.clear_log();
        let source = Reference {
            host: host.clone(),
            repository: "games/future".into(),
            tag: Some("1.0.0".into()),
            digest: Some(future.digest.clone()),
        };
        let (_, request) = crate::registry_transfer::PullRun::start(
            &setup.accounts,
            source,
            row.local_tag.clone(),
        );
        let events = Mutex::new(Vec::new());
        platforms::with_image_providers(|providers| {
            let pc = crate::pc_desktop::PcDesktop::new(providers, &[]);
            crate::registry_transfer::run(
                &setup.store,
                request,
                &[&pc],
                &|event| events.lock().unwrap().push(event),
                &|| {},
            )
        });
        let events = events.into_inner().unwrap();
        let [crate::registry_transfer::TransferEvent::Pulled {
            result: Err(crate::registry_transfer::PullFailure::Failed(message)),
            ..
        }] = events.as_slice()
        else {
            panic!("the pull is refused")
        };
        assert!(
            message.contains("needs unemupc 99.0.0 or newer"),
            "{message}"
        );
        assert!(blob_requests(&registry).is_empty(), "{:?}", registry.log());
        assert!(setup.store.tag_descriptor(&row.local_tag).is_err());
    }
}
