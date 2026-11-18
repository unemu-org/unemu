// SPDX-License-Identifier: AGPL-3.0-or-later
//! The tests of Remotes' model (`library_model::remote_browse`) that read PC
//! artifacts or browse through the library worker. They live beside the
//! worker; the model itself is platform-neutral and holds the tests that
//! need no PC.

use library_model::remote_browse::*;

mod tests {

    use super::*;
    use crate::pc_desktop::PcDesktop;
    use image_registry::{
        browse::MIN_APP_VERSION,
        client::Challenge,
        credential_store::{Secret, SessionStore},
        credentials::EnvCredentials,
        test_support::{MockRegistry, Policy},
    };
    use image_store::{
        media_set, media_type as mt, BlobSource, ConfigValidator, Descriptor, Digest, Manifest,
        Store,
    };
    use library_model::registry_transfer::Accounts;
    use library_model::remote_browse::test_support::{LATEST_LOOKUPS, TAG_PAGE, TITLE, VERSION};
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::{atomic::Ordering, mpsc, Arc, Mutex},
    };

    /// A stand-in for the PC provider for local tags: the config lists its
    /// payload digests.
    struct Synthetic;
    impl ConfigValidator for Synthetic {
        fn platform(&self) -> &str {
            pc_image::PLATFORM
        }
        fn validate(
            &self,
            table: &toml::Table,
            _: &[Descriptor],
            _: &dyn BlobSource,
        ) -> image_store::Result<Vec<Digest>> {
            table
                .get("blobs")
                .and_then(toml::Value::as_array)
                .ok_or_else(|| image_store::Error::Invalid("missing blobs".into()))?
                .iter()
                .map(|v| v.as_str().unwrap_or_default().parse())
                .collect()
        }
    }

    /// A synthetic artifact: its manifest and every blob it names.
    struct Image {
        manifest: Manifest,
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
                manifest,
                bytes,
                config,
                layers,
            }
        }

        fn layer_digests(&self) -> Vec<Digest> {
            self.layers.iter().map(|l| Digest::of(l)).collect()
        }

        /// Publish it to `registry` as `repository:tag`.
        fn seed(&self, registry: &MockRegistry, repository: &str, tag: &str) {
            registry.seed_blob(repository, &self.config);
            for layer in &self.layers {
                registry.seed_blob(repository, layer);
            }
            registry.seed_manifest(repository, tag, &self.bytes);
        }

        /// Tag it in the library as `tag`.
        fn tag(&self, store: &Store, tag: &str) {
            for blob in std::iter::once(&self.config).chain(&self.layers) {
                store.import(blob.as_slice(), None).unwrap();
            }
            let descriptor = store.import_manifest(&self.manifest).unwrap();
            assert_eq!(descriptor.digest, self.digest);
            store
                .update_tag(tag, &descriptor, None, &Synthetic)
                .unwrap();
        }
    }

    fn blob(kind: &str, bytes: &[u8]) -> Descriptor {
        Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64)
    }

    /// A metadata record with one title, its edition and version named after
    /// `label`.
    fn metadata(label: &str) -> Vec<u8> {
        metadata_of(pc_image::PLATFORM, label)
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

    /// An installation media set of one disk per format.
    fn media_set(label: &str, formats: &[&str]) -> Image {
        let disks: Vec<Vec<u8>> = (0..formats.len())
            .map(|i| format!("{label} disk {i}").into_bytes())
            .collect();
        let config = media_set::Config {
            schema: 1,
            platform: pc_image::PLATFORM.into(),
            media: formats
                .iter()
                .zip(&disks)
                .enumerate()
                .map(|(i, (format, disk))| media_set::Medium {
                    id: format!("disk-{i}"),
                    label: format!("Disk {i}"),
                    format: pc_image::media_set::format(format).unwrap(),
                    blob: Digest::of(disk),
                })
                .collect(),
            extras: Vec::new(),
        }
        .encode()
        .unwrap();
        let mut layers: Vec<Descriptor> = disks.iter().map(|d| blob(mt::FLOPPY, d)).collect();
        layers.push(blob(mt::METADATA, &metadata(label)));
        let mut manifest = Manifest::new_media_set(
            pc_image::PLATFORM,
            blob(mt::MEDIA_SET_CONFIG, &config),
            layers,
        )
        .unwrap();
        manifest
            .annotations
            .insert(TITLE.into(), format!("{label} set"));
        let mut all = disks;
        all.push(metadata(label));
        Image::new(manifest, config, all)
    }

    /// A store, preferences and the accounts that read them, each in its own
    /// directory.
    struct Setup {
        _temp: assembly::test_support::TempDir,
        store: Store,
        preferences: PathBuf,
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
                preferences,
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

    /// Run `request` on a thread until `registry` holds the request that
    /// starts with `prefix`, handing the answers so far to `remotes`; then
    /// press Stop for `host`, release the request and hand over the rest.
    /// Returns the answers after Stop.
    fn stop_while_held(
        setup: &Setup,
        remotes: &mut Remotes,
        registry: &MockRegistry,
        host: &str,
        prefix: &str,
        request: Option<BrowseRequest>,
    ) -> Vec<BrowseEvent> {
        registry.hold_next(prefix);
        let request = request.expect("a request");
        let (send, receive) = mpsc::channel();
        let root = setup.store.root().to_owned();
        let worker = std::thread::spawn(move || {
            let store = Store::open(root).unwrap();
            let pc = PcDesktop::new(&[], &[]);
            run(&store, request, &[&pc], &|event| send.send(event).unwrap());
        });
        for () in crate::test_wait::rounds() {
            if registry.held() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(registry.held(), "{prefix} was never asked");
        for event in receive.try_iter() {
            remotes.on_event(event);
        }
        remotes.stop(host);
        registry.release();
        worker.join().unwrap();
        let late: Vec<_> = receive.try_iter().collect();
        for event in late.clone() {
            remotes.on_event(event);
        }
        late
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

    fn ended(events: &[BrowseEvent]) -> &End {
        match events.last() {
            Some(BrowseEvent::Ended { end, .. }) => end,
            other => panic!("the job did not end: {other:?}"),
        }
    }

    fn names(view: &RegistryView) -> Vec<&str> {
        view.repositories.iter().map(|r| r.name.as_str()).collect()
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

    #[test]
    fn tags_follow_the_registry_paging_and_only_the_newest_is_read() {
        let registry = MockRegistry::start(open_registry());
        let image = machine("many", &[]);
        let tags: Vec<String> = (0..=TAG_PAGE).map(|i| format!("1.0.{i}")).collect();
        for tag in &tags {
            image.seed(&registry, "games/many", tag);
        }
        image.seed(&registry, "games/many", "latest");
        image.seed(&registry, "games/many", "2.0.0-rc.1");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let row = remotes
            .registry(&host)
            .unwrap()
            .repository("games/many")
            .unwrap()
            .clone();
        assert_eq!(row.tags.len(), TAG_PAGE + 3, "every page was read");
        assert_eq!(
            row.representative.as_deref(),
            Some("1.0.100"),
            "the newest release"
        );
        assert_eq!(row.read.len(), 1);
        let log = registry.log();
        assert_eq!(
            log.iter()
                .filter(|line| *line == "GET /v2/games/many/tags/list")
                .count(),
            2,
            "{log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|line| line.starts_with("GET /v2/games/many/manifests/"))
                .count(),
            1,
            "only the representative tag's manifest is read while listing"
        );
        assert_eq!(
            representative(&["latest".into(), "beta".into()]).as_deref(),
            Some("latest")
        );
        assert_eq!(
            representative(&["2.0.0-rc.1".into(), "2.0.0-rc.2".into()]).as_deref(),
            Some("2.0.0-rc.2"),
            "pre-releases when there is no release"
        );
        let pick =
            |tags: &[&str]| representative(&tags.iter().map(|t| t.to_string()).collect::<Vec<_>>());
        assert_eq!(pick(&["1.4", "1.4-720k"]).as_deref(), Some("1.4"));
        assert_eq!(pick(&["1.4-720k", "1.4"]).as_deref(), Some("1.4"));
        // Sets and packages: the alias of the first release stands for its
        // newest revision; without one, the highest revision.
        assert_eq!(
            pick(&["1.4-720k-r1", "1.4-r2", "1.4-r1", "1.4", "1.4-720k"]).as_deref(),
            Some("1.4")
        );
        assert_eq!(
            pick(&["1.4-720k-r1", "1.4-r1", "1.4-r2"]).as_deref(),
            Some("1.4-r2")
        );
        assert_eq!(pick(&["r1", "r10", "r2"]).as_deref(), Some("r10"));
        assert_eq!(pick(&["4x16k-r1", "2x32k-r1"]).as_deref(), Some("2x32k-r1"));
        assert_eq!(
            pick(&["usa-rev1-r1", "usa-r1", "europe-r1"]).as_deref(),
            Some("europe-r1")
        );
        assert_eq!(pick(&["1", "10", "2"]).as_deref(), Some("10"), "firmware");
        assert_eq!(pick(&["stable", "beta"]).as_deref(), Some("beta"));
    }

    #[test]
    fn records_read_the_manifest_and_config_record_but_never_a_layer() {
        let registry = MockRegistry::start(open_registry());
        let set = media_set("Monkey Island", &["720K", "720K", "1.44M"]);
        set.seed(&registry, "private/media/monkey-island", "1.1-vga");
        let game = machine(
            "dune",
            &[
                (TITLE, "Dune II"),
                (VERSION, "1.07"),
                (MIN_APP_VERSION, "99.0.0"),
                ("example.unknown", "kept"),
            ],
        );
        game.seed(&registry, "private/games/dune2", "1.0.0");
        let older = machine("dune old", &[(TITLE, "Dune II")]);
        older.seed(&registry, "private/games/dune2", "0.9.0");
        let oldest = machine("dune oldest", &[(TITLE, "Dune II")]);
        oldest.seed(&registry, "private/games/dune2", "0.8.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);

        let view = remotes.registry(&host).unwrap();
        let row = view.repository("private/media/monkey-island").unwrap();
        let record = row.representative_row().unwrap().record.clone().unwrap();
        assert_eq!(record.artifact_type, mt::MEDIA_SET);
        assert_eq!(record.formats, ["720K", "1.44M"]);
        assert_eq!(record.title.as_deref(), Some("Monkey Island set"));
        assert_eq!(record.digest, set.digest);
        let size =
            set.bytes.len() + set.config.len() + set.layers.iter().map(Vec::len).sum::<usize>();
        assert_eq!(record.size, size as u64);
        assert_eq!(record.min_app_version, None);
        // Every descriptor is kept for Contents…, the config first, and the
        // set's media with their ids for Pull and insert….
        assert_eq!(record.manifest_size, set.bytes.len() as u64);
        assert_eq!(
            record.descriptors,
            std::iter::once(set.manifest.config.clone())
                .chain(set.manifest.layers.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            record.media,
            [("disk-0", "720K"), ("disk-1", "720K"), ("disk-2", "1.44M")]
                .map(|(id, format)| (id.to_owned(), format.to_owned()))
        );

        // 1.0.0 needs a newer release, so the row stands for 0.9.0, the
        // newest version this release runs, and names what 1.0.0 needs.
        let row = view.repository("private/games/dune2").unwrap();
        assert_eq!(row.representative.as_deref(), Some("0.9.0"));
        assert_eq!(
            row.newer_needs,
            [(
                "1.0.0".to_owned(),
                "Needs unemupc 99.0.0 or newer".to_owned()
            )]
        );
        assert_eq!(row.read.len(), 2, "1.0.0, then 0.9.0");
        let record = row.row("1.0.0").unwrap().record.clone().unwrap();
        assert_eq!(record.artifact_type, mt::MACHINE);
        assert_eq!(record.title.as_deref(), Some("Dune II"));
        assert_eq!(record.version.as_deref(), Some("1.07"));
        assert_eq!(
            record.min_app_version.as_deref(),
            Some("99.0.0"),
            "kept raw"
        );
        assert_eq!(record.annotations["example.unknown"], "kept");
        assert!(record.formats.is_empty());

        assert_eq!(
            record.needs_newer().as_deref(),
            Some("Needs unemupc 99.0.0 or newer")
        );

        // Expanding reads the other tag; a read tag is never read again.
        let request = remotes.expand(&host, "private/games/dune2");
        let events = setup.answer(&mut remotes, request);
        assert_eq!(ended(&events), &End::Done);
        let row = remotes
            .registry(&host)
            .unwrap()
            .repository("private/games/dune2")
            .unwrap();
        assert_eq!(row.read.len(), 3);
        assert_eq!(
            row.row("0.8.0").unwrap().record.as_ref().unwrap().digest,
            oldest.digest
        );
        assert_eq!(
            row.row("0.9.0").unwrap().record.as_ref().unwrap().digest,
            older.digest
        );
        assert!(row
            .row("0.8.0")
            .unwrap()
            .record
            .as_ref()
            .unwrap()
            .details
            .is_none());
        assert!(remotes.expand(&host, "private/games/dune2").is_none());

        // Only the media set's config record was fetched while listing and
        // expanding: no machine config, and no layer of any image.
        assert_eq!(
            blob_requests(&registry),
            [Digest::of(&set.config).to_string()]
        );

        // Selecting a tag adds its metadata record, and only that layer.
        registry.clear_log();
        let request = remotes.select(&host, "private/games/dune2", "0.9.0");
        setup.answer(&mut remotes, request);
        let record = |remotes: &Remotes, repository: &str, tag: &str| {
            remotes
                .registry(&host)
                .unwrap()
                .repository(repository)
                .unwrap()
                .row(tag)
                .unwrap()
                .record
                .clone()
                .unwrap()
        };
        assert_eq!(
            record(&remotes, "private/games/dune2", "0.9.0").details,
            Some(Ok(Details {
                edition: Some("dune old edition".into()),
                version: Some("dune old version".into()),
            }))
        );
        assert_eq!(
            blob_requests(&registry),
            [Digest::of(&older.layers[1]).to_string()]
        );
        assert!(
            remotes
                .select(&host, "private/games/dune2", "0.9.0")
                .is_none(),
            "read once"
        );
        registry.clear_log();
        let request = remotes.select(&host, "private/media/monkey-island", "1.1-vga");
        setup.answer(&mut remotes, request);
        let details = record(&remotes, "private/media/monkey-island", "1.1-vga").details;
        assert_eq!(
            details.unwrap().unwrap().edition.as_deref(),
            Some("Monkey Island edition")
        );
        let metadata = set.layers.last().unwrap();
        assert_eq!(
            blob_requests(&registry),
            [
                Digest::of(&set.config).to_string(),
                Digest::of(metadata).to_string()
            ],
            "the set's config record again, then its metadata; never a disk"
        );
        for layer in set.layer_digests().into_iter().chain(game.layer_digests()) {
            if layer != Digest::of(metadata) {
                assert!(!blob_requests(&registry).contains(&layer.to_string()));
            }
        }
    }

    #[test]
    fn stop_ends_the_listing_at_its_next_read_and_keeps_its_rows() {
        let registry = MockRegistry::start(open_registry());
        for (name, label) in [("games/a", "a"), ("games/b", "b"), ("games/c", "c")] {
            machine(label, &[]).seed(&registry, name, "1.0.0");
        }
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        let late = stop_while_held(
            &setup,
            &mut remotes,
            &registry,
            &host,
            "GET /v2/games/b/tags/list",
            request,
        );
        assert_eq!(ended(&late), &End::Stopped, "the worker saw Stop");
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Stopped);
        assert_eq!(names(view), ["games/a"]);
        let banner = view.banner().unwrap();
        assert_eq!(banner.heading, "Listing stopped");
        assert!(
            banner.text.contains("after 1 of 3 repositories"),
            "{}",
            banner.text
        );
        assert!(
            !registry.log().iter().any(|line| line.contains("games/c")),
            "nothing after the held read was asked"
        );
        assert!(
            !registry
                .log()
                .iter()
                .any(|line| line.starts_with("GET /v2/games/b/manifests/")),
            "the stop took effect at the next read"
        );
        // Refresh lists the registry again.
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(names(view), ["games/a", "games/b", "games/c"]);
    }

    #[test]
    fn an_unreachable_registry_keeps_its_earlier_rows_marked_stale() {
        let registry = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        machine("b", &[]).seed(&registry, "games/b", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        assert_eq!(remotes.registry(&host).unwrap().pull_blocked(), None);
        drop(registry);

        let request = remotes.refresh(&host);
        assert!(
            remotes.registry(&host).unwrap().repositories.is_empty(),
            "a refresh replaces the rows"
        );
        let events = setup.answer(&mut remotes, request);
        assert!(matches!(
            ended(&events),
            End::Failed(Failure::Unreachable(_))
        ));
        let view = remotes.registry(&host).unwrap();
        assert!(view.stale());
        assert_eq!(names(view), ["games/a", "games/b"], "the earlier rows stay");
        assert_eq!(view.message(), None, "the rows are shown, under a banner");
        let banner = view.banner().unwrap();
        assert_eq!(banner.heading, format!("{host} does not answer."));
        assert_eq!(banner.action, Some(Action::TryAgain));
        assert!(view.pull_blocked().is_some(), "Pull waits");
        assert!(remotes.expand(&host, "games/a").is_none());

        // Try again while it still does not answer keeps them stale.
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert!(view.stale());
        assert_eq!(names(view), ["games/a", "games/b"]);
    }

    #[test]
    fn server_errors_name_their_status_and_reason() {
        let registry = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        machine("b", &[]).seed(&registry, "games/b", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        registry.fail_requests("GET /v2/_catalog", 500);
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(
            view.state,
            State::ServerError {
                status: Some(500),
                reason: "Internal Server Error (UNKNOWN)".into()
            }
        );
        let message = view.message().unwrap();
        assert_eq!(message.heading, format!("{host} answered with an error"));
        assert!(
            message.text.contains("500 Internal Server Error"),
            "{}",
            message.text
        );
        assert!(view.repositories.is_empty());

        // One repository's failure stays with its row; the others list.
        registry.fail_requests("GET /v2/games/a/tags/list", 503);
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        let failed = view.repository("games/a").unwrap();
        assert!(
            failed.error.as_deref().is_some_and(|e| e.contains("503")),
            "{failed:?}"
        );
        assert!(view
            .repository("games/b")
            .unwrap()
            .representative_row()
            .is_some());

        // A registry without a catalog says so.
        registry.stop_failing();
        let none = MockRegistry::start(Policy {
            no_catalog: true,
            ..open_registry()
        });
        let setup = Setup::with(&none);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&none.host());
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&none.host()).unwrap();
        assert!(matches!(
            view.state,
            State::ServerError {
                status: Some(404),
                ..
            }
        ));
        assert_eq!(
            view.message().unwrap().heading,
            format!("{} does not list its repositories", none.host())
        );
    }

    #[test]
    fn a_login_request_reports_the_challenge_and_keeps_nothing() {
        let registry = MockRegistry::start(Policy {
            user: Some(("ci".into(), "secret".into())),
            catalog_requires_login: true,
            ..Policy::default()
        });
        machine("a", &[]).seed(&registry, "private/games/a", "1.0.0");
        let host = registry.host();
        let setup = Setup::new(&[format!("{}\nuser = \"ci\"", plain(&host))]);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(
            view.state,
            State::LoginRequired {
                challenge: Some(Challenge::Basic {
                    realm: Some("mock".into())
                }),
                refused: false
            }
        );
        let message = view.message().unwrap();
        assert_eq!(message.heading, format!("{host} asks for a login"));
        assert!(
            message.text.contains("401 Unauthorized"),
            "{}",
            message.text
        );
        assert_eq!(message.action, Some(Action::LogIn));
        assert_eq!(
            registry.authorized_log(),
            Vec::<String>::new(),
            "no login was invented"
        );

        // A login lists the namespace; the login goes with the first request.
        setup
            .accounts
            .store
            .store(&host, "ci", &Secret::new("secret"))
            .unwrap();
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(names(view), ["private/games/a"]);
        assert!(registry
            .authorized_log()
            .contains(&"GET /v2/_catalog".to_owned()));

        // A refused login keeps nothing, not even the rows listed before.
        setup
            .accounts
            .store
            .store(&host, "ci", &Secret::new("wrong"))
            .unwrap();
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert!(
            matches!(view.state, State::LoginRequired { refused: true, .. }),
            "{:?}",
            view.state
        );
        assert!(view.repositories.is_empty());
        assert_eq!(
            view.message().unwrap().heading,
            format!("{host} refused the login")
        );
        let request = remotes.refresh(&host);
        drop(registry);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        assert!(
            matches!(view.state, State::Unreachable { stale: false, .. }),
            "nothing was kept to show stale"
        );
    }

    #[test]
    fn listed_tags_are_compared_with_the_store_and_again_when_the_library_changes() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        let setup = Setup::with(&registry);
        let local = |path: &str| format!("{host}/{path}");
        // In library.
        let pulled = machine("pulled", &[]);
        pulled.seed(&registry, "private/games/pulled", "1.0.0");
        pulled.tag(&setup.store, &local("private/games/pulled:1.0.0"));
        // The tag changed on the registry after the pull.
        let moved = machine("moved", &[]);
        moved.seed(&registry, "private/games/moved", "1.0.0");
        machine("moved before", &[]).tag(&setup.store, &local("private/games/moved:1.0.0"));
        // A newer version than the library's.
        let newer = machine("newer 2", &[]);
        newer.seed(&registry, "private/games/newer", "2.0.0");
        machine("newer 1", &[]).tag(&setup.store, &local("private/games/newer:1.0.0"));
        // The same image under another name.
        let copy = machine("copy", &[]);
        copy.seed(&registry, "private/games/copy", "1.0.0");
        copy.tag(&setup.store, "private/games/copy-made-here:1.0.0");

        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let states = |remotes: &Remotes| -> Vec<(String, LocalState)> {
            remotes
                .registry(&host)
                .unwrap()
                .repositories
                .iter()
                .map(|r| {
                    let row = r.representative_row().unwrap();
                    (r.name.clone(), row.local.clone().unwrap())
                })
                .collect()
        };
        assert_eq!(
            states(&remotes),
            [
                (
                    "private/games/copy".into(),
                    LocalState::NotPulled {
                        same_as: Some("private/games/copy-made-here:1.0.0".into())
                    }
                ),
                (
                    "private/games/moved".into(),
                    LocalState::OtherImage {
                        current: machine("moved before", &[]).digest
                    }
                ),
                (
                    "private/games/newer".into(),
                    LocalState::NewerAvailable {
                        have: local("private/games/newer:1.0.0")
                    }
                ),
                ("private/games/pulled".into(), LocalState::InLibrary),
            ]
        );
        let row = remotes
            .registry(&host)
            .unwrap()
            .repository("private/games/newer")
            .unwrap()
            .representative_row()
            .unwrap()
            .clone();
        assert_eq!(row.local_tag, local("private/games/newer:2.0.0"));

        // A pull of the newer version: the states follow the store, and no
        // registry is asked.
        newer.tag(&setup.store, &local("private/games/newer:2.0.0"));
        registry.clear_log();
        let request = remotes.library_changed();
        setup.answer(&mut remotes, request);
        assert!(registry.log().is_empty());
        let now = states(&remotes);
        assert_eq!(
            now[2],
            ("private/games/newer".into(), LocalState::InLibrary)
        );
        assert_eq!(
            now[3],
            ("private/games/pulled".into(), LocalState::InLibrary)
        );
    }

    /// Every file under `root` with its bytes.
    fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_owned()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.insert(path.clone(), Vec::new());
                    pending.push(path);
                } else {
                    files.insert(path.clone(), std::fs::read(&path).unwrap());
                }
            }
        }
        files
    }

    #[test]
    fn browsing_writes_nothing_to_disk() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        let set = media_set("Keen", &["360K"]);
        set.seed(&registry, "games/keen", "1.4");
        let game = machine("pop", &[(TITLE, "Prince of Persia")]);
        game.seed(&registry, "games/pop", "1.0.0");
        game.seed(&registry, "games/pop", "1.1.0");
        let setup = Setup::with(&registry);
        game.tag(&setup.store, &format!("{host}/games/pop:1.0.0"));
        let store_before = files(setup.store.root());
        let config = setup.preferences.parent().unwrap().to_owned();
        let config_before = files(&config);

        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let request = remotes.expand(&host, "games/pop");
        setup.answer(&mut remotes, request);
        let request = remotes.library_changed();
        setup.answer(&mut remotes, request);
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        assert_eq!(remotes.registry(&host).unwrap().state, State::Listed);

        assert_eq!(
            files(setup.store.root()),
            store_before,
            "the store is unchanged"
        );
        assert_eq!(
            files(&config),
            config_before,
            "the preferences are unchanged"
        );
    }

    #[test]
    fn the_library_worker_answers_browsing_on_a_later_frame() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let worker = crate::library_worker::Worker::synthetic(setup.store.root().to_owned());
        let request = remotes.open_node(&host).unwrap();
        worker
            .send
            .send(crate::library_worker::Request::Browse(request))
            .unwrap();
        loop {
            let event = worker
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .expect("worker notification");
            match event {
                crate::library_worker::Event::Browse(event) => {
                    let done = matches!(event, BrowseEvent::Ended { .. });
                    remotes.on_event(event);
                    if done {
                        break;
                    }
                }
                crate::library_worker::Event::Error(error) => panic!("worker: {error}"),
                _ => {}
            }
        }
        let view = remotes.registry(&host).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(names(view), ["games/a"]);
    }

    #[test]
    fn an_entry_that_names_namespaces_reads_only_their_repositories() {
        let registry = MockRegistry::start(open_registry());
        for (name, label) in [
            ("games/a", "a"),
            ("private/b", "b"),
            ("private/c", "c"),
            ("unemu/d", "d"),
        ] {
            machine(label, &[]).seed(&registry, name, "1.0.0");
        }
        let host = registry.host();
        // The live registry test's repositories are never listed, whatever
        // the entry.
        machine("live", &[]).seed(&registry, "unemu/test-1a2b", "1.0.0");
        let whole = Setup::with(&registry);
        let mut remotes = whole.remotes();
        let request = remotes.open_node(&host);
        whole.answer(&mut remotes, request);
        assert_eq!(
            names(remotes.registry(&host).unwrap()),
            ["games/a", "private/b", "private/c", "unemu/d"]
        );
        assert!(!registry.log().iter().any(|l| l.contains("unemu/test-")));
        registry.clear_log();
        let setup = Setup::new(&[format!(
            "{}\nnamespaces = [{{ name = \"private\", private = true }}]",
            plain(&host)
        )]);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        let events = setup.answer(&mut remotes, request);
        let BrowseEvent::Page { repositories, .. } = &events[0] else {
            panic!("{:?}", events[0]);
        };
        assert_eq!(repositories, &["private/b", "private/c"]);
        let view = remotes.registry(&host).unwrap();
        assert_eq!(names(view), ["private/b", "private/c"]);
        assert_eq!(view.expected, 2);
        let log = registry.log();
        assert!(
            !log.iter()
                .any(|l| l.contains("games/a") || l.contains("unemu/d")),
            "{log:?}"
        );
    }

    #[test]
    fn a_library_that_cannot_be_read_is_reported() {
        let registry = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let Some(BrowseRequest::Local { id, .. }) = remotes.library_changed() else {
            panic!("a local-state request");
        };
        remotes.on_event(BrowseEvent::Local {
            id,
            states: Err("The library could not be read: locked".into()),
        });
        let view = remotes.registry(&host).unwrap();
        assert_eq!(
            view.state,
            State::Failed("The library could not be read: locked".into())
        );
        assert_eq!(names(view), ["games/a"], "the rows stay");
        assert!(view.banner().unwrap().text.contains("locked"));
    }

    #[test]
    fn a_tag_whose_read_failed_is_read_again_when_selected() {
        let registry = MockRegistry::start(open_registry());
        let image = machine("a", &[]);
        image.seed(&registry, "games/a", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        registry.fail_requests("GET /v2/games/a/manifests/", 500);
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let failed = |remotes: &Remotes| {
            remotes
                .registry(&host)
                .unwrap()
                .repository("games/a")
                .unwrap()
                .row("1.0.0")
                .unwrap()
                .record
                .is_err()
        };
        assert!(failed(&remotes));
        registry.stop_failing();
        let request = remotes.select(&host, "games/a", "1.0.0");
        assert!(request.is_some(), "a failed read is read again");
        setup.answer(&mut remotes, request);
        assert!(!failed(&remotes));
    }

    #[test]
    fn stop_holds_between_tag_pages_and_between_a_manifest_and_its_records() {
        let registry = MockRegistry::start(open_registry());
        let image = machine("many", &[]);
        for i in 0..=TAG_PAGE {
            image.seed(&registry, "games/many", &format!("1.0.{i}"));
        }
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        let late = stop_while_held(
            &setup,
            &mut remotes,
            &registry,
            &host,
            "GET /v2/games/many/tags/list",
            request,
        );
        assert_eq!(ended(&late), &End::Stopped);
        let log = registry.log();
        assert_eq!(
            log.iter()
                .filter(|l| *l == "GET /v2/games/many/tags/list")
                .count(),
            1,
            "no second tag page after Stop: {log:?}"
        );

        // Between a set's manifest and its config record.
        let set = media_set("Keen", &["360K"]);
        let other = MockRegistry::start(open_registry());
        set.seed(&other, "games/keen", "1.4");
        let setup = Setup::with(&other);
        let mut remotes = setup.remotes();
        let host = other.host();
        let request = remotes.open_node(&host);
        stop_while_held(
            &setup,
            &mut remotes,
            &other,
            &host,
            "GET /v2/games/keen/manifests/",
            request,
        );
        assert_eq!(
            blob_requests(&other),
            Vec::<String>::new(),
            "no config after Stop"
        );

        // Between the config record and the metadata record of a selected tag.
        let request = remotes.refresh(&host);
        setup.answer(&mut remotes, request);
        other.clear_log();
        let request = remotes.select(&host, "games/keen", "1.4");
        let config = format!("GET /v2/games/keen/blobs/{}", Digest::of(&set.config));
        let late = stop_while_held(&setup, &mut remotes, &other, &host, &config, request);
        assert_eq!(ended(&late), &End::Stopped);
        assert_eq!(
            blob_requests(&other),
            [Digest::of(&set.config).to_string()],
            "no metadata after Stop"
        );
    }

    #[test]
    fn stop_all_stops_every_registry() {
        let one = MockRegistry::start(open_registry());
        let two = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&one, "games/a", "1.0.0");
        machine("b", &[]).seed(&two, "games/b", "1.0.0");
        let setup = Setup::new(&[plain(&one.host()), plain(&two.host())]);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&one.host());
        setup.answer(&mut remotes, request);
        let read = remotes.select(&one.host(), "games/a", "1.0.0");
        let list = remotes.open_node(&two.host());
        let flag = |request: &Option<BrowseRequest>| match request {
            Some(BrowseRequest::List { job, .. } | BrowseRequest::Read { job, .. }) => {
                job.cancel.clone()
            }
            _ => panic!("a registry request"),
        };
        let (read_flag, list_flag) = (flag(&read), flag(&list));
        remotes.stop_all();
        assert!(read_flag.load(Ordering::SeqCst) && list_flag.load(Ordering::SeqCst));
        assert_eq!(remotes.registry(&two.host()).unwrap().state, State::Stopped);
        assert_eq!(remotes.registry(&one.host()).unwrap().state, State::Listed);
        // Their answers change nothing now.
        setup.answer(&mut remotes, read);
        setup.answer(&mut remotes, list);
        assert_eq!(remotes.registry(&two.host()).unwrap().repositories.len(), 0);
        let row = remotes
            .registry(&one.host())
            .unwrap()
            .repository("games/a")
            .unwrap();
        assert!(row
            .row("1.0.0")
            .unwrap()
            .record
            .as_ref()
            .unwrap()
            .details
            .is_none());
    }

    #[test]
    fn browsing_is_answered_while_a_machine_runs() {
        use crate::library_worker::{Event, Finish, Request, Worker, WorkspaceKind};
        let registry = MockRegistry::start(open_registry());
        machine("a", &[]).seed(&registry, "games/a", "1.0.0");
        let setup = Setup::with(&registry);
        let fixture = crate::library_test_support::Fixture::new();
        let worker = Worker::synthetic(fixture.store.root().to_owned());
        let wait = |want: &dyn Fn(&Event) -> bool| loop {
            let event = worker
                .receive
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .expect("worker notification");
            if let Event::Error(error) = &event {
                panic!("worker: {error}");
            }
            if want(&event) {
                return event;
            }
        };
        worker
            .send
            .send(Request::Launch(
                fixture.row.clone(),
                WorkspaceKind::Regular,
                None,
            ))
            .unwrap();
        let Event::Launched(transfer, ..) = wait(&|e| matches!(e, Event::Launched(..))) else {
            unreachable!()
        };
        let session = transfer.restore().unwrap();
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&registry.host()).unwrap();
        worker.send.send(Request::Browse(request)).unwrap();
        loop {
            let Event::Browse(event) = wait(&|e| matches!(e, Event::Browse(_))) else {
                unreachable!()
            };
            let done = matches!(event, BrowseEvent::Ended { .. });
            remotes.on_event(event);
            if done {
                break;
            }
        }
        let view = remotes.registry(&registry.host()).unwrap();
        assert_eq!(view.state, State::Listed);
        assert_eq!(names(view), ["games/a"]);
        drop(session);
        worker.send.send(Request::Finish(Finish::Retain)).unwrap();
        wait(&|e| matches!(e, Event::Recoveries(_)));
    }

    /// A repository's row stands for the version `latest` names when it is
    /// one of the newest and runs here, else for the newest version this
    /// release runs, and names the newer versions that need a newer
    /// release. A repository none of whose versions runs here stands for its
    /// newest one, whose need dims the row.
    #[test]
    fn the_row_stands_for_latests_version_or_the_newest_this_release_runs() {
        let registry = MockRegistry::start(open_registry());
        let named = machine("named by latest", &[]);
        named.seed(&registry, "games/latest", "1.0.1");
        named.seed(&registry, "games/latest", "latest");
        machine("newer, not named", &[]).seed(&registry, "games/latest", "1.1.0");
        let future = machine("future", &[(MIN_APP_VERSION, "99.0.0")]);
        future.seed(&registry, "games/future", "1.3.0");
        future.seed(&registry, "games/future", "latest");
        let runs = machine("runs", &[]);
        runs.seed(&registry, "games/future", "1.2.0");
        machine("none 2", &[(MIN_APP_VERSION, "99.0.0")]).seed(&registry, "games/none", "2.0.0");
        machine("none 1", &[(MIN_APP_VERSION, "98.0.0")]).seed(&registry, "games/none", "1.9.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();

        let row = view.repository("games/latest").unwrap();
        assert_eq!(
            row.representative.as_deref(),
            Some("1.0.1"),
            "latest's version"
        );
        assert!(row.newer_needs.is_empty());
        assert_eq!(
            row.representative_row()
                .unwrap()
                .record
                .as_ref()
                .unwrap()
                .digest,
            named.digest
        );
        let log = registry.log();
        for tag in ["latest", "1.1.0", "1.0.1"] {
            assert!(
                log.contains(&format!("HEAD /v2/games/latest/manifests/{tag}")),
                "{tag} is compared by digest: {log:?}"
            );
        }

        let row = view.repository("games/future").unwrap();
        assert_eq!(
            row.representative.as_deref(),
            Some("1.2.0"),
            "latest names a version this release cannot run"
        );
        assert_eq!(
            row.newer_needs,
            [(
                "1.3.0".to_owned(),
                "Needs unemupc 99.0.0 or newer".to_owned()
            )]
        );
        let record = row.representative_row().unwrap().record.as_ref().unwrap();
        assert_eq!(record.digest, runs.digest);
        assert_eq!(record.needs_newer(), None);

        let row = view.repository("games/none").unwrap();
        assert_eq!(row.representative.as_deref(), Some("2.0.0"), "the newest");
        assert!(row.newer_needs.is_empty());
        assert_eq!(row.read.len(), 2, "every version was looked at");
        assert_eq!(
            row.representative_row()
                .unwrap()
                .record
                .as_ref()
                .unwrap()
                .needs_newer()
                .as_deref(),
            Some("Needs unemupc 99.0.0 or newer")
        );
        assert_eq!(
            candidates(&["1.0.0".into(), "latest".into(), "2.0.0-rc.1".into()]),
            ["1.0.0"],
            "pre-releases only without a release; aliases never"
        );
    }

    /// What a pull would download leaves out the blobs the store holds,
    /// before and after the library changes; an image in the library
    /// downloads nothing.
    #[test]
    fn the_download_leaves_out_blobs_the_store_holds() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        let image = machine("partly here", &[]);
        image.seed(&registry, "games/partly", "1.0.0");
        let setup = Setup::with(&registry);
        // The config is in the store already, as another image shares it.
        setup.store.import(image.config.as_slice(), None).unwrap();
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let download = |remotes: &Remotes| {
            remotes
                .registry(&host)
                .unwrap()
                .repository("games/partly")
                .unwrap()
                .representative_row()
                .unwrap()
                .download()
        };
        let layers: u64 = image.layers.iter().map(|l| l.len() as u64).sum();
        assert_eq!(
            download(&remotes),
            Some(Download {
                bytes: image.bytes.len() as u64 + layers,
                present: 1,
                blobs: 3,
            })
        );
        // Every blob arrives, without the manifest: only the manifest is left.
        for layer in &image.layers {
            setup.store.import(layer.as_slice(), None).unwrap();
        }
        let request = remotes.library_changed();
        setup.answer(&mut remotes, request);
        assert_eq!(
            download(&remotes),
            Some(Download {
                bytes: image.bytes.len() as u64,
                present: 3,
                blobs: 3,
            })
        );
        image.tag(&setup.store, &local_tag_of(&host, "games/partly", "1.0.0"));
        let request = remotes.library_changed();
        setup.answer(&mut remotes, request);
        assert_eq!(download(&remotes), None, "in the library: nothing to fetch");
    }

    /// Only IBM PC compatible artifacts of the PC provider's three types run
    /// here, whichever of the family's ids they name: a machine image naming
    /// `ibm-pc` or the retired `ibm-pc-at` can be pulled, while one for
    /// another family, or another tool's artifact, is refused with the
    /// reason, as Get templates left out what is not a PC machine image.
    #[test]
    fn only_ibm_pc_artifacts_run_here_and_others_say_why() {
        let registry = MockRegistry::start(open_registry());
        let host = registry.host();
        let retired = image_store::platform::ALIASES[0].0;
        machine_of(pc_image::PLATFORM, "pc", &[]).seed(&registry, "unemu/pc", "1.0.0");
        machine_of(retired, "pc at", &[]).seed(&registry, "unemu/pc-at", "1.0.0");
        machine_of("c64", "c64", &[]).seed(&registry, "unemu/c64", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let view = remotes.registry(&host).unwrap();
        let reason = |repository: &str| {
            let row = view
                .repository(repository)
                .unwrap()
                .representative_row()
                .unwrap();
            let pick = library_model::remote_pull::Pick {
                host: &host,
                repository,
                tag: "1.0.0",
                row: Some(row),
                blocked: None,
                facts: &PcDesktop::facts(),
            };
            (
                row.record
                    .as_ref()
                    .unwrap()
                    .not_runnable(&PcDesktop::facts()),
                library_model::remote_pull::refusal(&pick),
            )
        };
        assert_eq!(reason("unemu/pc"), (None, None));
        assert_eq!(
            reason("unemu/pc-at"),
            (None, None),
            "the retired id is the PC"
        );
        assert_eq!(
            reason("unemu/c64"),
            (
                Some("A c64 machine image: this release runs IBM PC compatibles".into()),
                Some("A c64 machine image: this release runs IBM PC compatibles.".into())
            )
        );
        let mut other = view
            .repository("unemu/pc")
            .unwrap()
            .representative_row()
            .unwrap()
            .record
            .clone()
            .unwrap();
        other.artifact_type = "application/vnd.example.save-game.v1".into();
        assert_eq!(
            other.not_runnable(&PcDesktop::facts()).as_deref(),
            Some("Another tool's artifact (application/vnd.example.save-game.v1): this release pulls machine images, installation media sets and firmware packages")
        );
        other.artifact_type = mt::MACHINE.into();
        other.platform = None;
        assert_eq!(
            other.not_runnable(&PcDesktop::facts()).as_deref(),
            Some("A machine image that names no machine family: this release runs IBM PC compatibles")
        );
    }

    /// The listing reads at most the 16 newest versions of a repository
    /// looking for one that runs here; past them the row stands for the
    /// newest, dimmed.
    #[test]
    fn the_listing_reads_at_most_sixteen_versions_per_repository() {
        let registry = MockRegistry::start(open_registry());
        let future = machine("future", &[(MIN_APP_VERSION, "99.0.0")]);
        for patch in 0..=LATEST_LOOKUPS {
            future.seed(&registry, "games/many", &format!("2.0.{patch}"));
        }
        machine("old", &[]).seed(&registry, "games/many", "1.0.0");
        let setup = Setup::with(&registry);
        let mut remotes = setup.remotes();
        let host = registry.host();
        let request = remotes.open_node(&host);
        setup.answer(&mut remotes, request);
        let row = remotes
            .registry(&host)
            .unwrap()
            .repository("games/many")
            .unwrap()
            .clone();
        assert_eq!(row.representative.as_deref(), Some("2.0.16"));
        assert_eq!(row.read.len(), LATEST_LOOKUPS);
        assert_eq!(
            registry
                .log()
                .iter()
                .filter(|line| line.starts_with("GET /v2/games/many/manifests/"))
                .count(),
            LATEST_LOOKUPS
        );
    }
}
