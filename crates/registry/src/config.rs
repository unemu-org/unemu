// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side registry list read from the shared preferences file; machine-images §5.1.
//!
//! The list carries a host, whether the registry is private, an optional user
//! name and a loopback-only plain-HTTP opt-in for test registries. It never
//! carries secrets: passwords come from the environment or the operating
//! system's credential store (see [`crate::credentials`]). Unknown preference
//! keys are ignored so the desktop shell and this crate can share one file.

use crate::{
    reference::{validate_host, DEFAULT_REGISTRY},
    Error, Result,
};
use serde::{Deserialize, Serialize};

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
    let registries = table
        .entry("registries")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| config("registries must be an array of tables"))?;
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
}
