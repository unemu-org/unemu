// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Remotes group's test rig: a library frame whose worker records every
//! request instead of contacting a registry, the preferences it lists and
//! the sample listing. The Remotes tests drive it, and so does the
//! application's test of the refresh key.

use super::{Group, Node};
use crate::{
    library_view::{LibraryView, Request},
    library_worker::{Event, Worker},
    registry_transfer::Accounts,
    remote_browse::{
        local_tag_of, representative, BrowseRequest, Job, LocalState, RegistryView, Remotes,
        RepositoryRow, State, TagRecord, TagRow,
    },
};
use image_registry::{
    credential_store::SessionStore, credentials::EnvCredentials, reference::OFFICIAL_NAMESPACE,
    DEFAULT_REGISTRY,
};
use image_store::{media_type as mt, Digest};
use std::sync::{mpsc, Arc};

pub const LAN: &str = "registry.lan.example";
pub const NAS: &str = "nas.example:5000";

/// Preferences that list `registry.lan.example` with a private namespace
/// and no account, and `nas.example:5000`, a whole private registry with
/// an account; the family default registry is listed without an entry.
pub fn preferences() -> (machine_file::test_support::TempDir, Accounts) {
    let temp = machine_file::test_support::TempDir::new("remotes-view");
    let path = temp.path().join("preferences.toml");
    std::fs::write(
        &path,
        format!(
            "[[registries]]\nhost = \"{LAN}\"\nnamespaces = [{{ name = \"private\", private = true }}]\n\
             [[registries]]\nhost = \"{NAS}\"\nprivate = true\nuser = \"maintainer\"\n"
        ),
    )
    .unwrap();
    let accounts = Accounts {
        path: Some(path),
        store: Arc::new(SessionStore::default()),
        env: EnvCredentials::default(),
        default_override: None,
    };
    (temp, accounts)
}

/// A library view whose worker records the requests it gets.
pub struct Rig {
    pub view: LibraryView,
    pub requests: mpsc::Receiver<Request>,
    /// Dropping it is the worker going away.
    pub events: Option<mpsc::SyncSender<Event>>,
    _preferences: machine_file::test_support::TempDir,
}

impl Default for Rig {
    fn default() -> Self {
        Self::new()
    }
}

impl Rig {
    pub fn new() -> Self {
        let (temp, accounts) = preferences();
        let mut view = LibraryView::new(None);
        let (worker, requests, events) = Worker::recording();
        view.worker = Some(worker);
        view.attach_remotes(accounts);
        Self {
            view,
            requests,
            events: Some(events),
            _preferences: temp,
        }
    }

    /// Every request sent since the last call.
    pub fn sent(&self) -> Vec<Request> {
        self.requests.try_iter().collect()
    }

    /// The browsing requests sent since the last call.
    pub fn browsing(&self) -> Vec<BrowseRequest> {
        self.sent()
            .into_iter()
            .filter_map(|request| match request {
                Request::Browse(browse) => Some(browse),
                _ => None,
            })
            .collect()
    }

    /// The model, for a change the test makes to it directly.
    pub fn model(&mut self) -> &mut Remotes {
        let model = self.view.remote.model.as_mut().expect("remotes attached");
        model.touch();
        model
    }

    pub fn registry(&mut self, host: &str) -> &mut RegistryView {
        self.model()
            .registries
            .iter_mut()
            .find(|v| v.host == host)
            .unwrap_or_else(|| panic!("no registry {host}"))
    }

    /// `registry.unemu.dev` with the sample listing, listed.
    pub fn listed(&mut self) {
        let registry = self.registry(DEFAULT_REGISTRY);
        registry.repositories = sample(DEFAULT_REGISTRY);
        registry.state = State::Listed;
    }

    pub fn show(&mut self, node: Node) {
        self.view.remote.node = Some(node);
    }
}

pub fn job_of(request: &BrowseRequest) -> &Job {
    match request {
        BrowseRequest::List { job, .. } | BrowseRequest::Read { job, .. } => job,
        BrowseRequest::Local { .. } => panic!("a local-state request has no job"),
    }
}

pub fn record(artifact_type: &str, title: &str) -> TagRecord {
    TagRecord {
        digest: Digest::of(title.as_bytes()),
        artifact_type: artifact_type.into(),
        title: Some(title.into()),
        version: None,
        formats: Vec::new(),
        min_app_version: None,
        platform: Some(pc_image::PLATFORM.into()),
        size: 3 * 1024 * 1024 + 512 * 1024,
        user_supplied: false,
        annotations: Default::default(),
        details: None,
        manifest_size: 1024,
        descriptors: Vec::new(),
        media: Vec::new(),
    }
}

pub fn read(
    host: &str,
    repository: &str,
    tag: &str,
    record: TagRecord,
    local: LocalState,
) -> TagRow {
    TagRow {
        tag: tag.into(),
        local_tag: local_tag_of(host, repository, tag),
        record: Ok(record),
        local: Some(local),
        stored: Default::default(),
        newest_revision: None,
    }
}

pub fn repository(name: &str, tags: &[&str], read: Vec<TagRow>) -> RepositoryRow {
    let tags: Vec<String> = tags.iter().map(|t| (*t).to_owned()).collect();
    RepositoryRow {
        name: name.into(),
        representative: representative(&tags),
        tags,
        read,
        error: None,
        newer_needs: Vec::new(),
    }
}

/// The sample listing: three machine images (one with three tags), two
/// installation media sets and a firmware package this release cannot
/// run, in every local state.
pub fn sample(host: &str) -> Vec<RepositoryRow> {
    let mut future = record(mt::FIRMWARE, "Future BIOS");
    future.min_app_version = Some("99.0.0".into());
    let mut freedos = record(mt::MEDIA_SET, "FreeDOS");
    freedos.formats = vec!["1.44M".into()];
    freedos.media = vec![
        ("disk-1".into(), "1.44M".into()),
        ("disk-2".into(), "1.44M".into()),
    ];
    let mut keen = record(mt::MEDIA_SET, "Commander Keen 4");
    keen.user_supplied = true;
    vec![
        repository(
            "unemu/open-at",
            &["1.0.0", "latest", "1.2.0"],
            vec![read(
                host,
                "unemu/open-at",
                "1.2.0",
                record(mt::MACHINE, "Open AT 286"),
                LocalState::InLibrary,
            )],
        ),
        repository(
            "unemu/open-at-games",
            &["1.2.0"],
            vec![read(
                host,
                "unemu/open-at-games",
                "1.2.0",
                record(mt::MACHINE, "Open AT 286 for games"),
                LocalState::NewerAvailable {
                    have: "open-at-games:1.0.0".into(),
                },
            )],
        ),
        repository(
            "unemu/clash",
            &["1.0.0"],
            vec![read(
                host,
                "unemu/clash",
                "1.0.0",
                record(mt::MACHINE, "Clash"),
                LocalState::OtherImage {
                    current: Digest::of(b"another"),
                },
            )],
        ),
        repository(
            "unemu/media/freedos",
            &["1.4-720k", "1.4"],
            vec![read(
                host,
                "unemu/media/freedos",
                "1.4",
                freedos,
                LocalState::NotPulled { same_as: None },
            )],
        ),
        repository(
            "unemu/media/keen4",
            &["1.4"],
            vec![read(
                host,
                "unemu/media/keen4",
                "1.4",
                keen,
                LocalState::NotPulled {
                    same_as: Some("keen4:1.4".into()),
                },
            )],
        ),
        repository(
            "unemu/firmware/future-bios",
            &["1"],
            vec![read(
                host,
                "unemu/firmware/future-bios",
                "1",
                future,
                LocalState::NotPulled { same_as: None },
            )],
        ),
    ]
}

pub fn unemu(group: Option<Group>) -> Node {
    Node {
        host: DEFAULT_REGISTRY.into(),
        namespace: Some(Some(OFFICIAL_NAMESPACE.into())),
        group,
    }
}
