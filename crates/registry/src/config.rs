// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side registry list read from the shared preferences file; machine-images §5.1.
//!
//! The list carries a host, whether the registry is private, an optional user
//! name and a loopback-only plain-HTTP opt-in for test registries. It never
//! carries secrets: passwords come from the environment or the operating
//! system's credential store (see [`crate::credentials`]). Unknown preference
//! keys are ignored so the desktop shell and this crate can share one file.
//!
//! Privacy is set for the whole registry or per namespace, the first path
//! component of a repository. An entry without namespaces keeps the older
//! form, one `private` flag for every repository:
//!
//! ```toml
//! [[registries]]
//! host = "nas.example:5000"
//! private = true
//! ```
//!
//! An entry that lists namespaces marks each of them, and its own `private`
//! flag must then be absent or false; a push into a namespace the entry does
//! not list is refused ([`RegistryEntry::scope`]):
//!
//! ```toml
//! [[registries]]
//! host = "registry.unemu.dev"
//! namespaces = [{ name = "unemu", private = false }, { name = "private", private = true }]
//! ```
//!
//! The official `unemu` namespace takes open content only on every host, so an
//! entry that marks it private is refused when preferences are read.
//!
//! Writers share one rule ([`update_preferences`]): take the advisory lock on
//! the sibling `<file>.lock`, read the file afresh, change only their own keys
//! and replace the file atomically. The command line, the desktop shell's
//! Registries page and the shell's own settings all follow it, so none of them
//! drops a key another one wrote meanwhile.

use crate::{
    reference::{validate_host, validate_namespace, DEFAULT_REGISTRY, OFFICIAL_NAMESPACE},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::Write,
    path::{Path, PathBuf},
};

/// Environment variable that overrides the default registry host, for CI.
pub const DEFAULT_REGISTRY_ENV: &str = "UNEMU_REGISTRY";

/// One configured registry. Secrets are never part of this record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    /// `name[:port]`, exactly as it appears in references.
    pub host: String,
    /// The whole registry is private: it may receive images that contain
    /// user-supplied blobs. Used only while `namespaces` is empty.
    #[serde(default)]
    pub private: bool,
    /// Account name for the credential store; the password is looked up separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Plain HTTP for a registry on a loopback address, such as a local test
    /// registry. TLS stays mandatory for every other host.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plain_http: bool,
    /// The namespaces pushes may go to, each public or private; empty means
    /// the whole registry, as `private` says.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<Namespace>,
}

/// One namespace of a registry entry: the first path component of the
/// repositories it holds, such as `private` for `private/games`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
    pub name: String,
    /// Only a private namespace may receive images that contain
    /// user-supplied blobs.
    #[serde(default)]
    pub private: bool,
}

/// How an entry's privacy applies to one repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The entry lists no namespaces; its own flag covers every repository.
    WholeRegistry { private: bool },
    /// The repository is in this listed namespace.
    Namespace { name: String, private: bool },
    /// The entry lists namespaces and the repository is in none of them;
    /// `namespace` is its first path component, `None` for a repository
    /// whose name has a single component.
    Unlisted { namespace: Option<String> },
}

impl Scope {
    /// The entry lets user-supplied blobs go here, subject to the guard's
    /// other rules (the official namespace and the anonymous-read probe).
    pub fn private(&self) -> bool {
        match self {
            Self::WholeRegistry { private } | Self::Namespace { private, .. } => *private,
            Self::Unlisted { .. } => false,
        }
    }
}

/// The scope as the push checks say it, after "HOST is configured; ".
impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WholeRegistry { private: true } => {
                f.write_str("the whole registry is marked private")
            }
            Self::WholeRegistry { private: false } => f.write_str("the whole registry is public"),
            Self::Namespace {
                name,
                private: true,
            } => write!(f, "{name}/ is marked private"),
            Self::Namespace {
                name,
                private: false,
            } => write!(f, "{name}/ is public"),
            Self::Unlisted {
                namespace: Some(name),
            } => write!(f, "{name}/ is not a configured namespace on this registry"),
            Self::Unlisted { namespace: None } => f.write_str(
                "the repository is in none of the namespaces configured on this registry",
            ),
        }
    }
}

/// The settings the Add and Edit dialog and `image registry set` change; the
/// host and the user stay as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntrySettings {
    /// The whole-registry flag; false whenever `namespaces` is not empty.
    pub private: bool,
    pub namespaces: Vec<Namespace>,
    pub plain_http: bool,
}

impl RegistryEntry {
    /// The built-in entry for the default registry: public unless the user's
    /// preferences mark it private.
    pub fn builtin(host: &str) -> Self {
        Self {
            host: host.into(),
            private: false,
            user: None,
            plain_http: false,
            namespaces: Vec::new(),
        }
    }

    /// Where `repository` falls on this registry. A repository is in a
    /// namespace only below it: `private/games` is in `private`, a repository
    /// named just `private` is not. The official namespace is never private,
    /// whatever the entry says.
    pub fn scope(&self, repository: &str) -> Scope {
        if self.namespaces.is_empty() {
            return Scope::WholeRegistry {
                private: self.private,
            };
        }
        let first = repository.split_once('/').map(|(first, _)| first);
        match first.and_then(|name| self.namespaces.iter().find(|n| n.name == name)) {
            Some(namespace) => Scope::Namespace {
                name: namespace.name.clone(),
                private: namespace.private && namespace.name != OFFICIAL_NAMESPACE,
            },
            None => Scope::Unlisted {
                namespace: first.map(str::to_owned),
            },
        }
    }

    pub fn settings(&self) -> EntrySettings {
        EntrySettings {
            private: self.private,
            namespaces: self.namespaces.clone(),
            plain_http: self.plain_http,
        }
    }
}

/// The registries known to this host, as listed in preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistryConfig {
    /// `default_registry` from preferences, if set.
    pub default_registry: Option<String>,
    pub registries: Vec<RegistryEntry>,
}

#[derive(Deserialize)]
struct Preferences {
    #[serde(default)]
    default_registry: Option<String>,
    #[serde(default)]
    registries: Vec<RegistryEntry>,
}

impl RegistryConfig {
    /// Read the registry keys of a preferences document, ignoring all others.
    pub fn from_toml(text: &str) -> Result<Self> {
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| config(e))?;
        Self::from_table(&table)
    }

    /// Read the registry keys of a parsed preferences table.
    pub fn from_table(table: &toml::Table) -> Result<Self> {
        let mut relevant = toml::Table::new();
        for key in ["default_registry", "registries"] {
            if let Some(value) = table.get(key) {
                relevant.insert(key.into(), value.clone());
            }
        }
        let preferences: Preferences = toml::Value::Table(relevant)
            .try_into()
            .map_err(|e: toml::de::Error| config(e))?;
        let parsed = Self {
            default_registry: preferences.default_registry,
            registries: preferences.registries,
        };
        parsed.validate()?;
        Ok(parsed)
    }

    fn validate(&self) -> Result<()> {
        if let Some(host) = &self.default_registry {
            validate_host(host)?;
        }
        let mut seen = std::collections::BTreeSet::new();
        for entry in &self.registries {
            validate_host(&entry.host)?;
            if entry.host != entry.host.to_ascii_lowercase() {
                return Err(config(format!(
                    "registry host must be lowercase: {}",
                    entry.host
                )));
            }
            if !seen.insert(entry.host.as_str()) {
                return Err(config(format!("duplicate registry entry: {}", entry.host)));
            }
            if entry.plain_http && !is_loopback(&entry.host) {
                return Err(config(format!(
                    "plain HTTP is only allowed for a loopback registry, not {}",
                    entry.host
                )));
            }
            if let Some(user) = &entry.user {
                validate_user(user)?;
            }
            validate_namespaces(&entry.host, entry.private, &entry.namespaces)?;
        }
        Ok(())
    }

    /// Precedence: a command-line `--registry`, then the environment, then
    /// preferences, then the family default.
    pub fn default_host(&self, flag: Option<&str>, env: Option<&str>) -> Result<String> {
        let host = flag
            .or(env.filter(|h| !h.is_empty()))
            .or(self.default_registry.as_deref())
            .unwrap_or(DEFAULT_REGISTRY)
            .to_ascii_lowercase();
        validate_host(&host)?;
        Ok(host)
    }

    /// A configured entry, or the built-in entry for the family default.
    pub fn entry(&self, host: &str) -> Option<RegistryEntry> {
        self.registries
            .iter()
            .find(|e| e.host == host)
            .cloned()
            .or_else(|| (host == DEFAULT_REGISTRY).then(|| RegistryEntry::builtin(host)))
    }

    /// Pull may reach any TLS registry; unknown hosts are treated as public.
    pub fn entry_or_public(&self, host: &str) -> RegistryEntry {
        self.entry(host)
            .unwrap_or_else(|| RegistryEntry::builtin(host))
    }
}

/// Record `user` for `host` in a preferences table, keeping every other key.
/// A new entry starts public; only the user can mark a registry private.
pub fn set_user(table: &mut toml::Table, host: &str, user: &str) -> Result<()> {
    validate_host(host)?;
    validate_user(user)?;
    let registries = registries_mut(table)?;
    for value in registries.iter_mut() {
        let entry = value
            .as_table_mut()
            .ok_or_else(|| config("registries must be an array of tables"))?;
        if entry.get("host").and_then(toml::Value::as_str) == Some(host) {
            entry.insert("user".into(), user.into());
            return RegistryConfig::from_table(table).map(|_| ());
        }
    }
    registries.push(toml::Value::Table(toml::Table::from_iter([
        ("host".to_owned(), toml::Value::from(host)),
        ("private".to_owned(), toml::Value::from(false)),
        ("user".to_owned(), toml::Value::from(user)),
    ])));
    RegistryConfig::from_table(table).map(|_| ())
}

/// Normalize a registry host typed by a user: trimmed, lowercase, and shaped
/// so that a reference names it, with a dot, a port or `localhost`, and no
/// scheme or path.
pub fn validate_registry_host(input: &str) -> Result<String> {
    let host = input.trim().to_ascii_lowercase();
    let shaped = host.contains('.') || host.contains(':') || host == "localhost";
    if host.is_empty() || host.contains('/') || !shaped || validate_host(&host).is_err() {
        return Err(config(
            "enter a registry host with a dot or a port, or localhost, without a path",
        ));
    }
    Ok(host)
}

/// One edit of a registry entry, as the Add or Edit dialog made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryEdit {
    pub host: String,
    /// The settings as the dialog showed them; `None` adds an entry.
    pub started: Option<EntrySettings>,
    pub settings: EntrySettings,
    /// The user confirmed that what was private becomes public.
    pub confirmed_public: bool,
}

/// What an edit makes public that was private before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exposed {
    /// The registry was private as a whole and is public as a whole now.
    WholeRegistry,
    /// These namespaces were private, listed or as part of a private
    /// registry, and are public now.
    Namespaces(Vec<String>),
}

impl Exposed {
    /// `host` or `host/name/`, joined, as a confirmation names them.
    pub fn describe(&self, host: &str) -> String {
        match self {
            Self::WholeRegistry => host.to_owned(),
            Self::Namespaces(names) => names
                .iter()
                .map(|name| format!("{host}/{name}/"))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// Whether going from `current` to `next` makes anything public that was
/// private. A namespace that becomes unlisted is not public: pushes into it
/// are refused. The official namespace never counts, because it is never
/// private.
pub fn made_public(current: &EntrySettings, next: &EntrySettings) -> Option<Exposed> {
    let public_in_next = |name: &str| {
        if next.namespaces.is_empty() {
            !next.private
        } else {
            next.namespaces.iter().any(|n| n.name == name && !n.private)
        }
    };
    let names: Vec<String> = if current.namespaces.is_empty() {
        if !current.private {
            return None;
        }
        if next.namespaces.is_empty() {
            return (!next.private).then_some(Exposed::WholeRegistry);
        }
        next.namespaces
            .iter()
            .filter(|n| !n.private && n.name != OFFICIAL_NAMESPACE)
            .map(|n| n.name.clone())
            .collect()
    } else {
        current
            .namespaces
            .iter()
            .filter(|n| n.private && n.name != OFFICIAL_NAMESPACE && public_in_next(&n.name))
            .map(|n| n.name.clone())
            .collect()
    };
    (!names.is_empty()).then_some(Exposed::Namespaces(names))
}

/// Apply `edit` to the document as it is now, keeping every other key, the
/// entry's user included. The edit is refused when the entry changed since
/// the dialog read it, so a stale dialog never re-marks or un-marks a
/// registry or a namespace; what was private becomes public only with
/// confirmation. Only the keys the user changed are written. A refused edit
/// leaves `table` as it was.
pub fn apply_entry_edit(table: &mut toml::Table, edit: &EntryEdit) -> Result<()> {
    let host = edit.host.as_str();
    validate_host(host)?;
    let now = RegistryConfig::from_table(table)?;
    let listed = now.registries.iter().find(|entry| entry.host == host);
    let current = listed
        .map(RegistryEntry::settings)
        .or_else(|| (host == DEFAULT_REGISTRY).then(EntrySettings::default));
    match &edit.started {
        None if listed.is_some() => {
            return Err(config(format!("{host} is already in the list")));
        }
        Some(started) if current.as_ref() != Some(started) => {
            return Err(config(format!(
                "{host} changed meanwhile; reopen it to see its current settings"
            )));
        }
        _ => {}
    }
    let was = current.unwrap_or_default();
    let next = &edit.settings;
    if let Some(exposed) = made_public(&was, next) {
        if !edit.confirmed_public {
            return Err(config(format!(
                "making {} public needs the user's confirmation",
                exposed.describe(host)
            )));
        }
    }
    let adding = listed.is_none();
    let mut edited = table.clone();
    let registries = registries_mut(&mut edited)?;
    let index = match registries
        .iter()
        .position(|value| host_of(value) == Some(host))
    {
        Some(index) => index,
        None => {
            registries.push(toml::Value::Table(toml::Table::from_iter([(
                "host".to_owned(),
                toml::Value::from(host),
            )])));
            registries.len() - 1
        }
    };
    let entry = registries[index]
        .as_table_mut()
        .ok_or_else(|| config("registries must be an array of tables"))?;
    if adding || next.private != was.private {
        entry.insert("private".into(), next.private.into());
    }
    if adding || next.namespaces != was.namespaces {
        if next.namespaces.is_empty() {
            entry.remove("namespaces");
        } else {
            entry.insert("namespaces".into(), namespaces_value(&next.namespaces));
        }
    }
    if adding || next.plain_http != was.plain_http {
        if next.plain_http {
            entry.insert("plain_http".into(), true.into());
        } else {
            entry.remove("plain_http");
        }
    }
    RegistryConfig::from_table(&edited)?;
    *table = edited;
    Ok(())
}

/// Set how private `host` is, as `image registry set` does: the whole
/// registry private or public when `namespaces` is empty, otherwise exactly
/// these namespaces. The entry's user, its plain-HTTP opt-in and every other
/// key stay; an unlisted host gets an entry. The command itself is the
/// confirmation the dialog asks for when something private becomes public.
pub fn set_privacy(
    table: &mut toml::Table,
    host: &str,
    private: bool,
    namespaces: Vec<Namespace>,
) -> Result<()> {
    if private && !namespaces.is_empty() {
        return Err(config(
            "a registry that lists namespaces is private per namespace: mark each one NAME:private",
        ));
    }
    let current = RegistryConfig::from_table(table)?
        .entry(host)
        .map(|entry| entry.settings());
    let plain_http = current.as_ref().is_some_and(|c| c.plain_http);
    apply_entry_edit(
        table,
        &EntryEdit {
            host: host.into(),
            started: current,
            settings: EntrySettings {
                private,
                namespaces,
                plain_http,
            },
            confirmed_public: true,
        },
    )
}

/// One namespace as typed: `NAME`, optionally with a trailing `/`, and an
/// optional `:private` or `:public`; without one it is private when `private`
/// is set. The official namespace stays public without a suffix, because it
/// is never private, and `unemu:private` is refused.
pub fn parse_namespace(item: &str, private: bool) -> Result<Namespace> {
    let item = item.trim();
    let (name, marked) = match item.rsplit_once(':') {
        Some((name, "private")) => (name, Some(true)),
        Some((name, "public")) => (name, Some(false)),
        Some(_) => {
            return Err(config(format!(
                "namespace {item}: write NAME, NAME:private or NAME:public"
            )))
        }
        None => (item, None),
    };
    let name = name.trim().trim_end_matches('/');
    validate_namespace(name).map_err(|_| {
        config(format!(
            "namespace {name:?}: use lowercase letters and digits, separated by . _ or -, and no further /"
        ))
    })?;
    let official = name == OFFICIAL_NAMESPACE;
    if official && marked == Some(true) {
        return Err(config(official_never_private()));
    }
    Ok(Namespace {
        name: name.to_owned(),
        private: marked.unwrap_or(private && !official),
    })
}

/// A comma-separated namespaces list, as the Add and Edit dialog's field holds
/// it, each item read by [`parse_namespace`]. Empty means the whole registry.
pub fn parse_namespaces(text: &str, private: bool) -> Result<Vec<Namespace>> {
    let mut namespaces: Vec<Namespace> = Vec::new();
    for item in text.split(',').filter(|item| !item.trim().is_empty()) {
        let namespace = parse_namespace(item, private)?;
        if namespaces.iter().any(|n| n.name == namespace.name) {
            return Err(config(format!(
                "namespace {}/ is listed twice",
                namespace.name
            )));
        }
        namespaces.push(namespace);
    }
    Ok(namespaces)
}

/// `namespaces` written for the dialog's field, so that [`parse_namespaces`]
/// with the same `private` reads them back: a suffix only where a namespace
/// differs from `private`, never on the official namespace.
pub fn namespaces_field(namespaces: &[Namespace], private: bool) -> String {
    namespaces
        .iter()
        .map(|n| {
            if n.private == private || n.name == OFFICIAL_NAMESPACE {
                n.name.clone()
            } else if n.private {
                format!("{}:private", n.name)
            } else {
                format!("{}:public", n.name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn namespaces_value(namespaces: &[Namespace]) -> toml::Value {
    toml::Value::Array(
        namespaces
            .iter()
            .map(|n| {
                toml::Value::Table(toml::Table::from_iter([
                    ("name".to_owned(), toml::Value::from(n.name.as_str())),
                    ("private".to_owned(), toml::Value::from(n.private)),
                ]))
            })
            .collect(),
    )
}

fn official_never_private() -> String {
    format!(
        "{OFFICIAL_NAMESPACE}/ is the official namespace and is never private: it takes open content only"
    )
}

/// The namespaces of the entry for `host`: valid, distinct names, the official
/// one never private, and no whole-registry flag beside them.
fn validate_namespaces(host: &str, private: bool, namespaces: &[Namespace]) -> Result<()> {
    if private && !namespaces.is_empty() {
        return Err(config(format!(
            "{host} lists namespaces, so each namespace is marked private on its own; remove private = true from the entry"
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    for namespace in namespaces {
        validate_namespace(&namespace.name).map_err(|_| {
            config(format!(
                "{host}: invalid namespace name {:?}",
                namespace.name
            ))
        })?;
        if !seen.insert(namespace.name.as_str()) {
            return Err(config(format!(
                "{host}: namespace {}/ is listed twice",
                namespace.name
            )));
        }
        if namespace.private && namespace.name == OFFICIAL_NAMESPACE {
            return Err(config(format!("{host}: {}", official_never_private())));
        }
    }
    Ok(())
}

/// Remove the entry of `host`. The default registry stays until another one
/// is chosen, so short names never resolve to a host that was just removed.
pub fn remove_entry(table: &mut toml::Table, host: &str) -> Result<()> {
    if RegistryConfig::from_table(table)?.default_host(None, None)? == host {
        return Err(config(format!(
            "{host} is the default registry; choose another default first"
        )));
    }
    let registries = registries_mut(table)?;
    let before = registries.len();
    registries.retain(|value| host_of(value) != Some(host));
    if registries.len() == before {
        return Err(config(format!("{host} is not a configured registry")));
    }
    Ok(())
}

/// Make `host`, which must be listed or the family default, the registry that
/// short names resolve to. The family default is stored as the absence of the key.
pub fn set_default_registry(table: &mut toml::Table, host: &str) -> Result<()> {
    let listed = RegistryConfig::from_table(table)?
        .registries
        .iter()
        .any(|entry| entry.host == host);
    if host == DEFAULT_REGISTRY {
        table.remove("default_registry");
    } else if listed {
        table.insert("default_registry".into(), host.into());
    } else {
        return Err(config(format!("{host} is not a configured registry")));
    }
    RegistryConfig::from_table(table).map(|_| ())
}

fn registries_mut(table: &mut toml::Table) -> Result<&mut Vec<toml::Value>> {
    table
        .entry("registries")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| config("registries must be an array of tables"))
}

fn host_of(value: &toml::Value) -> Option<&str> {
    value.get("host").and_then(toml::Value::as_str)
}

/// The preferences document at `path`; a missing file is an empty document.
pub fn read_preferences(path: &Path) -> Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .map_err(|e| config(format!("invalid preferences {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(config(format!("cannot read {}: {e}", path.display()))),
    }
}

/// Read-modify-write of the shared preferences file under the advisory lock on
/// the sibling `<file>.lock`, which every writer takes. `change` sees the file
/// as it is now; when it fails, the file is left untouched. Returns the
/// document as written.
pub fn update_preferences(
    path: &Path,
    change: impl FnOnce(&mut toml::Table) -> Result<()>,
) -> Result<toml::Table> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let io = |e: std::io::Error| config(format!("cannot write {}: {e}", path.display()));
    std::fs::create_dir_all(parent).map_err(io)?;
    let mut name = path
        .file_name()
        .ok_or_else(|| config("preferences path has no file name"))?
        .to_owned();
    name.push(".lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(name))
        .map_err(io)?;
    lock.lock().map_err(io)?;
    let mut table = read_preferences(path)?;
    change(&mut table)?;
    let text = toml::to_string(&table).map_err(config)?;
    write_atomically(path, parent, &text).map_err(io)?;
    Ok(table)
}

/// Replace `path` by rename from a temporary sibling; the caller holds the lock.
fn write_atomically(path: &Path, parent: &Path, text: &str) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (temporary, mut file): (PathBuf, _) = loop {
        let temporary = parent.join(format!(
            ".preferences-registry-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            // A leftover from an earlier process with the same id.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Loopback addresses never leave this host, so a local test registry may use
/// plain HTTP. Name resolution is not consulted except for `localhost`.
pub fn is_loopback(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    name == "localhost"
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn validate_user(user: &str) -> Result<()> {
    if user.is_empty()
        || user.len() > 128
        || user.contains(':')
        || user.chars().any(char::is_control)
    {
        return Err(config("invalid registry user name"));
    }
    Ok(())
}

fn config(message: impl std::fmt::Display) -> Error {
    Error::Config(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_list_registries_without_secrets_and_ignore_other_keys() {
        let text = r#"
            rom_dirs = []
            reduced_motion = true
            default_registry = "nas.example:5000"
            [[registries]]
            host = "nas.example:5000"
            private = true
            user = "maintainer"
            [[registries]]
            host = "127.0.0.1:5001"
            plain_http = true
        "#;
        let config = RegistryConfig::from_toml(text).unwrap();
        assert_eq!(config.default_host(None, None).unwrap(), "nas.example:5000");
        let nas = config.entry("nas.example:5000").unwrap();
        assert!(nas.private);
        assert_eq!(nas.user.as_deref(), Some("maintainer"));
        assert!(config.entry("127.0.0.1:5001").unwrap().plain_http);
        assert!(config.entry("other.example").is_none());
        assert!(!config.entry(DEFAULT_REGISTRY).unwrap().private);
    }

    #[test]
    fn secrets_unknown_fields_and_remote_plain_http_are_refused() {
        for text in [
            "[[registries]]\nhost = \"a.example\"\npassword = \"x\"",
            "[[registries]]\nhost = \"a.example\"\nplain_http = true",
            "[[registries]]\nhost = \"a.example\"\n[[registries]]\nhost = \"a.example\"",
            "[[registries]]\nhost = \"A.example\"",
            "[[registries]]\nhost = \"a.example\"\nuser = \"a:b\"",
            "default_registry = \"not a host\"",
        ] {
            assert!(RegistryConfig::from_toml(text).is_err(), "{text}");
        }
    }

    #[test]
    fn default_host_precedence_is_flag_then_env_then_preferences() {
        let config = RegistryConfig {
            default_registry: Some("prefs.example".into()),
            registries: Vec::new(),
        };
        assert_eq!(
            config
                .default_host(Some("flag.example"), Some("env.example"))
                .unwrap(),
            "flag.example"
        );
        assert_eq!(
            config.default_host(None, Some("env.example")).unwrap(),
            "env.example"
        );
        assert_eq!(
            config.default_host(None, Some("")).unwrap(),
            "prefs.example"
        );
        assert_eq!(
            RegistryConfig::default().default_host(None, None).unwrap(),
            DEFAULT_REGISTRY
        );
    }

    #[test]
    fn login_records_the_user_and_keeps_every_other_preference() {
        let mut table: toml::Table =
            "rom_dirs = [\"/roms\"]\n[[registries]]\nhost = \"a.example\"\nprivate = true\n"
                .parse()
                .unwrap();
        set_user(&mut table, "a.example", "me").unwrap();
        set_user(&mut table, "b.example", "you").unwrap();
        let config = RegistryConfig::from_table(&table).unwrap();
        assert_eq!(
            config.entry("a.example").unwrap(),
            RegistryEntry {
                host: "a.example".into(),
                private: true,
                user: Some("me".into()),
                plain_http: false,
                namespaces: Vec::new(),
            }
        );
        assert!(!config.entry("b.example").unwrap().private);
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
    }

    #[test]
    fn only_loopback_hosts_count_as_loopback() {
        for host in ["127.0.0.1:5000", "localhost", "[::1]:80", "127.1.2.3"] {
            assert!(is_loopback(host), "{host}");
        }
        for host in ["registry.unemu.dev", "10.0.0.1:5000", "localhost.example"] {
            assert!(!is_loopback(host), "{host}");
        }
    }

    #[test]
    fn typed_hosts_need_a_dot_a_port_or_localhost_and_no_path() {
        for (input, host) in [
            (" NAS.example:5000 ", "nas.example:5000"),
            ("localhost", "localhost"),
            ("127.0.0.1:5001", "127.0.0.1:5001"),
            ("[::1]:5000", "[::1]:5000"),
        ] {
            assert_eq!(validate_registry_host(input).unwrap(), host);
        }
        for input in [
            "",
            "nas",
            "nas.example/private",
            "https://nas.example",
            "nas example.org",
            "nas.example:99999",
            "-bad.example",
        ] {
            assert!(validate_registry_host(input).is_err(), "{input:?}");
        }
    }

    fn edit(
        host: &str,
        started: Option<(bool, bool)>,
        private: bool,
        plain_http: bool,
    ) -> EntryEdit {
        let settings = |(private, plain_http)| EntrySettings {
            private,
            namespaces: Vec::new(),
            plain_http,
        };
        EntryEdit {
            host: host.into(),
            started: started.map(settings),
            settings: settings((private, plain_http)),
            confirmed_public: false,
        }
    }

    #[test]
    fn entry_edits_keep_every_other_key_and_guard_the_default() {
        let mut table: toml::Table =
            "rom_dirs = [\"/roms\"]\n[[registries]]\nhost = \"a.example\"\nuser = \"me\"\n"
                .parse()
                .unwrap();
        apply_entry_edit(
            &mut table,
            &edit("a.example", Some((false, false)), true, false),
        )
        .unwrap();
        apply_entry_edit(&mut table, &edit("127.0.0.1:5001", None, false, true)).unwrap();
        let before = table.clone();
        assert!(apply_entry_edit(&mut table, &edit("b.example", None, false, true)).is_err());
        assert!(apply_entry_edit(&mut table, &edit("a.example", None, true, false)).is_err());
        assert_eq!(
            table, before,
            "a refused edit leaves the document as it was"
        );
        let config = RegistryConfig::from_table(&table).unwrap();
        let a = config.entry("a.example").unwrap();
        assert!(a.private && !a.plain_http);
        assert_eq!(a.user.as_deref(), Some("me"));
        assert!(config.entry("127.0.0.1:5001").unwrap().plain_http);
        apply_entry_edit(
            &mut table,
            &edit("127.0.0.1:5001", Some((false, true)), false, false),
        )
        .unwrap();
        assert!(table["registries"][1].get("plain_http").is_none());

        assert!(set_default_registry(&mut table, "unlisted.example").is_err());
        set_default_registry(&mut table, "a.example").unwrap();
        assert_eq!(table["default_registry"].as_str(), Some("a.example"));
        assert!(remove_entry(&mut table, "a.example").is_err());
        set_default_registry(&mut table, DEFAULT_REGISTRY).unwrap();
        assert!(table.get("default_registry").is_none());
        remove_entry(&mut table, "a.example").unwrap();
        assert!(remove_entry(&mut table, "a.example").is_err());
        assert_eq!(table["registries"].as_array().unwrap().len(), 1);
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
    }

    #[test]
    fn stale_edits_and_unconfirmed_un_marking_are_refused() {
        let text = |private: bool| {
            format!("[[registries]]\nhost = \"127.0.0.1:5000\"\nprivate = {private}\n")
        };
        let host = "127.0.0.1:5000";
        // A: the dialog showed private; meanwhile it became public elsewhere.
        // Saving only the plain-HTTP change must not re-mark it private.
        let mut table: toml::Table = text(false).parse().unwrap();
        let stale = apply_entry_edit(&mut table, &edit(host, Some((true, false)), true, true));
        assert!(stale.unwrap_err().to_string().contains("changed meanwhile"));
        assert_eq!(table, text(false).parse::<toml::Table>().unwrap());
        // B: the dialog showed public; meanwhile it was marked private. Saving
        // the dialog must not un-mark it.
        let mut table: toml::Table = text(true).parse().unwrap();
        let stale = apply_entry_edit(&mut table, &edit(host, Some((false, false)), false, true));
        assert!(stale.unwrap_err().to_string().contains("changed meanwhile"));
        assert_eq!(table["registries"][0]["private"].as_bool(), Some(true));
        // Un-marking needs the confirmation flag, and only changed keys are written.
        let refused = apply_entry_edit(&mut table, &edit(host, Some((true, false)), false, false));
        assert!(refused.unwrap_err().to_string().contains("confirmation"));
        let mut confirmed = edit(host, Some((true, false)), false, false);
        confirmed.confirmed_public = true;
        apply_entry_edit(&mut table, &confirmed).unwrap();
        assert_eq!(table["registries"][0]["private"].as_bool(), Some(false));
        apply_entry_edit(&mut table, &edit(host, Some((false, false)), false, true)).unwrap();
        assert_eq!(table["registries"][0]["plain_http"].as_bool(), Some(true));
        let mut untouched: toml::Table = "[[registries]]\nhost = \"127.0.0.1:5000\"\n"
            .parse()
            .unwrap();
        apply_entry_edit(
            &mut untouched,
            &edit(host, Some((false, false)), false, true),
        )
        .unwrap();
        assert!(
            untouched["registries"][0].get("private").is_none(),
            "an unchanged flag is not written"
        );
    }

    #[test]
    fn a_failed_update_leaves_the_preferences_file_untouched() {
        let dir =
            std::env::temp_dir().join(format!("image-registry-preferences-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preferences.toml");
        std::fs::write(&path, "rom_dirs = []\nfuture = 1\n").unwrap();
        assert!(update_preferences(&path, |table| {
            set_user(table, "a.example", "me")?;
            Err(config("injected failure"))
        })
        .is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "rom_dirs = []\nfuture = 1\n"
        );
        let written =
            update_preferences(&path, |table| set_user(table, "a.example", "me")).unwrap();
        assert_eq!(written["future"].as_integer(), Some(1));
        assert_eq!(read_preferences(&path).unwrap(), written);
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension() == Some("tmp".as_ref()))
            .count();
        assert_eq!(leftovers, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const NAMESPACED: &str = r#"
        rom_dirs = []
        [[registries]]
        host = "registry.unemu.dev"
        user = "maintainer"
        namespaces = [{ name = "unemu", private = false }, { name = "private", private = true }]
    "#;

    fn namespace(name: &str, private: bool) -> Namespace {
        Namespace {
            name: name.into(),
            private,
        }
    }

    #[test]
    fn old_and_namespaced_entries_read_and_write_back_unchanged() {
        let old = "[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n";
        let nas = RegistryConfig::from_toml(old)
            .unwrap()
            .entry("nas.example:5000")
            .unwrap();
        assert!(nas.namespaces.is_empty());
        for repository in ["private/games", "anything/else", "games"] {
            assert_eq!(
                nas.scope(repository),
                Scope::WholeRegistry { private: true },
                "{repository}"
            );
        }

        let config = RegistryConfig::from_toml(NAMESPACED).unwrap();
        let entry = config.entry(DEFAULT_REGISTRY).unwrap();
        assert_eq!(
            entry.namespaces,
            [namespace("unemu", false), namespace("private", true)]
        );
        assert!(!entry.private);
        assert_eq!(
            entry.scope("private/ibm-at-games"),
            Scope::Namespace {
                name: "private".into(),
                private: true
            }
        );
        assert_eq!(entry.scope("unemu/open-at").to_string(), "unemu/ is public");
        assert_eq!(
            entry.scope("owner/game"),
            Scope::Unlisted {
                namespace: Some("owner".into())
            }
        );
        // A repository named like a namespace is not inside it.
        assert_eq!(entry.scope("private"), Scope::Unlisted { namespace: None });
        assert_eq!(
            entry.scope("private/a").to_string(),
            "private/ is marked private"
        );

        // The array-of-tables spelling is the same document.
        let tables = "[[registries]]\nhost = \"registry.unemu.dev\"\nuser = \"maintainer\"\n[[registries.namespaces]]\nname = \"unemu\"\n[[registries.namespaces]]\nname = \"private\"\nprivate = true\n";
        assert_eq!(RegistryConfig::from_toml(tables).unwrap(), config);

        // Writing keeps both forms as they were read.
        let dir =
            std::env::temp_dir().join(format!("image-registry-namespaces-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preferences.toml");
        for text in [old, NAMESPACED] {
            std::fs::write(&path, text).unwrap();
            let before = RegistryConfig::from_toml(text).unwrap();
            let written =
                update_preferences(&path, |table| set_user(table, "other.example", "me")).unwrap();
            let mut after = RegistryConfig::from_table(&written).unwrap();
            after.registries.retain(|e| e.host != "other.example");
            assert_eq!(after, before, "{text}");
            assert_eq!(
                RegistryConfig::from_table(&read_preferences(&path).unwrap()).unwrap(),
                RegistryConfig::from_table(&written).unwrap()
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_official_namespace_marked_private_and_bad_namespaces_are_refused() {
        let entry = |namespaces: &str, extra: &str| {
            format!("[[registries]]\nhost = \"a.example\"\n{extra}namespaces = [{namespaces}]\n")
        };
        let official =
            RegistryConfig::from_toml(&entry("{ name = \"unemu\", private = true }", ""))
                .unwrap_err()
                .to_string();
        assert!(official.contains("never private"), "{official}");
        for text in [
            entry("{ name = \"private\", private = true }", "private = true\n"),
            entry("{ name = \"a\" }, { name = \"a\", private = true }", ""),
            entry("{ name = \"Private\" }", ""),
            entry("{ name = \"a/b\" }", ""),
            entry("{ name = \"*\" }", ""),
            entry("{ name = \"\" }", ""),
            entry("{ name = \"a\", public = true }", ""),
        ] {
            assert!(RegistryConfig::from_toml(&text).is_err(), "{text}");
        }
        // An empty list is the whole registry, as before.
        let whole = RegistryConfig::from_toml(&entry("", "private = true\n")).unwrap();
        assert_eq!(
            whole.entry("a.example").unwrap().scope("x/y"),
            Scope::WholeRegistry { private: true }
        );
    }

    #[test]
    fn the_namespaces_field_reads_back_what_it_shows() {
        assert_eq!(parse_namespaces(" ", true).unwrap(), []);
        assert_eq!(
            parse_namespaces("unemu, private/", true).unwrap(),
            [namespace("unemu", false), namespace("private", true)],
            "the official namespace stays public"
        );
        assert_eq!(
            parse_namespaces("unemu,private:private,games", false).unwrap(),
            [
                namespace("unemu", false),
                namespace("private", true),
                namespace("games", false)
            ]
        );
        for text in ["unemu:private", "a, a:private", "a/b", "a:secret", "UPPER"] {
            assert!(parse_namespaces(text, true).is_err(), "{text}");
        }
        assert!(parse_namespace("unemu:private", false)
            .unwrap_err()
            .to_string()
            .contains("never private"));
        let mixed = [
            namespace("unemu", false),
            namespace("private", true),
            namespace("games", false),
        ];
        for private in [true, false] {
            let field = namespaces_field(&mixed, private);
            assert_eq!(parse_namespaces(&field, private).unwrap(), mixed, "{field}");
        }
        assert_eq!(
            namespaces_field(&mixed, true),
            "unemu, private, games:public"
        );
    }

    #[test]
    fn namespace_edits_ask_before_anything_private_becomes_public() {
        let settings = |private: bool, namespaces: &[Namespace]| EntrySettings {
            private,
            namespaces: namespaces.to_vec(),
            plain_http: false,
        };
        let split = [namespace("unemu", false), namespace("private", true)];
        let whole_private = settings(true, &[]);
        let whole_public = settings(false, &[]);
        assert_eq!(
            made_public(&whole_private, &whole_public),
            Some(Exposed::WholeRegistry)
        );
        assert_eq!(made_public(&whole_private, &settings(false, &split)), None);
        assert_eq!(
            made_public(&settings(false, &split), &whole_public),
            Some(Exposed::Namespaces(vec!["private".into()]))
        );
        assert_eq!(
            made_public(
                &settings(false, &split),
                &settings(false, &[namespace("private", false)])
            )
            .unwrap()
            .describe("h.example"),
            "h.example/private/"
        );
        assert_eq!(
            made_public(&settings(false, &split), &settings(false, &split[..1])),
            None,
            "an unlisted namespace refuses pushes, so it is not public"
        );
        assert_eq!(
            made_public(
                &whole_private,
                &settings(false, &[namespace("games", false)])
            ),
            Some(Exposed::Namespaces(vec!["games".into()]))
        );

        let mut table: toml::Table = NAMESPACED.parse().unwrap();
        let started = Some(settings(false, &split));
        let mut edit = EntryEdit {
            host: DEFAULT_REGISTRY.into(),
            started: started.clone(),
            settings: settings(
                false,
                &[namespace("unemu", false), namespace("private", false)],
            ),
            confirmed_public: false,
        };
        let before = table.clone();
        let refused = apply_entry_edit(&mut table, &edit).unwrap_err().to_string();
        assert!(
            refused.contains("registry.unemu.dev/private/ public needs the user's confirmation"),
            "{refused}"
        );
        assert_eq!(table, before);
        edit.confirmed_public = true;
        apply_entry_edit(&mut table, &edit).unwrap();
        let entry = RegistryConfig::from_table(&table)
            .unwrap()
            .entry(DEFAULT_REGISTRY)
            .unwrap();
        assert!(!entry.scope("private/x").private());
        assert_eq!(entry.user.as_deref(), Some("maintainer"));
        assert!(
            table["registries"][0].get("private").is_none(),
            "an unchanged flag is not written"
        );
        // A dialog that still shows the old namespaces is stale.
        edit.settings = settings(false, &split);
        edit.started = started;
        assert!(apply_entry_edit(&mut table, &edit)
            .unwrap_err()
            .to_string()
            .contains("changed meanwhile"));
    }

    #[test]
    fn set_privacy_lists_namespaces_or_marks_the_whole_registry() {
        let mut table: toml::Table = "rom_dirs = []\n".parse().unwrap();
        set_user(&mut table, "nas.example:5000", "me").unwrap();
        let split = vec![namespace("unemu", false), namespace("private", true)];
        set_privacy(&mut table, DEFAULT_REGISTRY, false, split.clone()).unwrap();
        set_privacy(&mut table, "nas.example:5000", true, Vec::new()).unwrap();
        let config = RegistryConfig::from_table(&table).unwrap();
        let default = config.entry(DEFAULT_REGISTRY).unwrap();
        assert_eq!(default.namespaces, split);
        assert!(default.scope("private/a").private() && !default.scope("unemu/a").private());
        let nas = config.entry("nas.example:5000").unwrap();
        assert!(nas.private && nas.namespaces.is_empty());
        assert_eq!(nas.user.as_deref(), Some("me"));
        // Back to one flag for the whole registry: the list goes away.
        set_privacy(&mut table, DEFAULT_REGISTRY, true, Vec::new()).unwrap();
        assert!(table["registries"][1].get("namespaces").is_none());
        assert_eq!(
            RegistryConfig::from_table(&table)
                .unwrap()
                .entry(DEFAULT_REGISTRY)
                .unwrap()
                .scope("private/a"),
            Scope::WholeRegistry { private: true }
        );
        assert!(set_privacy(&mut table, DEFAULT_REGISTRY, true, split).is_err());
        assert_eq!(table["rom_dirs"].as_array().map(Vec::len), Some(0));
    }
}
