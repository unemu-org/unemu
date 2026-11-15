// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless model of Preferences › Registries; `backlog/docs/design/machine-images.md`
//! §5.1 and the approved registry-accounts mock-up.
//!
//! Every action applies at once, through the same registry-crate functions the
//! command line uses: preferences change under the shared lock, keeping keys
//! other writers own, and the list is read again afterwards, so an `image
//! login` made meanwhile shows up. Anything that may reach the credential
//! store or the network (sign-in states, logging in and out, removing,
//! testing a connection) runs as a background job that the frame loop polls,
//! because a Keychain call can wait on a system prompt. Each job has an id and
//! a cancel flag: a closed dialog cancels its login, and a late result never
//! changes a newer dialog. The password typed into Log in moves into the login
//! job and leaves the dialog at once; it is never shown, logged, or written to
//! preferences. Secrets are not wiped from memory after use.

use image_registry::{
    accounts::{self, ConnectionReport, LoginCheck, LoginOutcome},
    client::ApiAccess,
    config::{self, EntryEdit, EntrySettings, RegistryConfig, RegistryEntry},
    credential_store::{CredentialStore, Secret, StoreKind, MAX_KEYCHAIN_SECRET},
    credentials::{EnvCredentials, HostCredentials},
    reference::OFFICIAL_NAMESPACE,
    Error, DEFAULT_REGISTRY,
};
use std::{
    collections::BTreeMap,
    fmt,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
};

/// What the emulator says about keeping secrets, for scripts and CI too.
pub(crate) const ENV_HINT: &str = "For scripts and CI, set the UNEMU_REGISTRY_* environment variables instead, for example UNEMU_REGISTRY_USER and UNEMU_REGISTRY_PASSWORD; they are read for that run only and never saved.";

const LOST: &str = "the background job stopped unexpectedly";

/// One configured registry as listed on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub entry: RegistryEntry,
    /// False for the family default registry while preferences do not list it.
    pub listed: bool,
    pub sign_in: SignIn,
    pub test: TestState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SignIn {
    /// The credential store is being asked in the background.
    Checking,
    SignedIn,
    SignedOut,
    /// The credential store could not be asked.
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TestState {
    NotRun,
    Running,
    Done(Vec<Check>),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Good,
    Bad,
    Note,
}

/// One line of a connection test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Check {
    pub mark: Mark,
    pub text: String,
}

/// The Add registry or Edit registry dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Editor {
    /// The host being edited; `None` adds a registry.
    pub original: Option<String>,
    pub host: String,
    /// The Namespaces field: comma-separated names, empty for the whole
    /// registry ([`config::parse_namespaces`]).
    pub namespaces: String,
    /// The whole registry, or each listed namespace without its own mark.
    pub private: bool,
    pub plain_http: bool,
    /// The entry's settings as the dialog opened, so a save can tell
    /// whether the entry changed meanwhile.
    started: Option<EntrySettings>,
    pub error: Option<String>,
}

/// The Log in dialog.
pub(crate) struct LoginDialog {
    pub host: String,
    pub user: String,
    pub password: String,
    pub error: Option<String>,
    pub busy: bool,
    /// The login job this dialog waits for.
    job: Option<u64>,
}

impl fmt::Debug for LoginDialog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginDialog")
            .field("host", &self.host)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("error", &self.error)
            .field("busy", &self.busy)
            .field("job", &self.job)
            .finish()
    }
}

/// Un-marking a private registry or namespace waits for this confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfirmPublic {
    pub host: String,
    /// What becomes public: the host, or `host/name/` for each namespace.
    pub exposed: String,
    pub settings: EntrySettings,
}

/// Why a login ended without a stored secret.
#[derive(Debug)]
enum LoginFailure {
    Refused,
    TokenService,
    Cancelled,
    Other(String),
}

impl From<Error> for LoginFailure {
    fn from(error: Error) -> Self {
        match error {
            Error::Unauthorized { .. } => Self::Refused,
            Error::UnsupportedAuth(_) => Self::TokenService,
            Error::Cancelled => Self::Cancelled,
            Error::Transport(_) | Error::Status { .. } | Error::Protocol(_) => {
                Self::Other(format!("{error}. Nothing was stored."))
            }
            other => Self::Other(format!("Login failed: {other}")),
        }
    }
}

impl LoginFailure {
    fn message(&self) -> String {
        match self {
            Self::Refused => {
                "The registry refused the user name or password (401). Nothing was stored.".into()
            }
            Self::TokenService => "The registry asks for a token service; only Basic authentication is implemented. Nothing was stored.".into(),
            Self::Cancelled => "Cancelled. Nothing was stored.".into(),
            Self::Other(message) => message.clone(),
        }
    }
}

/// What a background job is doing, so a job that dies can still be ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Work {
    SignIn { generation: u64, hosts: Vec<String> },
    Login { host: String, user: String },
    Test { host: String },
    LogOut { host: String },
    Remove { host: String },
}

enum Outcome {
    SignIn {
        generation: u64,
        states: Vec<(String, SignIn)>,
    },
    Login {
        host: String,
        user: String,
        result: Result<LoginOutcome, LoginFailure>,
    },
    Test {
        host: String,
        result: Result<ConnectionReport, String>,
    },
    LogOut {
        host: String,
        result: Result<Option<String>, String>,
    },
    Remove {
        host: String,
        result: Result<(), String>,
    },
}

impl Work {
    fn account(&self) -> bool {
        matches!(
            self,
            Self::Login { .. } | Self::LogOut { .. } | Self::Remove { .. }
        )
    }

    /// The outcome of a job whose thread ended without sending one.
    fn lost(self) -> Outcome {
        match self {
            Self::SignIn { generation, hosts } => Outcome::SignIn {
                generation,
                states: hosts
                    .into_iter()
                    .map(|host| (host, SignIn::Unknown(LOST.into())))
                    .collect(),
            },
            Self::Login { host, user } => Outcome::Login {
                host,
                user,
                result: Err(LoginFailure::Other(format!("{LOST}. Nothing was stored."))),
            },
            Self::Test { host } => Outcome::Test {
                host,
                result: Err(LOST.into()),
            },
            Self::LogOut { host } => Outcome::LogOut {
                host,
                result: Err(LOST.into()),
            },
            Self::Remove { host } => Outcome::Remove {
                host,
                result: Err(LOST.into()),
            },
        }
    }
}

struct Job {
    id: u64,
    work: Work,
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Outcome>,
}

pub(crate) struct RegistriesPage {
    path: Option<PathBuf>,
    store: Arc<dyn CredentialStore>,
    env: EnvCredentials,
    pub rows: Vec<Row>,
    pub default_host: String,
    pub selected: usize,
    pub editor: Option<Editor>,
    pub login: Option<LoginDialog>,
    pub confirm: Option<ConfirmPublic>,
    /// The last action's result, for the line under the list.
    pub status: Option<String>,
    pub error: Option<String>,
    jobs: Vec<Job>,
    next_job: u64,
    /// Sign-in states from an older reload are ignored.
    generation: u64,
}

impl Drop for RegistriesPage {
    /// Closing Preferences cancels a login in flight.
    fn drop(&mut self) {
        for job in &self.jobs {
            job.cancel.store(true, Ordering::SeqCst);
        }
    }
}

impl RegistriesPage {
    /// `env` is the environment's credential pair, which a connection test
    /// uses like push and pull do.
    pub fn new(
        path: Option<PathBuf>,
        store: Arc<dyn CredentialStore>,
        env: EnvCredentials,
    ) -> Self {
        let mut page = Self {
            path,
            store,
            env,
            rows: Vec::new(),
            default_host: DEFAULT_REGISTRY.into(),
            selected: 0,
            editor: None,
            login: None,
            confirm: None,
            status: None,
            error: None,
            jobs: Vec::new(),
            next_job: 0,
            generation: 0,
        };
        page.reload();
        page
    }

    pub fn store_kind(&self) -> StoreKind {
        self.store.kind()
    }

    fn spawn(
        &mut self,
        work: Work,
        run: impl FnOnce(&AtomicBool) -> Outcome + Send + 'static,
    ) -> u64 {
        let id = self.next_job;
        self.next_job += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let flag = cancel.clone();
        std::thread::spawn(move || {
            let _ = sender.send(run(&flag));
        });
        self.jobs.push(Job {
            id,
            work,
            cancel,
            receiver,
        });
        id
    }

    fn cancel_job(&self, id: u64) {
        if let Some(job) = self.jobs.iter().find(|job| job.id == id) {
            job.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Read the registries list afresh; the store is asked in the background
    /// which of them are signed in.
    pub fn reload(&mut self) {
        let selected = self.selected_row().map(|row| row.entry.host.clone());
        let config = match &self.path {
            Some(path) => {
                config::read_preferences(path).and_then(|t| RegistryConfig::from_table(&t))
            }
            None => Ok(RegistryConfig::default()),
        };
        let config = match config {
            Ok(config) => config,
            Err(error) => {
                self.error = Some(format!("Preferences: {error}"));
                self.rows.clear();
                return;
            }
        };
        self.default_host = config
            .default_host(None, None)
            .unwrap_or_else(|_| DEFAULT_REGISTRY.into());
        let mut entries: Vec<_> = config.registries.into_iter().map(|e| (e, true)).collect();
        if !entries.iter().any(|(e, _)| e.host == DEFAULT_REGISTRY) {
            entries.insert(0, (RegistryEntry::builtin(DEFAULT_REGISTRY), false));
        }
        let mut tests: BTreeMap<_, _> = self
            .rows
            .drain(..)
            .map(|row| (row.entry.host, row.test))
            .collect();
        self.rows = entries
            .into_iter()
            .map(|(entry, listed)| Row {
                sign_in: if entry.user.is_some() {
                    SignIn::Checking
                } else {
                    SignIn::SignedOut
                },
                test: tests.remove(&entry.host).unwrap_or(TestState::NotRun),
                entry,
                listed,
            })
            .collect();
        self.selected = selected
            .and_then(|host| self.rows.iter().position(|row| row.entry.host == host))
            .unwrap_or(0);
        self.generation += 1;
        let accounts: Vec<(String, String)> = self
            .rows
            .iter()
            .filter_map(|row| Some((row.entry.host.clone(), row.entry.user.clone()?)))
            .collect();
        if accounts.is_empty() {
            return;
        }
        let generation = self.generation;
        let store = self.store.clone();
        let hosts = accounts.iter().map(|(host, _)| host.clone()).collect();
        self.spawn(Work::SignIn { generation, hosts }, move |_| {
            Outcome::SignIn {
                generation,
                states: accounts
                    .into_iter()
                    .map(|(host, user)| {
                        let state = match store.contains(&host, &user) {
                            Ok(true) => SignIn::SignedIn,
                            Ok(false) => SignIn::SignedOut,
                            Err(error) => SignIn::Unknown(error.to_string()),
                        };
                        (host, state)
                    })
                    .collect(),
            }
        });
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn select(&mut self, index: usize) {
        if index < self.rows.len() {
            self.selected = index;
        }
    }

    pub fn signed_in_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.sign_in == SignIn::SignedIn)
            .count()
    }

    fn writable(&self) -> bool {
        self.path.is_some()
    }

    /// A login, logout or removal is running. A cancelled login does not
    /// count: it can keep nothing, so a new login may start beside it.
    pub fn account_busy(&self) -> bool {
        self.jobs
            .iter()
            .any(|job| job.work.account() && !job.cancel.load(Ordering::SeqCst))
    }

    /// Why Remove is unavailable for the selected registry, if it is.
    pub fn remove_blocked(&self) -> Option<String> {
        let row = self.selected_row()?;
        if row.entry.host == self.default_host {
            Some(format!(
                "Remove: {} is the default registry. Choose another default first.",
                row.entry.host
            ))
        } else if !row.listed {
            Some(format!(
                "Remove: {} is built in and has no entry to remove.",
                row.entry.host
            ))
        } else {
            None
        }
    }

    pub fn can_remove(&self) -> bool {
        self.writable()
            && !self.account_busy()
            && self.selected_row().is_some()
            && self.remove_blocked().is_none()
    }

    pub fn can_log_in(&self) -> bool {
        self.writable()
            && self.login.is_none()
            && !self.account_busy()
            && self
                .selected_row()
                .is_some_and(|row| row.sign_in != SignIn::SignedIn)
    }

    pub fn can_log_out(&self) -> bool {
        !self.account_busy()
            && self
                .selected_row()
                .is_some_and(|row| row.sign_in == SignIn::SignedIn)
    }

    pub fn can_set_default(&self) -> bool {
        self.writable()
            && self
                .selected_row()
                .is_some_and(|row| row.entry.host != self.default_host)
    }

    pub fn can_test(&self) -> bool {
        self.selected_row()
            .is_some_and(|row| row.test != TestState::Running)
    }

    pub fn open_add(&mut self) {
        self.reload();
        self.editor = Some(Editor {
            original: None,
            host: String::new(),
            namespaces: String::new(),
            private: true,
            plain_http: false,
            started: None,
            error: None,
        });
    }

    /// Add registry… prefilled with `host`, for a registry that has no entry
    /// yet. No row stays selected, so nothing acts on another registry until
    /// the new one is added or a row is chosen.
    pub fn open_add_for(&mut self, host: &str) {
        self.open_add();
        if let Some(editor) = &mut self.editor {
            editor.host = host.into();
        }
        self.selected = self.rows.len();
    }

    pub fn open_edit(&mut self) {
        self.reload();
        if let Some(row) = self.selected_row() {
            let entry = &row.entry;
            // With namespaces listed, the box shows whether any of them is
            // private, and the field marks the ones that differ.
            let private = if entry.namespaces.is_empty() {
                entry.private
            } else {
                entry.namespaces.iter().any(|n| n.private)
            };
            self.editor = Some(Editor {
                original: Some(entry.host.clone()),
                host: entry.host.clone(),
                namespaces: config::namespaces_field(&entry.namespaces, private),
                private,
                plain_http: entry.plain_http,
                started: Some(entry.settings()),
                error: None,
            });
        }
    }

    /// Validate the editor and apply it; making a private registry or
    /// namespace public first asks for confirmation.
    pub fn submit_editor(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let (private, plain_http) = (editor.private, editor.plain_http);
        let started = editor.started.clone();
        let host = match &editor.original {
            Some(host) => Ok(host.clone()),
            None => config::validate_registry_host(&editor.host)
                .map_err(|_| {
                    "Enter a registry host with a dot or a port, or localhost — without a path."
                        .to_owned()
                })
                .and_then(|host| {
                    if self.rows.iter().any(|r| r.listed && r.entry.host == host) {
                        Err(format!("{host} is already in the list."))
                    } else {
                        Ok(host)
                    }
                }),
        }
        .and_then(|host| {
            if plain_http && !config::is_loopback(&host) {
                Err(
                    "Plain HTTP is allowed only for a test registry on this computer (loopback)."
                        .into(),
                )
            } else {
                Ok(host)
            }
        })
        .and_then(|host| {
            let namespaces = config::parse_namespaces(&editor.namespaces, private)
                .map_err(|error| format!("Namespaces: {}.", plain(&error)))?;
            Ok((host, namespaces))
        });
        let (host, namespaces) = match host {
            Ok(parsed) => parsed,
            Err(error) => {
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(error);
                }
                return;
            }
        };
        let settings = EntrySettings {
            private: private && namespaces.is_empty(),
            namespaces,
            plain_http,
        };
        if started.as_ref() == Some(&settings) {
            self.editor = None;
            self.status = Some(format!("No changes to {host}."));
            return;
        }
        if let Some(exposed) = started
            .as_ref()
            .and_then(|started| config::made_public(started, &settings))
        {
            self.confirm = Some(ConfirmPublic {
                exposed: exposed.describe(&host),
                host,
                settings,
            });
            return;
        }
        self.apply_edit(EntryEdit {
            host,
            started,
            settings,
            confirmed_public: false,
        });
    }

    /// Answer the un-mark confirmation; only an explicit yes makes it public.
    pub fn confirm_public(&mut self, accept: bool) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        let Some(editor) = &self.editor else {
            return;
        };
        if accept {
            let edit = EntryEdit {
                host: confirm.host,
                started: editor.started.clone(),
                settings: confirm.settings,
                confirmed_public: true,
            };
            self.apply_edit(edit);
        }
    }

    fn apply_edit(&mut self, edit: EntryEdit) {
        let adding = edit.started.is_none();
        let host = edit.host.clone();
        match self.write(|table| config::apply_entry_edit(table, &edit)) {
            Ok(()) => {
                self.editor = None;
                if let Some(index) = self.rows.iter().position(|r| r.entry.host == host) {
                    self.selected = index;
                }
                self.status = Some(if adding {
                    format!("Added {host}.")
                } else {
                    format!("Saved {host}.")
                });
            }
            Err(error) => match &mut self.editor {
                Some(editor) => editor.error = Some(error),
                None => self.error = Some(error),
            },
        }
    }

    fn write(
        &mut self,
        change: impl FnOnce(&mut toml::Table) -> image_registry::Result<()>,
    ) -> Result<(), String> {
        let path = self
            .path
            .clone()
            .ok_or("The preferences file location is unavailable.")?;
        let result = config::update_preferences(&path, change)
            .map(|_| ())
            .map_err(|e| e.to_string());
        self.reload();
        if result.is_ok() {
            self.error = None;
        }
        result
    }

    /// Remove the selected registry and its stored secret in the background.
    pub fn remove_selected(&mut self) {
        if !self.can_remove() {
            return;
        }
        let (Some(path), Some(row)) = (self.path.clone(), self.selected_row().cloned()) else {
            return;
        };
        let host = row.entry.host;
        let store = self.store.clone();
        let target = host.clone();
        self.spawn(Work::Remove { host: host.clone() }, move |_| {
            Outcome::Remove {
                result: accounts::remove(&path, &target, &*store).map_err(|e| e.to_string()),
                host: target,
            }
        });
        self.status = Some(format!("Removing {host}…"));
    }

    pub fn set_default_selected(&mut self) {
        let Some(host) = self.selected_row().map(|row| row.entry.host.clone()) else {
            return;
        };
        match self.write(|table| config::set_default_registry(table, &host)) {
            Ok(()) => self.status = Some(format!("Short names now resolve to {host}.")),
            Err(error) => self.error = Some(error),
        }
    }

    pub fn open_login(&mut self) {
        if !self.can_log_in() {
            return;
        }
        self.reload();
        if let Some(row) = self.selected_row() {
            self.login = Some(LoginDialog {
                host: row.entry.host.clone(),
                user: row.entry.user.clone().unwrap_or_default(),
                password: String::new(),
                error: None,
                busy: false,
                job: None,
            });
        }
    }

    /// The Log in dialog's note on where and how the login goes.
    pub fn login_hint(&self) -> String {
        let host = self.login.as_ref().map(|dialog| dialog.host.as_str());
        let plain_http = self
            .rows
            .iter()
            .any(|row| Some(row.entry.host.as_str()) == host && row.entry.plain_http);
        let mut hint = String::from(
            "Never stored in preferences.toml, a plain-text file or any project file. ",
        );
        hint.push_str(if plain_http {
            "The login is checked with Basic authentication over plain HTTP, which is allowed only for a test registry on this computer."
        } else {
            "The login is checked with Basic authentication over TLS before it is kept."
        });
        match self.store.kind() {
            StoreKind::Keychain => hint.push_str(&format!(
                " The Keychain takes passwords and tokens of printable ASCII characters, at most {MAX_KEYCHAIN_SECRET} of them."
            )),
            StoreKind::Session => {
                hint.push(' ');
                hint.push_str(ENV_HINT);
            }
        }
        hint
    }

    /// Start the login job; the password leaves the dialog now.
    pub fn submit_login(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let Some(dialog) = &mut self.login else {
            return;
        };
        if dialog.busy {
            return;
        }
        let user = dialog.user.trim().to_owned();
        if user.is_empty() {
            dialog.error = Some("Enter a user name.".into());
            return;
        }
        if dialog.password.is_empty() {
            dialog.error = Some("Enter a password or an access token.".into());
            return;
        }
        let secret = Secret::new(std::mem::take(&mut dialog.password));
        dialog.error = None;
        dialog.busy = true;
        let host = dialog.host.clone();
        let store = self.store.clone();
        let work = Work::Login {
            host: host.clone(),
            user: user.clone(),
        };
        let id = self.spawn(work, move |cancel| {
            let proceed = || !cancel.load(Ordering::SeqCst);
            let result = accounts::log_in(&path, &host, &user, &secret, &*store, &proceed)
                .map_err(LoginFailure::from);
            Outcome::Login { host, user, result }
        });
        if let Some(dialog) = &mut self.login {
            dialog.job = Some(id);
        }
    }

    /// Close Log in; a login in flight is cancelled before it keeps anything.
    pub fn cancel_login(&mut self) {
        if let Some(id) = self.login.take().and_then(|dialog| dialog.job) {
            self.cancel_job(id);
        }
    }

    /// Delete the selected registry's stored secret in the background.
    pub fn log_out_selected(&mut self) {
        if !self.can_log_out() {
            return;
        }
        let (Some(path), Some(host)) = (
            self.path.clone(),
            self.selected_row().map(|row| row.entry.host.clone()),
        ) else {
            return;
        };
        self.rows[self.selected].sign_in = SignIn::Checking;
        let store = self.store.clone();
        let target = host.clone();
        self.spawn(Work::LogOut { host }, move |_| Outcome::LogOut {
            result: accounts::log_out(&path, &target, &*store).map_err(|e| e.to_string()),
            host: target,
        });
    }

    /// Probe the selected registry anonymously, then with its login.
    pub fn test_selected(&mut self) {
        if !self.can_test() {
            return;
        }
        let selected = self.selected;
        let entry = self.rows[selected].entry.clone();
        self.rows[selected].test = TestState::Running;
        let source = HostCredentials {
            configured_user: entry.user.clone(),
            env: self.env.clone(),
            store: self.store.clone(),
        };
        let host = entry.host.clone();
        self.spawn(Work::Test { host }, move |_| Outcome::Test {
            result: accounts::test_connection(&entry, &source).map_err(|e| e.to_string()),
            host: entry.host,
        });
    }

    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// Apply finished jobs; true when something changed. A job whose thread
    /// ended without an answer ends with an error.
    pub fn poll(&mut self) -> bool {
        let mut finished = Vec::new();
        let mut running = Vec::new();
        for job in std::mem::take(&mut self.jobs) {
            match job.receiver.try_recv() {
                Ok(outcome) => finished.push((job.id, outcome)),
                Err(mpsc::TryRecvError::Empty) => running.push(job),
                Err(mpsc::TryRecvError::Disconnected) => finished.push((job.id, job.work.lost())),
            }
        }
        self.jobs = running;
        let changed = !finished.is_empty();
        for (id, outcome) in finished {
            self.finish(id, outcome);
        }
        changed
    }

    /// Wait until no job runs, including jobs started by finishing others;
    /// tests only, the frame loop polls.
    #[cfg(test)]
    pub fn wait(&mut self) {
        while !self.jobs.is_empty() {
            for job in std::mem::take(&mut self.jobs) {
                let outcome = job.receiver.recv().unwrap_or_else(|_| job.work.lost());
                self.finish(job.id, outcome);
            }
        }
    }

    fn finish(&mut self, id: u64, outcome: Outcome) {
        match outcome {
            Outcome::SignIn { generation, states } => {
                if generation != self.generation {
                    return;
                }
                for (host, state) in states {
                    if let Some(row) = self.rows.iter_mut().find(|row| row.entry.host == host) {
                        row.sign_in = state;
                    }
                }
            }
            Outcome::Login { host, user, result } => {
                let current = self.login.as_ref().is_some_and(|d| d.job == Some(id));
                match result {
                    Ok(outcome) => {
                        if current {
                            self.login = None;
                        }
                        self.reload();
                        self.error = None;
                        let kept = match self.store.kind() {
                            StoreKind::Session => {
                                "the secret is kept in memory until the emulator quits"
                            }
                            StoreKind::Keychain => "the secret is in the macOS Keychain",
                        };
                        let unverified = match outcome {
                            LoginOutcome::Verified => "",
                            LoginOutcome::Unverifiable => {
                                " The registry answers without login, so it could not confirm the password."
                            }
                        };
                        self.status = Some(format!(
                            "Signed in to {host} as {user}; {kept}.{unverified}"
                        ));
                    }
                    // A dialog that was closed or replaced is not touched.
                    Err(failure) => {
                        if let Some(dialog) = self.login.as_mut().filter(|_| current) {
                            dialog.busy = false;
                            dialog.job = None;
                            dialog.error = Some(failure.message());
                        }
                    }
                }
            }
            Outcome::Test { host, result } => {
                if let Some(row) = self.rows.iter_mut().find(|row| row.entry.host == host) {
                    row.test = match result {
                        Ok(report) => TestState::Done(checks(&report)),
                        Err(error) => TestState::Failed(error),
                    };
                }
            }
            Outcome::LogOut { host, result } => {
                self.reload();
                match result {
                    Ok(_) => {
                        self.error = None;
                        self.status = Some(match self.store.kind() {
                            StoreKind::Session => {
                                format!("Logged out of {host}; its in-memory secret was dropped.")
                            }
                            StoreKind::Keychain => {
                                format!("Logged out of {host}; its Keychain item was deleted.")
                            }
                        });
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Outcome::Remove { host, result } => {
                self.reload();
                match result {
                    Ok(()) => {
                        self.error = None;
                        self.status = Some(format!("Removed {host} and its stored credentials."));
                    }
                    Err(error) => {
                        self.status = None;
                        self.error = Some(error);
                    }
                }
            }
        }
    }
}

/// The connection test as lines for the details pane.
pub(crate) fn checks(report: &ConnectionReport) -> Vec<Check> {
    let anonymous = match &report.anonymous {
        ApiAccess::BasicLogin => Check {
            mark: Mark::Good,
            text: "Anonymous GET /v2/ → 401 with a Basic challenge: login required".into(),
        },
        ApiAccess::Anonymous => Check {
            mark: Mark::Note,
            text: "Anonymous GET /v2/ → 200: the registry answers without login".into(),
        },
        ApiAccess::TokenService => Check {
            mark: Mark::Bad,
            text: "Anonymous GET /v2/ → 401 with a Bearer challenge: a token service; only Basic authentication is implemented".into(),
        },
        ApiAccess::Unsupported(scheme) => Check {
            mark: Mark::Bad,
            text: format!("Anonymous GET /v2/ → 401 with an unsupported challenge ({scheme})"),
        },
    };
    let login = match &report.login {
        LoginCheck::Accepted { user } => Check {
            mark: Mark::Good,
            text: format!("Basic login as {user} → 200: accepted"),
        },
        LoginCheck::Unverifiable { user } => Check {
            mark: Mark::Note,
            text: format!(
                "Basic login as {user} → 200, but the registry answers without login; credentials not verifiable"
            ),
        },
        LoginCheck::Rejected { user } => Check {
            mark: Mark::Bad,
            text: format!("Basic login as {user} → 401: refused; log in again"),
        },
        LoginCheck::NotSignedIn => Check {
            mark: Mark::Note,
            text: "Basic login: not signed in, nothing to check".into(),
        },
        LoginCheck::NotTried => Check {
            mark: Mark::Note,
            text: "Basic login: not tried".into(),
        },
    };
    vec![anonymous, login]
}

/// Where secrets are kept, under the registries list.
pub(crate) fn secret_hint(kind: StoreKind) -> String {
    match kind {
        StoreKind::Session => format!(
            "No system keychain found on this computer: passwords and tokens are kept in memory for this session only, never in preferences.toml, a plain-text file or any project file. {ENV_HINT}"
        ),
        StoreKind::Keychain => "Passwords and tokens are kept in the macOS Keychain, never in preferences.toml, a plain-text file or any project file. Scripts and CI pass credentials through the UNEMU_REGISTRY_* environment variables instead.".into(),
    }
}

/// The Log in dialog's description of the detected store.
pub(crate) fn store_description(kind: StoreKind) -> (&'static str, &'static str) {
    match kind {
        StoreKind::Keychain => (
            "macOS Keychain",
            "Stored and read through the system security service, so later sign-ins need no further prompts.",
        ),
        StoreKind::Session => (
            "No system keychain found: the password is kept for this session only",
            "Nothing is written to disk. After the emulator quits, you log in again.",
        ),
    }
}

/// One line of the registries table: a registry, or one of its namespaces
/// right under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableRow {
    /// The registry row this line belongs to; choosing the line selects it.
    pub row: usize,
    pub namespace: bool,
    pub name: String,
    pub user: String,
    pub signed_in: String,
    pub private: String,
}

impl RegistriesPage {
    /// The table as the mock-up shows it: each registry, then a child line
    /// per listed namespace. A registry without namespaces is private or not
    /// as a whole; one with namespaces says "Per namespace".
    pub fn table(&self) -> Vec<TableRow> {
        let session = self.store.kind() == StoreKind::Session;
        let mut lines = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            let entry = &row.entry;
            let mut name = entry.host.clone();
            if entry.host == self.default_host {
                name.push_str("  [default]");
            }
            if !row.listed {
                name.push_str("  (built in)");
            }
            lines.push(TableRow {
                row: index,
                namespace: false,
                name,
                user: entry.user.clone().unwrap_or_else(|| "—".into()),
                signed_in: match &row.sign_in {
                    SignIn::SignedIn if session => "● Signed in · this session".into(),
                    SignIn::SignedIn => "● Signed in".into(),
                    SignIn::SignedOut => "○ Signed out".into(),
                    SignIn::Checking => "… Checking…".into(),
                    SignIn::Unknown(_) => "? Unknown".into(),
                },
                private: match (entry.namespaces.is_empty(), entry.private) {
                    (false, _) => "Per namespace",
                    (true, true) => "Yes",
                    (true, false) => "No",
                }
                .into(),
            });
            for namespace in &entry.namespaces {
                let private = entry.scope(&format!("{}/", namespace.name)).private();
                lines.push(TableRow {
                    row: index,
                    namespace: true,
                    name: format!(
                        "    {}/ — {}",
                        namespace.name,
                        namespace_label(&namespace.name, private)
                    ),
                    user: String::new(),
                    signed_in: String::new(),
                    private: if private { "Yes" } else { "No — open only" }.into(),
                });
            }
        }
        lines
    }
}

/// What a namespace holds, as the table says it beside the name.
fn namespace_label(name: &str, private: bool) -> &'static str {
    if name == OFFICIAL_NAMESPACE {
        "official namespace — open images, public pulls"
    } else if private {
        "proprietary images — login always required"
    } else {
        "public — open images only"
    }
}

/// The details pane's Namespaces line: each namespace, or the whole
/// registry, with the push rule its mark selects. There is no override.
pub(crate) fn namespaces_text(entry: &RegistryEntry) -> String {
    let rule = |private: bool| {
        if private {
            "private"
        } else {
            "public, open content only"
        }
    };
    if entry.namespaces.is_empty() {
        return format!("whole registry — {}", rule(entry.private));
    }
    entry
        .namespaces
        .iter()
        .map(|n| {
            let private = entry.scope(&format!("{}/", n.name)).private();
            format!("{}/ — {}", n.name, rule(private))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A configuration error without its "registry configuration:" prefix.
fn plain(error: &Error) -> String {
    match error {
        Error::Config(message) => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_registry::{
        credential_store::SessionStore,
        test_support::{MockRegistry, Policy},
    };

    const SECRET: &str = "synthetic-secret";

    struct Fixture {
        _temp: assembly::test_support::TempDir,
        path: PathBuf,
        store: Arc<SessionStore>,
        page: RegistriesPage,
    }

    fn fixture(preferences: &str) -> Fixture {
        let temp = assembly::test_support::TempDir::new("registries-page");
        let path = temp.path().join("preferences.toml");
        std::fs::write(&path, preferences).unwrap();
        let store = Arc::new(SessionStore::default());
        let page =
            RegistriesPage::new(Some(path.clone()), store.clone(), EnvCredentials::default());
        Fixture {
            _temp: temp,
            path,
            store,
            page,
        }
    }

    fn mock_registry() -> MockRegistry {
        MockRegistry::start(Policy {
            user: Some(("ci".into(), SECRET.into())),
            ..Policy::default()
        })
    }

    fn select(page: &mut RegistriesPage, host: &str) {
        let index = page
            .rows
            .iter()
            .position(|row| row.entry.host == host)
            .unwrap();
        page.select(index);
    }

    #[test]
    fn the_default_registry_is_listed_built_in_and_cannot_be_removed() {
        let mut f = fixture("rom_dirs = []\n");
        assert_eq!(f.page.rows.len(), 1);
        let row = &f.page.rows[0];
        assert_eq!(row.entry.host, DEFAULT_REGISTRY);
        assert!(!row.listed && !row.entry.private);
        assert_eq!(f.page.default_host, DEFAULT_REGISTRY);
        assert!(!f.page.can_remove() && !f.page.can_set_default());
        assert!(f
            .page
            .remove_blocked()
            .unwrap()
            .contains("default registry"));
        f.page.remove_selected();
        assert_eq!(f.page.rows.len(), 1);
        assert_eq!(f.page.signed_in_count(), 0);
        assert!(f.store.read(DEFAULT_REGISTRY, "x").unwrap().is_none());
    }

    #[test]
    fn add_validates_hosts_and_edits_keep_other_preferences() {
        let mut f = fixture("rom_dirs = [\"/roms\"]\nreduced_motion = true\n");
        for bad in ["", "nas", "nas.example/private", "https://nas.example"] {
            f.page.open_add();
            f.page.editor.as_mut().unwrap().host = bad.into();
            f.page.submit_editor();
            let error = f.page.editor.as_ref().unwrap().error.clone();
            assert!(error.unwrap().contains("dot or a port"), "{bad}");
        }
        f.page.open_add();
        let editor = f.page.editor.as_mut().unwrap();
        assert!(editor.private, "new registries start private");
        editor.host = "nas.example:5000".into();
        editor.plain_http = true;
        f.page.submit_editor();
        assert!(f
            .page
            .editor
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("loopback"));
        f.page.editor.as_mut().unwrap().plain_http = false;
        f.page.submit_editor();
        assert!(f.page.editor.is_none());
        assert_eq!(f.page.status.as_deref(), Some("Added nas.example:5000."));
        let row = f.page.selected_row().unwrap();
        assert!(row.listed && row.entry.private && row.entry.host == "nas.example:5000");

        f.page.open_add();
        f.page.editor.as_mut().unwrap().host = "NAS.example:5000".into();
        f.page.submit_editor();
        assert!(f
            .page
            .editor
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("already"));
        f.page.editor = None;

        let table = config::read_preferences(&f.path).unwrap();
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
        assert_eq!(table["reduced_motion"].as_bool(), Some(true));

        f.page.set_default_selected();
        assert_eq!(f.page.default_host, "nas.example:5000");
        assert!(!f.page.can_remove());
        select(&mut f.page, DEFAULT_REGISTRY);
        f.page.set_default_selected();
        assert_eq!(f.page.default_host, DEFAULT_REGISTRY);
        assert!(config::read_preferences(&f.path)
            .unwrap()
            .get("default_registry")
            .is_none());
        select(&mut f.page, "nas.example:5000");
        assert!(f.page.can_remove());
        f.page.remove_selected();
        f.page.wait();
        assert_eq!(f.page.rows.len(), 1);
        assert!(f
            .page
            .status
            .as_ref()
            .unwrap()
            .contains("Removed nas.example:5000"));
    }

    #[test]
    fn un_marking_private_needs_an_explicit_confirmation() {
        let mut f = fixture("[[registries]]\nhost = \"nas.example\"\nprivate = true\n");
        select(&mut f.page, "nas.example");
        f.page.open_edit();
        f.page.editor.as_mut().unwrap().private = false;
        f.page.submit_editor();
        assert_eq!(
            f.page.confirm,
            Some(ConfirmPublic {
                host: "nas.example".into(),
                exposed: "nas.example".into(),
                settings: EntrySettings::default(),
            })
        );
        assert!(f.page.selected_row().unwrap().entry.private);
        f.page.confirm_public(false);
        assert!(f.page.confirm.is_none());
        assert!(f.page.selected_row().unwrap().entry.private);
        assert!(f.page.editor.is_some(), "cancel returns to the editor");

        f.page.submit_editor();
        f.page.confirm_public(true);
        assert!(f.page.editor.is_none());
        assert!(!f.page.selected_row().unwrap().entry.private);
        // Marking private again needs no confirmation.
        f.page.open_edit();
        f.page.editor.as_mut().unwrap().private = true;
        f.page.submit_editor();
        assert!(f.page.confirm.is_none());
        assert!(f.page.selected_row().unwrap().entry.private);
    }

    #[test]
    fn login_checks_the_registry_and_never_writes_or_shows_the_secret() {
        let registry = mock_registry();
        let host = registry.host();
        let mut f = fixture(&format!(
            "rom_dirs = []\n[[registries]]\nhost = \"{host}\"\nprivate = true\nplain_http = true\n"
        ));
        select(&mut f.page, &host);
        assert!(f.page.can_log_in() && !f.page.can_log_out());
        f.page.open_login();
        let dialog = f.page.login.as_mut().unwrap();
        dialog.user = "ci".into();
        f.page.submit_login();
        assert_eq!(
            f.page.login.as_ref().unwrap().error.as_deref(),
            Some("Enter a password or an access token.")
        );
        f.page.login.as_mut().unwrap().password = "wrong-secret".into();
        f.page.submit_login();
        assert!(f.page.login.as_ref().unwrap().password.is_empty());
        assert!(f.page.login.as_ref().unwrap().busy);
        f.page.wait();
        let dialog = f.page.login.as_ref().unwrap();
        assert!(!dialog.busy);
        assert_eq!(
            dialog.error.as_deref(),
            Some("The registry refused the user name or password (401). Nothing was stored.")
        );
        assert!(!f.store.contains(&host, "ci").unwrap());
        assert_eq!(f.page.selected_row().unwrap().sign_in, SignIn::SignedOut);

        f.page.login.as_mut().unwrap().password = SECRET.into();
        let debug = format!("{:?}", f.page.login.as_ref().unwrap());
        assert!(!debug.contains(SECRET), "{debug}");
        f.page.submit_login();
        f.page.wait();
        assert!(f.page.login.is_none());
        let row = f.page.selected_row().unwrap();
        assert_eq!(row.sign_in, SignIn::SignedIn);
        assert_eq!(row.entry.user.as_deref(), Some("ci"));
        assert_eq!(f.page.signed_in_count(), 1);
        assert!(f.page.can_log_out() && !f.page.can_log_in());
        let status = f.page.status.clone().unwrap();
        assert!(
            status.contains("kept in memory until the emulator quits"),
            "{status}"
        );

        let written = std::fs::read_to_string(&f.path).unwrap();
        let rows = format!("{:?}", f.page.rows);
        for text in [written.as_str(), status.as_str(), rows.as_str()] {
            assert!(!text.contains(SECRET), "{text}");
        }

        f.page.log_out_selected();
        assert_eq!(f.page.selected_row().unwrap().sign_in, SignIn::Checking);
        assert!(f.page.account_busy() && !f.page.can_log_in());
        f.page.wait();
        assert_eq!(f.page.selected_row().unwrap().sign_in, SignIn::SignedOut);
        assert_eq!(
            f.page.selected_row().unwrap().entry.user.as_deref(),
            Some("ci")
        );
        assert!(!f.store.contains(&host, "ci").unwrap());
        assert!(f
            .page
            .status
            .as_ref()
            .unwrap()
            .contains("in-memory secret was dropped"));
    }

    #[test]
    fn test_connection_shows_each_result_state() {
        let registry = mock_registry();
        let tokens = MockRegistry::start(Policy {
            bearer_challenge: true,
            ..Policy::default()
        });
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let unreachable = closed.local_addr().unwrap().to_string();
        drop(closed);
        let entries: String = [&registry.host(), &tokens.host(), &unreachable]
            .iter()
            .map(|host| format!("[[registries]]\nhost = \"{host}\"\nprivate = true\nplain_http = true\nuser = \"ci\"\n"))
            .collect();
        let mut f = fixture(&entries);
        let host = registry.host();
        select(&mut f.page, &host);
        assert_eq!(f.page.selected_row().unwrap().test, TestState::NotRun);
        f.page.test_selected();
        assert_eq!(f.page.selected_row().unwrap().test, TestState::Running);
        assert!(!f.page.can_test());
        f.page.wait();
        let TestState::Done(lines) = &f.page.selected_row().unwrap().test else {
            panic!("{:?}", f.page.selected_row());
        };
        assert_eq!(lines[0].mark, Mark::Good);
        assert_eq!(
            lines[1].text,
            "Basic login: not signed in, nothing to check"
        );

        f.store.store(&host, "ci", &Secret::new(SECRET)).unwrap();
        f.page.reload();
        f.page.test_selected();
        f.page.wait();
        let TestState::Done(lines) = &f.page.selected_row().unwrap().test else {
            panic!();
        };
        assert_eq!(
            lines[1],
            Check {
                mark: Mark::Good,
                text: "Basic login as ci → 200: accepted".into()
            }
        );
        f.store
            .store(&host, "ci", &Secret::new("wrong-secret"))
            .unwrap();
        f.page.test_selected();
        f.page.wait();
        let TestState::Done(lines) = &f.page.selected_row().unwrap().test else {
            panic!();
        };
        assert_eq!(lines[1].mark, Mark::Bad);

        select(&mut f.page, &tokens.host());
        f.page.test_selected();
        f.page.wait();
        let TestState::Done(lines) = &f.page.selected_row().unwrap().test else {
            panic!();
        };
        assert!(lines[0].text.contains("Bearer") && lines[0].mark == Mark::Bad);

        select(&mut f.page, &unreachable);
        f.page.test_selected();
        f.page.wait();
        assert!(matches!(
            &f.page.selected_row().unwrap().test,
            TestState::Failed(message) if message.starts_with("network:")
        ));
        // Results stay with their registry across a reload.
        f.page.reload();
        select(&mut f.page, &host);
        assert!(matches!(
            f.page.selected_row().unwrap().test,
            TestState::Done(_)
        ));
    }

    #[test]
    fn the_session_store_says_so_and_points_to_the_environment() {
        let hint = secret_hint(StoreKind::Session);
        assert!(hint.contains("this session only") && hint.contains("UNEMU_REGISTRY_USER"));
        assert!(secret_hint(StoreKind::Keychain).contains("macOS Keychain"));
        assert!(store_description(StoreKind::Session)
            .1
            .contains("log in again"));
    }

    #[test]
    fn a_cli_login_made_meanwhile_shows_after_the_next_change() {
        let mut f = fixture("[[registries]]\nhost = \"nas.example\"\nprivate = true\n");
        assert_eq!(f.page.selected_row().unwrap().entry.host, DEFAULT_REGISTRY);
        // `image login` records a user and stores its secret in the same store.
        config::update_preferences(&f.path, |t| config::set_user(t, "nas.example", "me")).unwrap();
        f.store
            .store("nas.example", "me", &Secret::new(SECRET))
            .unwrap();
        f.page.open_add();
        f.page.editor.as_mut().unwrap().host = "lab.example".into();
        f.page.submit_editor();
        select(&mut f.page, "nas.example");
        assert_eq!(f.page.selected_row().unwrap().sign_in, SignIn::Checking);
        f.page.wait();
        let row = f.page.selected_row().unwrap();
        assert_eq!(row.entry.user.as_deref(), Some("me"));
        assert_eq!(row.sign_in, SignIn::SignedIn);
    }

    #[test]
    fn saving_an_entry_that_changed_meanwhile_is_refused_in_both_directions() {
        let host = "127.0.0.1:5000";
        let entry =
            |private: bool| format!("[[registries]]\nhost = \"{host}\"\nprivate = {private}\n");
        for (shown, meanwhile) in [(true, false), (false, true)] {
            let mut f = fixture(&entry(shown));
            select(&mut f.page, host);
            f.page.open_edit();
            assert_eq!(f.page.editor.as_ref().unwrap().private, shown);
            // Another writer flips the flag while the dialog is open.
            std::fs::write(&f.path, entry(meanwhile)).unwrap();
            f.page.editor.as_mut().unwrap().plain_http = true;
            f.page.submit_editor();
            let error = f.page.editor.as_ref().unwrap().error.clone().unwrap();
            assert!(error.contains("changed meanwhile"), "{error}");
            assert_eq!(std::fs::read_to_string(&f.path).unwrap(), entry(meanwhile));
            // Reopening shows the current value, and the edit then applies.
            f.page.open_edit();
            assert_eq!(f.page.editor.as_ref().unwrap().private, meanwhile);
            f.page.editor.as_mut().unwrap().plain_http = true;
            f.page.submit_editor();
            assert!(f.page.editor.is_none());
            let row = f.page.selected_row().unwrap();
            assert_eq!((row.entry.private, row.entry.plain_http), (meanwhile, true));
        }
    }

    /// A store whose `check` waits until the test releases it, one gate per
    /// call, and which remembers the threads it was called on.
    #[derive(Default)]
    struct Gated {
        inner: SessionStore,
        gates: std::sync::Mutex<std::collections::VecDeque<mpsc::Receiver<()>>>,
        entered: std::sync::Mutex<Option<mpsc::Sender<()>>>,
        threads: std::sync::Mutex<Vec<std::thread::ThreadId>>,
    }
    impl Gated {
        fn record(&self) {
            self.threads
                .lock()
                .unwrap()
                .push(std::thread::current().id());
        }
        fn gate(&self) -> (mpsc::Sender<()>, mpsc::Receiver<()>) {
            let (release, gate) = mpsc::channel();
            self.gates.lock().unwrap().push_back(gate);
            let (entered, entries) = mpsc::channel();
            *self.entered.lock().unwrap() = Some(entered);
            (release, entries)
        }
    }
    impl CredentialStore for Gated {
        fn kind(&self) -> StoreKind {
            StoreKind::Session
        }
        fn check(&self, _: &str, _: &str, _: &Secret) -> image_registry::Result<()> {
            self.record();
            let gate = self.gates.lock().unwrap().pop_front();
            if let Some(gate) = gate {
                if let Some(entered) = &*self.entered.lock().unwrap() {
                    let _ = entered.send(());
                }
                let _ = gate.recv();
            }
            Ok(())
        }
        fn read(&self, host: &str, user: &str) -> image_registry::Result<Option<Secret>> {
            self.record();
            self.inner.read(host, user)
        }
        fn contains(&self, host: &str, user: &str) -> image_registry::Result<bool> {
            self.record();
            self.inner.contains(host, user)
        }
        fn store(&self, host: &str, user: &str, secret: &Secret) -> image_registry::Result<()> {
            self.record();
            self.inner.store(host, user, secret)
        }
        fn delete(&self, host: &str, user: &str) -> image_registry::Result<()> {
            self.record();
            self.inner.delete(host, user)
        }
    }

    fn gated_page(
        registry: &MockRegistry,
    ) -> (
        assembly::test_support::TempDir,
        PathBuf,
        Arc<Gated>,
        RegistriesPage,
    ) {
        let temp = assembly::test_support::TempDir::new("registries-gated");
        let path = temp.path().join("preferences.toml");
        std::fs::write(
            &path,
            format!(
                "[[registries]]\nhost = \"{}\"\nprivate = true\nplain_http = true\n",
                registry.host()
            ),
        )
        .unwrap();
        let store = Arc::new(Gated::default());
        let page =
            RegistriesPage::new(Some(path.clone()), store.clone(), EnvCredentials::default());
        (temp, path, store, page)
    }

    fn start_login(page: &mut RegistriesPage, host: &str) {
        select(page, host);
        page.open_login();
        let dialog = page.login.as_mut().unwrap();
        dialog.user = "ci".into();
        dialog.password = SECRET.into();
        page.submit_login();
    }

    #[test]
    fn cancelling_a_login_after_it_was_verified_stores_nothing() {
        let registry = mock_registry();
        let host = registry.host();
        let (_temp, path, store, mut page) = gated_page(&registry);
        let before = std::fs::read(&path).unwrap();
        let (release, entered) = store.gate();
        start_login(&mut page, &host);
        entered.recv().unwrap();
        page.cancel_login();
        assert!(page.login.is_none());
        release.send(()).unwrap();
        page.wait();
        assert!(
            !registry.authorized_log().is_empty(),
            "the login was verified"
        );
        assert!(!store.inner.contains(&host, "ci").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(page.error.is_none() && page.status.is_none());

        // Closing Preferences cancels a login in flight as well.
        let (release, entered) = store.gate();
        start_login(&mut page, &host);
        entered.recv().unwrap();
        drop(page);
        release.send(()).unwrap();
        while Arc::strong_count(&store) > 1 {
            std::thread::yield_now();
        }
        assert!(!store.inner.contains(&host, "ci").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_stale_login_never_changes_a_newer_dialog() {
        let registry = mock_registry();
        let host = registry.host();
        let (_temp, _path, store, mut page) = gated_page(&registry);
        let (release_old, entered) = store.gate();
        start_login(&mut page, &host);
        entered.recv().unwrap();
        page.cancel_login();
        // A cancelled login does not hold back a new one.
        assert!(page.can_log_in());
        let (release_new, entered) = store.gate();
        start_login(&mut page, &host);
        entered.recv().unwrap();
        release_old.send(()).unwrap();
        while page.jobs.len() > 1 {
            page.poll();
            std::thread::yield_now();
        }
        let dialog = page.login.as_ref().unwrap();
        assert!(dialog.busy && dialog.error.is_none(), "{dialog:?}");
        release_new.send(()).unwrap();
        page.wait();
        assert!(page.login.is_none());
        assert_eq!(page.selected_row().unwrap().sign_in, SignIn::SignedIn);
    }

    #[test]
    fn the_credential_store_is_never_called_on_the_caller_thread() {
        let registry = mock_registry();
        let host = registry.host();
        let temp = assembly::test_support::TempDir::new("registries-threads");
        let path = temp.path().join("preferences.toml");
        std::fs::write(
            &path,
            format!(
                "[[registries]]\nhost = \"{host}\"\nprivate = true\nplain_http = true\nuser = \"ci\"\n[[registries]]\nhost = \"lab.example\"\nuser = \"me\"\n"
            ),
        )
        .unwrap();
        let store = Arc::new(Gated::default());
        store
            .inner
            .store(&host, "ci", &Secret::new(SECRET))
            .unwrap();
        let mut page = RegistriesPage::new(Some(path), store.clone(), EnvCredentials::default());
        assert!(page
            .rows
            .iter()
            .filter(|r| r.entry.user.is_some())
            .all(|r| r.sign_in == SignIn::Checking));
        page.wait();
        select(&mut page, &host);
        assert_eq!(page.selected_row().unwrap().sign_in, SignIn::SignedIn);
        page.test_selected();
        page.log_out_selected();
        page.wait();
        select(&mut page, "lab.example");
        page.remove_selected();
        page.wait();
        let caller = std::thread::current().id();
        let threads = store.threads.lock().unwrap().clone();
        assert!(threads.len() >= 4, "{threads:?}");
        assert!(threads.iter().all(|thread| *thread != caller));
    }

    #[test]
    fn a_job_that_dies_ends_with_an_error() {
        let mut f = fixture("[[registries]]\nhost = \"nas.example\"\nuser = \"me\"\n");
        f.page.wait();
        select(&mut f.page, "nas.example");
        f.page.rows[f.page.selected].test = TestState::Running;
        let (_sender, receiver) = mpsc::channel::<Outcome>();
        drop(_sender);
        f.page.jobs.push(Job {
            id: 99,
            work: Work::Test {
                host: "nas.example".into(),
            },
            cancel: Arc::default(),
            receiver,
        });
        assert!(f.page.poll());
        assert!(!f.page.busy());
        assert_eq!(
            f.page.selected_row().unwrap().test,
            TestState::Failed(LOST.into())
        );
    }

    #[test]
    fn the_login_hint_names_the_transport_and_the_keychain_limit() {
        let mut f = fixture("[[registries]]\nhost = \"127.0.0.1:5000\"\nplain_http = true\n");
        select(&mut f.page, "127.0.0.1:5000");
        f.page.open_login();
        assert!(f.page.login_hint().contains("plain HTTP"));
        assert!(f.page.login_hint().contains("UNEMU_REGISTRY_USER"));
        f.page.cancel_login();
        select(&mut f.page, DEFAULT_REGISTRY);
        f.page.open_login();
        assert!(f.page.login_hint().contains("over TLS"));
    }

    const NAMESPACED: &str = "[[registries]]\nhost = \"registry.unemu.dev\"\nuser = \"maintainer\"\nnamespaces = [{ name = \"unemu\", private = false }, { name = \"private\", private = true }]\n[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n[[registries]]\nhost = \"lab.example\"\n";

    #[test]
    fn the_table_lists_namespaces_under_their_registry_as_the_mock_up_does() {
        let mut f = fixture(NAMESPACED);
        f.page.wait();
        let table: Vec<_> = f
            .page
            .table()
            .into_iter()
            .map(|line| (line.row, line.name, line.user, line.signed_in, line.private))
            .collect();
        let line = |row, name: &str, user: &str, signed: &str, private: &str| {
            (row, name.into(), user.into(), signed.into(), private.into())
        };
        assert_eq!(
            table,
            [
                line(
                    0,
                    "registry.unemu.dev  [default]",
                    "maintainer",
                    "○ Signed out",
                    "Per namespace"
                ),
                line(
                    0,
                    "    unemu/ — official namespace — open images, public pulls",
                    "",
                    "",
                    "No — open only"
                ),
                line(
                    0,
                    "    private/ — proprietary images — login always required",
                    "",
                    "",
                    "Yes"
                ),
                line(1, "nas.example:5000", "—", "○ Signed out", "Yes"),
                line(2, "lab.example", "—", "○ Signed out", "No"),
            ]
        );
        let details: Vec<_> = f
            .page
            .rows
            .iter()
            .map(|r| namespaces_text(&r.entry))
            .collect();
        assert_eq!(
            details,
            [
                "unemu/ — public, open content only; private/ — private",
                "whole registry — private",
                "whole registry — public, open content only",
            ]
        );
        // The built-in default registry has no namespaces until they are set.
        let builtin = fixture("rom_dirs = []\n");
        let lines = builtin.page.table();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].name, "registry.unemu.dev  [default]  (built in)");
        assert_eq!(lines[0].private, "No");
    }

    #[test]
    fn the_add_dialog_lists_namespaces_and_edit_reads_them_back() {
        let mut f = fixture("rom_dirs = [\"/roms\"]\n");
        f.page.open_add();
        let editor = f.page.editor.as_mut().unwrap();
        assert!(editor.namespaces.is_empty(), "empty = the whole registry");
        editor.host = DEFAULT_REGISTRY.into();
        editor.namespaces = "unemu, private/".into();
        f.page.submit_editor();
        assert_eq!(
            f.page.status.as_deref(),
            Some("Added registry.unemu.dev."),
            "{:?}",
            f.page.editor
        );
        let entry = &f.page.selected_row().unwrap().entry;
        assert!(!entry.private);
        assert!(entry.scope("private/games").private());
        assert!(!entry.scope("unemu/open-at").private());
        assert_eq!(f.page.table().len(), 3);

        f.page.open_edit();
        let editor = f.page.editor.as_ref().unwrap();
        assert_eq!(
            (editor.namespaces.as_str(), editor.private),
            ("unemu, private", true)
        );
        f.page.submit_editor();
        assert_eq!(
            f.page.status.as_deref(),
            Some("No changes to registry.unemu.dev.")
        );

        // Adding a public namespace makes nothing public that was private.
        f.page.open_edit();
        f.page.editor.as_mut().unwrap().namespaces = "unemu, private, games:public".into();
        f.page.submit_editor();
        assert!(f.page.confirm.is_none());
        assert_eq!(f.page.status.as_deref(), Some("Saved registry.unemu.dev."));
        assert_eq!(f.page.table().len(), 4);

        for (typed, problem) in [
            ("unemu:private", "never private"),
            ("private/games", "Namespaces:"),
            ("a, a", "listed twice"),
        ] {
            f.page.open_edit();
            f.page.editor.as_mut().unwrap().namespaces = typed.into();
            f.page.submit_editor();
            let error = f.page.editor.as_ref().unwrap().error.clone().unwrap();
            assert!(error.contains(problem), "{typed}: {error}");
        }

        // Un-marking private/ asks first and names it.
        f.page.open_edit();
        f.page.editor.as_mut().unwrap().namespaces = "unemu, private:public, games:public".into();
        f.page.submit_editor();
        let confirm = f.page.confirm.clone().unwrap();
        assert_eq!(confirm.exposed, "registry.unemu.dev/private/");
        assert!(f
            .page
            .selected_row()
            .unwrap()
            .entry
            .scope("private/a")
            .private());
        f.page.confirm_public(true);
        assert!(!f
            .page
            .selected_row()
            .unwrap()
            .entry
            .scope("private/a")
            .private());

        // A dialog opened before another writer changed the list is stale.
        f.page.open_edit();
        config::update_preferences(&f.path, |table| {
            config::set_privacy(
                table,
                DEFAULT_REGISTRY,
                false,
                vec![image_registry::config::Namespace {
                    name: "private".into(),
                    private: true,
                }],
            )
        })
        .unwrap();
        f.page.editor.as_mut().unwrap().namespaces = "unemu".into();
        f.page.submit_editor();
        let error = f.page.editor.as_ref().unwrap().error.clone().unwrap();
        assert!(error.contains("changed meanwhile"), "{error}");
        let table = config::read_preferences(&f.path).unwrap();
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use image_registry::{
        credential_store::SessionStore,
        test_support::{MockRegistry, Policy},
    };

    #[test]
    fn the_page_the_command_line_and_the_shell_keep_each_others_keys() {
        let registry = MockRegistry::start(Policy {
            user: Some(("ci".into(), "synthetic-secret".into())),
            ..Policy::default()
        });
        let host = registry.host();
        let temp = assembly::test_support::TempDir::new("registries-merge");
        let path = temp.path().join("preferences.toml");
        std::fs::write(&path, "rom_dirs = []\nfuture_option = 1\n").unwrap();
        // The shell loads its copy at startup.
        let mut shell = crate::preferences::Preferences::load(&path).unwrap();
        let store = Arc::new(SessionStore::default());
        let mut page =
            RegistriesPage::new(Some(path.clone()), store.clone(), EnvCredentials::default());
        page.open_add();
        let editor = page.editor.as_mut().unwrap();
        editor.host = host.clone();
        editor.plain_http = true;
        page.submit_editor();
        page.set_default_selected();
        // `image login` runs the same accounts function with the same store.
        accounts::log_in(
            &path,
            &host,
            "ci",
            &Secret::new("synthetic-secret"),
            &*store,
            &|| true,
        )
        .unwrap();
        // The shell saves its own keys from the copy it loaded before both.
        shell.reduced_motion = true;
        shell.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("synthetic-secret"), "{text}");
        let table: toml::Table = text.parse().unwrap();
        assert_eq!(table["reduced_motion"].as_bool(), Some(true));
        assert_eq!(table["future_option"].as_integer(), Some(1));
        assert_eq!(table["default_registry"].as_str(), Some(host.as_str()));
        let entry = RegistryConfig::from_table(&table)
            .unwrap()
            .entry(&host)
            .unwrap();
        assert!(entry.private && entry.plain_http);
        assert_eq!(entry.user.as_deref(), Some("ci"));
        page.reload();
        page.wait();
        assert_eq!(page.selected_row().unwrap().sign_in, SignIn::SignedIn);
    }
}
