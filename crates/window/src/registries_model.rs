// SPDX-License-Identifier: AGPL-3.0-or-later
//! Preferences › Registries' model is `library_model::registries_model`,
//! re-exported here under its old path. The one test that also loads the
//! application's own preferences stays with the application.

pub(crate) use library_model::registries_model::*;

#[cfg(test)]
mod merge_tests {
    use super::*;
    use image_registry::{
        accounts,
        config::RegistryConfig,
        credential_store::{Secret, SessionStore},
        credentials::EnvCredentials,
        test_support::{MockRegistry, Policy},
    };
    use std::sync::Arc;

    #[test]
    fn the_page_the_command_line_and_the_shell_keep_each_others_keys() {
        let registry = MockRegistry::start(Policy {
            user: Some(("ci".into(), "synthetic-secret".into())),
            ..Policy::default()
        });
        let host = registry.host();
        let temp = image_store::test_support::TempDir::new("registries-merge");
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
        page.wait();
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
