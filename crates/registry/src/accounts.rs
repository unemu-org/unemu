// SPDX-License-Identifier: AGPL-3.0-or-later
//! Per-registry accounts, shared by the command line and the desktop shell:
//! log in, log out, remove a registry and test a connection; machine-images §5.1.
//!
//! A login is checked before anything is kept: first whether the credential
//! store would accept the secret at all ([`CredentialStore::check`]), then the
//! anonymous API check (`GET /v2/`), then the same request with RFC 7617 Basic
//! credentials. Only an accepted login stores the secret in the credential
//! store ([`crate::credential_store`]) and then records the user name in
//! preferences under the shared lock ([`config::update_preferences`]). A
//! caller may cancel a login until its user name is recorded. Both frontends
//! call these functions with the store [`detect`] chose, so a login made in one
//! is visible in the other. Nothing here writes, logs or returns a secret.
//!
//! [`detect`]: crate::credential_store::detect

use crate::{
    client::{ApiAccess, Client},
    config::{self, RegistryConfig, RegistryEntry},
    credential_store::{CredentialStore, Secret},
    credentials::{Anonymous, CredentialSource, Credentials},
    Error, Result,
};
use std::path::Path;

/// How a configured login fared in a connection test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginCheck {
    /// The registry demands a login and accepted the credentials of `user`.
    Accepted { user: String },
    /// The registry answers without login, so a `200` to the credentials of
    /// `user` does not show that they are right.
    Unverifiable { user: String },
    /// The registry answered `401` to the credentials of `user`.
    Rejected { user: String },
    /// No credentials are available for this registry.
    NotSignedIn,
    /// The registry asks for a scheme this client does not implement.
    NotTried,
}

/// The outcome of "Test connection": the anonymous API check, then Basic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionReport {
    pub anonymous: ApiAccess,
    pub login: LoginCheck,
}

/// Reach the registry anonymously, then with the credentials `source` has for
/// it. A registry that cannot be reached is an error.
pub fn test_connection(
    entry: &RegistryEntry,
    source: &dyn CredentialSource,
) -> Result<ConnectionReport> {
    let found = source.credentials(&entry.host)?;
    let user = found.as_ref().map(|c| c.user.clone());
    let credentials: Box<dyn CredentialSource> = match found {
        Some(credentials) => Box::new(credentials),
        None => Box::new(Anonymous),
    };
    let client = Client::for_probes(entry, credentials)?;
    let anonymous = client.probe_api()?;
    let login = match (&anonymous, user) {
        (ApiAccess::TokenService | ApiAccess::Unsupported(_), _) => LoginCheck::NotTried,
        (_, None) => LoginCheck::NotSignedIn,
        (_, Some(user)) => match client.check_login() {
            Ok(true) if anonymous == ApiAccess::Anonymous => LoginCheck::Unverifiable { user },
            Ok(true) => LoginCheck::Accepted { user },
            Ok(false) => LoginCheck::NotTried,
            Err(Error::Unauthorized { .. }) => LoginCheck::Rejected { user },
            Err(error) => return Err(error),
        },
    };
    Ok(ConnectionReport { anonymous, login })
}

/// What an accepted login showed about the credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginOutcome {
    /// The registry demands a login and accepted these credentials.
    Verified,
    /// The registry answers without login, so it accepted the credentials
    /// without showing that they are right. They are kept all the same.
    Unverifiable,
}

/// Check `user` and `secret` against the registry `host`, then keep the secret
/// in `store` and the user name in the preferences at `path`. A refused login
/// stores nothing; preferences keep every key another writer changed meanwhile.
///
/// `proceed` is asked before the secret is stored and again before the user
/// name is recorded; when it answers false the login ends as
/// [`Error::Cancelled`] and a secret stored by this call is removed again.
pub fn log_in(
    path: &Path,
    host: &str,
    user: &str,
    secret: &Secret,
    store: &dyn CredentialStore,
    proceed: &dyn Fn() -> bool,
) -> Result<LoginOutcome> {
    if secret.is_empty() {
        return Err(Error::Config("enter a password or an access token".into()));
    }
    let mut table = config::read_preferences(path)?;
    config::set_user(&mut table, host, user)?;
    let entry = RegistryConfig::from_table(&table)?
        .entry(host)
        .ok_or_else(|| Error::Config(format!("{host} is not a configured registry")))?;
    store.check(host, user, secret)?;
    let outcome = verify(&entry, user, secret)?;
    if !proceed() {
        return Err(Error::Cancelled);
    }
    store.store(host, user, secret)?;
    if !proceed() {
        store.delete(host, user)?;
        return Err(Error::Cancelled);
    }
    config::update_preferences(path, |table| config::set_user(table, host, user))?;
    Ok(outcome)
}

fn verify(entry: &RegistryEntry, user: &str, secret: &Secret) -> Result<LoginOutcome> {
    let client = Client::for_probes(entry, Box::new(Credentials::new(user, secret.expose())))?;
    let anonymous = client.probe_api()?;
    match &anonymous {
        ApiAccess::TokenService => {
            return Err(Error::UnsupportedAuth(
                "bearer token service; only Basic authentication is implemented".into(),
            ))
        }
        ApiAccess::Unsupported(scheme) => {
            return Err(Error::UnsupportedAuth(format!("scheme {scheme}")))
        }
        ApiAccess::Anonymous | ApiAccess::BasicLogin => {}
    }
    if !client.check_login()? {
        return Err(Error::Config(
            "credentials are only sent over TLS or to a loopback registry".into(),
        ));
    }
    Ok(match anonymous {
        ApiAccess::Anonymous => LoginOutcome::Unverifiable,
        _ => LoginOutcome::Verified,
    })
}

/// Delete the stored secret of the configured user of `host`. The user name
/// stays in preferences for the next login. Returns that user, if any.
pub fn log_out(path: &Path, host: &str, store: &dyn CredentialStore) -> Result<Option<String>> {
    let config = RegistryConfig::from_table(&config::read_preferences(path)?)?;
    let user = config.entry(host).and_then(|entry| entry.user);
    if let Some(user) = &user {
        store.delete(host, user)?;
    }
    Ok(user)
}

/// Remove the entry of `host` from preferences together with its stored
/// secret. The default registry cannot be removed ([`config::remove_entry`]).
/// The user whose secret goes is read under the preferences lock, so a login
/// recorded meanwhile is not left behind with its secret.
pub fn remove(path: &Path, host: &str, store: &dyn CredentialStore) -> Result<()> {
    config::update_preferences(path, |table| {
        let user = RegistryConfig::from_table(table)?
            .entry(host)
            .and_then(|entry| entry.user);
        config::remove_entry(table, host)?;
        if let Some(user) = &user {
            store.delete(host, user)?;
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        credential_store::SessionStore,
        credentials::{EnvCredentials, HostCredentials},
        test_support::{MockRegistry, Policy},
        tests::Temp,
    };
    use std::sync::Arc;

    const SECRET: &str = "synthetic-secret";

    fn user_policy() -> Policy {
        Policy {
            user: Some(("ci".into(), SECRET.into())),
            ..Policy::default()
        }
    }

    fn preferences(dir: &Temp, registry: &MockRegistry) -> std::path::PathBuf {
        let path = dir.path().join("preferences.toml");
        std::fs::write(
            &path,
            format!(
                "rom_dirs = [\"/roms\"]\n[[registries]]\nhost = \"{}\"\nprivate = true\nplain_http = true\n",
                registry.host()
            ),
        )
        .unwrap();
        path
    }

    /// A store that fails, or runs `during` while it stores, as a slow
    /// Keychain prompt would.
    struct Hooked<F: Fn() + Send + Sync> {
        inner: SessionStore,
        fail: bool,
        during: F,
    }
    impl<F: Fn() + Send + Sync> CredentialStore for Hooked<F> {
        fn kind(&self) -> crate::credential_store::StoreKind {
            self.inner.kind()
        }
        fn read(&self, host: &str, user: &str) -> Result<Option<Secret>> {
            self.inner.read(host, user)
        }
        fn store(&self, host: &str, user: &str, secret: &Secret) -> Result<()> {
            (self.during)();
            if self.fail {
                return Err(Error::Credentials("injected store failure".into()));
            }
            self.inner.store(host, user, secret)
        }
        fn delete(&self, host: &str, user: &str) -> Result<()> {
            self.inner.delete(host, user)
        }
    }

    #[test]
    fn a_refused_login_stores_nothing_and_an_accepted_one_keeps_every_other_key() {
        let dir = Temp::new();
        let registry = MockRegistry::start(user_policy());
        let path = preferences(&dir, &registry);
        let host = registry.host();
        let before = std::fs::read(&path).unwrap();
        let store = SessionStore::default();
        let refused = log_in(
            &path,
            &host,
            "ci",
            &Secret::new("wrong-secret"),
            &store,
            &|| true,
        );
        assert!(
            matches!(&refused, Err(Error::Unauthorized { .. })),
            "{refused:?}"
        );
        let text = format!("{} {:?}", refused.unwrap_err(), store);
        assert!(!text.contains("wrong-secret"), "{text}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!store.contains(&host, "ci").unwrap());
        assert!(log_in(&path, &host, "ci", &Secret::new(""), &store, &|| true).is_err());

        log_in(&path, &host, "ci", &Secret::new(SECRET), &store, &|| true).unwrap();
        assert_eq!(store.read(&host, "ci").unwrap(), Some(Secret::new(SECRET)));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains(SECRET), "{written}");
        let table = config::read_preferences(&path).unwrap();
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
        let entry = RegistryConfig::from_table(&table)
            .unwrap()
            .entry(&host)
            .unwrap();
        assert_eq!(entry.user.as_deref(), Some("ci"));
        assert!(entry.private && entry.plain_http);
        assert!(!registry.authorized_log().is_empty());
    }

    #[test]
    fn a_login_keeps_a_preferences_save_made_while_the_secret_is_stored() {
        let dir = Temp::new();
        let registry = MockRegistry::start(user_policy());
        let path = preferences(&dir, &registry);
        let host = registry.host();
        let store = Hooked {
            inner: SessionStore::default(),
            fail: false,
            during: || {
                // The desktop shell saves its own keys meanwhile, under the lock.
                config::update_preferences(&path, |table| {
                    table.insert("reduced_motion".into(), true.into());
                    Ok(())
                })
                .unwrap();
            },
        };
        log_in(&path, &host, "ci", &Secret::new(SECRET), &store, &|| true).unwrap();
        let table = config::read_preferences(&path).unwrap();
        assert_eq!(table["reduced_motion"].as_bool(), Some(true));
        assert_eq!(table["registries"][0]["user"].as_str(), Some("ci"));

        let failing = Hooked {
            inner: SessionStore::default(),
            fail: true,
            during: || {},
        };
        let before = std::fs::read(&path).unwrap();
        let failed = log_in(&path, &host, "ci", &Secret::new(SECRET), &failing, &|| true);
        assert!(matches!(failed, Err(Error::Credentials(_))), "{failed:?}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn test_connection_reports_the_anonymous_check_then_the_basic_login() {
        let source = |user: Option<&str>, secret: &str, host: &str| {
            let store = Arc::new(SessionStore::default());
            if let Some(user) = user {
                store.store(host, user, &Secret::new(secret)).unwrap();
            }
            HostCredentials {
                configured_user: user.map(str::to_owned),
                env: EnvCredentials::default(),
                store,
            }
        };
        let private = MockRegistry::start(user_policy());
        let host = private.host();
        let entry = private.entry(true);
        for (user, secret, login) in [
            (None, "", LoginCheck::NotSignedIn),
            (
                Some("ci"),
                SECRET,
                LoginCheck::Accepted { user: "ci".into() },
            ),
            (
                Some("ci"),
                "wrong-secret",
                LoginCheck::Rejected { user: "ci".into() },
            ),
        ] {
            let report = test_connection(&entry, &source(user, secret, &host)).unwrap();
            assert_eq!(
                report,
                ConnectionReport {
                    anonymous: ApiAccess::BasicLogin,
                    login
                }
            );
        }
        let open = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..user_policy()
        });
        let report = test_connection(&open.entry(false), &source(None, "", &open.host())).unwrap();
        assert_eq!(report.anonymous, ApiAccess::Anonymous);
        assert_eq!(report.login, LoginCheck::NotSignedIn);
        let tokens = MockRegistry::start(Policy {
            bearer_challenge: true,
            ..user_policy()
        });
        let report = test_connection(
            &tokens.entry(true),
            &source(Some("ci"), SECRET, &tokens.host()),
        )
        .unwrap();
        assert_eq!(report.anonymous, ApiAccess::TokenService);
        assert_eq!(report.login, LoginCheck::NotTried);
        assert!(tokens.authorized_log().is_empty());

        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = closed.local_addr().unwrap().to_string();
        drop(closed);
        let unreachable = RegistryEntry {
            host: address,
            private: true,
            user: None,
            plain_http: true,
        };
        let failed = test_connection(&unreachable, &Anonymous);
        assert!(matches!(failed, Err(Error::Transport(_))), "{failed:?}");
    }

    #[test]
    fn a_cancel_after_verification_stores_nothing() {
        let dir = Temp::new();
        let registry = MockRegistry::start(user_policy());
        let path = preferences(&dir, &registry);
        let host = registry.host();
        let before = std::fs::read(&path).unwrap();
        let store = SessionStore::default();
        let cancelled = log_in(&path, &host, "ci", &Secret::new(SECRET), &store, &|| false);
        assert!(matches!(cancelled, Err(Error::Cancelled)), "{cancelled:?}");
        assert!(
            !registry.authorized_log().is_empty(),
            "the login was verified first"
        );
        assert!(!store.contains(&host, "ci").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // Cancelled after the secret went in: it is taken out again.
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let second = || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let cancelled = log_in(&path, &host, "ci", &Secret::new(SECRET), &store, &second);
        assert!(matches!(cancelled, Err(Error::Cancelled)), "{cancelled:?}");
        assert!(!store.contains(&host, "ci").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// The Keychain backend's rules over an in-memory store.
    #[derive(Default)]
    struct KeychainRules(SessionStore);
    impl CredentialStore for KeychainRules {
        fn kind(&self) -> crate::credential_store::StoreKind {
            crate::credential_store::StoreKind::Keychain
        }
        fn check(&self, host: &str, user: &str, secret: &Secret) -> Result<()> {
            crate::credential_store::KeychainStore::new().check(host, user, secret)
        }
        fn read(&self, host: &str, user: &str) -> Result<Option<Secret>> {
            self.0.read(host, user)
        }
        fn store(&self, host: &str, user: &str, secret: &Secret) -> Result<()> {
            self.0.store(host, user, secret)
        }
        fn delete(&self, host: &str, user: &str) -> Result<()> {
            self.0.delete(host, user)
        }
    }

    #[test]
    fn a_secret_the_store_refuses_fails_before_any_request() {
        let dir = Temp::new();
        let registry = MockRegistry::start(user_policy());
        let path = preferences(&dir, &registry);
        let store = KeychainRules::default();
        let refused = log_in(
            &path,
            &registry.host(),
            "ci",
            &Secret::new("p\u{e4}ssword"),
            &store,
            &|| true,
        );
        assert!(matches!(refused, Err(Error::Credentials(_))), "{refused:?}");
        assert!(registry.log().is_empty(), "{:?}", registry.log());
    }

    #[test]
    fn a_registry_open_without_login_cannot_verify_credentials() {
        let dir = Temp::new();
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..user_policy()
        });
        let path = preferences(&dir, &registry);
        let host = registry.host();
        let store = Arc::new(SessionStore::default());
        let outcome = log_in(&path, &host, "ci", &Secret::new(SECRET), &*store, &|| true);
        assert_eq!(outcome.unwrap(), LoginOutcome::Unverifiable);
        let source = HostCredentials {
            configured_user: Some("ci".into()),
            env: EnvCredentials::default(),
            store: store.clone(),
        };
        let report = test_connection(&registry.entry(true), &source).unwrap();
        assert_eq!(report.login, LoginCheck::Unverifiable { user: "ci".into() });
        let closed = MockRegistry::start(user_policy());
        let path = preferences(&dir, &closed);
        let outcome = log_in(
            &path,
            &closed.host(),
            "ci",
            &Secret::new(SECRET),
            &*store,
            &|| true,
        );
        assert_eq!(outcome.unwrap(), LoginOutcome::Verified);
    }

    #[test]
    fn a_token_service_registry_refuses_the_login_without_storing_it() {
        let dir = Temp::new();
        let registry = MockRegistry::start(Policy {
            bearer_challenge: true,
            ..user_policy()
        });
        let path = preferences(&dir, &registry);
        let store = SessionStore::default();
        let refused = log_in(
            &path,
            &registry.host(),
            "ci",
            &Secret::new(SECRET),
            &store,
            &|| true,
        );
        assert!(
            matches!(refused, Err(Error::UnsupportedAuth(_))),
            "{refused:?}"
        );
        assert!(!store.contains(&registry.host(), "ci").unwrap());
    }

    #[test]
    fn log_out_keeps_the_user_and_remove_drops_entry_and_secret() {
        let dir = Temp::new();
        let registry = MockRegistry::start(user_policy());
        let path = preferences(&dir, &registry);
        let host = registry.host();
        let store = SessionStore::default();
        log_in(&path, &host, "ci", &Secret::new(SECRET), &store, &|| true).unwrap();
        assert_eq!(
            log_out(&path, &host, &store).unwrap().as_deref(),
            Some("ci")
        );
        assert!(!store.contains(&host, "ci").unwrap());
        let config = RegistryConfig::from_table(&config::read_preferences(&path).unwrap()).unwrap();
        assert_eq!(config.entry(&host).unwrap().user.as_deref(), Some("ci"));

        store.store(&host, "ci", &Secret::new(SECRET)).unwrap();
        config::update_preferences(&path, |t| config::set_default_registry(t, &host)).unwrap();
        assert!(remove(&path, &host, &store).is_err());
        assert!(store.contains(&host, "ci").unwrap());
        config::update_preferences(&path, |t| {
            config::set_default_registry(t, crate::DEFAULT_REGISTRY)
        })
        .unwrap();
        remove(&path, &host, &store).unwrap();
        assert!(!store.contains(&host, "ci").unwrap());
        let table = config::read_preferences(&path).unwrap();
        assert!(RegistryConfig::from_table(&table)
            .unwrap()
            .entry(&host)
            .is_none());
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
    }
}
