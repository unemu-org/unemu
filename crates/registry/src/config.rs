// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side registry list read from the shared preferences file; machine-images §5.1.
//!
//! The list carries a host, whether the registry is private, an optional user
//! name and a loopback-only plain-HTTP opt-in for test registries. It never
//! carries secrets: passwords come from the environment or the operating
//! system's credential store (see [`crate::credentials`]). Unknown preference
//! keys are ignored so the desktop shell and this crate can share one file.
//!
//! Writers share one rule ([`update_preferences`]): take the advisory lock on
//! the sibling `<file>.lock`, read the file afresh, change only their own keys
//! and replace the file atomically. The command line, the desktop shell's
//! Registries page and the shell's own settings all follow it, so none of them
//! drops a key another one wrote meanwhile.

use crate::{
    reference::{validate_host, DEFAULT_REGISTRY},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{
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
    /// Only a private registry may receive images that contain user-supplied blobs.
    #[serde(default)]
    pub private: bool,
    /// Account name for the credential store; the password is looked up separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Plain HTTP for a registry on a loopback address, such as a local test
    /// registry. TLS stays mandatory for every other host.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
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
    /// `(private, plain_http)` as the dialog showed them; `None` adds an entry.
    pub started: Option<(bool, bool)>,
    pub private: bool,
    pub plain_http: bool,
    /// The user confirmed that a private registry becomes public.
    pub confirmed_public: bool,
}

/// Apply `edit` to the document as it is now, keeping every other key, the
/// entry's user included. The edit is refused when the entry changed since
/// the dialog read it, so a stale dialog never re-marks or un-marks a
/// registry; a private registry becomes public only with confirmation. Only
/// the keys the user changed are written. A refused edit leaves `table` as it was.
pub fn apply_entry_edit(table: &mut toml::Table, edit: &EntryEdit) -> Result<()> {
    let host = edit.host.as_str();
    validate_host(host)?;
    let now = RegistryConfig::from_table(table)?;
    let listed = now.registries.iter().find(|entry| entry.host == host);
    let current = listed
        .map(|entry| (entry.private, entry.plain_http))
        .or_else(|| (host == DEFAULT_REGISTRY).then_some((false, false)));
    match edit.started {
        None if listed.is_some() => {
            return Err(config(format!("{host} is already in the list")));
        }
        Some(started) if current != Some(started) => {
            return Err(config(format!(
                "{host} changed meanwhile; reopen it to see its current settings"
            )));
        }
        _ => {}
    }
    let (was_private, was_plain_http) = current.unwrap_or((false, false));
    if was_private && !edit.private && !edit.confirmed_public {
        return Err(config(format!(
            "making {host} public needs the user's confirmation"
        )));
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
    if adding || edit.private != was_private {
        entry.insert("private".into(), edit.private.into());
    }
    if adding || edit.plain_http != was_plain_http {
        if edit.plain_http {
            entry.insert("plain_http".into(), true.into());
        } else {
            entry.remove("plain_http");
        }
    }
    RegistryConfig::from_table(&edited)?;
    *table = edited;
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
                plain_http: false
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
        EntryEdit {
            host: host.into(),
            started,
            private,
            plain_http,
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
}
