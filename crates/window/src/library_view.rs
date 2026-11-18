// SPDX-License-Identifier: AGPL-3.0-or-later
//! The machine library's frame is `pc_ui::library_view`, re-exported here
//! under the path the shell names it by, with the frame's fixtures the
//! shell's tests use. Two tests stay with the application: the store path's
//! round trip through the application's preferences, and the refresh key of
//! Remotes, which reads the shell's hotkeys.

pub(crate) use pc_ui::library_view::*;

/// The Firmware collection, with the fixtures the shell's tests build a
/// library and wait on it with.
pub(crate) mod firmware {
    pub(crate) use pc_ui::library_view::firmware::*;

    #[cfg(test)]
    pub(crate) mod tests {
        pub(crate) use pc_library::library_test_support::synthetic_catalogue;
        pub(crate) use pc_ui::library_view::firmware::test_support::{
            poll_until, publish_open_package, warm,
        };
    }
}

/// The Installation media collection, with the library of sets the shell's
/// tests insert from.
pub(crate) mod install_media {
    pub(crate) use pc_ui::library_view::install_media::*;

    #[cfg(test)]
    pub(crate) mod tests {
        pub(crate) use pc_ui::library_view::install_media::test_support::{library, warm};
    }
}

/// The Machines collection, with the machine row the shell's tests build.
pub(crate) mod machines {
    pub(crate) use pc_ui::library_view::machines::*;

    #[cfg(test)]
    pub(crate) mod tests {
        pub(crate) use pc_ui::library_view::machines::test_support::row;
    }
}

/// The Remotes group, with the test of its refresh key.
#[cfg(test)]
pub(crate) mod remotes {
    pub(crate) use pc_ui::library_view::remotes::*;

    mod tests {
        // What the test took from the Remotes tests' own imports.
        use crate::remote_browse::{BrowseRequest, State};
        use image_registry::DEFAULT_REGISTRY;
        use pc_ui::library_view::remotes::test_support::{job_of, unemu, Rig};

        /// F5 (View › Refresh remote) and the toolbar's Refresh list the shown
        /// registry again; neither applies while the library shows a collection.
        #[test]
        fn refresh_lists_the_shown_registry_again() {
            let mut rig = Rig::new();
            rig.view.refresh_remote();
            assert!(rig.sent().is_empty(), "no registry is shown");
            rig.listed();
            rig.show(unemu(None));
            rig.view.refresh_remote();
            let requests = rig.browsing();
            assert!(matches!(
                requests.as_slice(),
                [BrowseRequest::List { next: None, .. }]
            ));
            assert_eq!(job_of(&requests[0]).host, DEFAULT_REGISTRY);
            assert_eq!(rig.registry(DEFAULT_REGISTRY).state, State::Listing);
            // F5 is View › Refresh remote while the library shows, never the guest's.
            use crate::shell_model::{hotkey, Command, State as Run};
            use winit::keyboard::KeyCode;
            assert_eq!(
                hotkey(KeyCode::F5, [false; 8], Run::Stopped, false, false),
                Some(Command::RefreshRemote)
            );
            assert_eq!(
                hotkey(KeyCode::F5, [false; 8], Run::Stopped, false, true),
                Some(Command::RefreshRemote)
            );
            assert_eq!(
                hotkey(KeyCode::F5, [false; 8], Run::Running, true, false),
                None
            );
            assert_eq!(
                hotkey(KeyCode::F5, [false; 8], Run::Running, false, false),
                None
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // What the test took from the frame's own imports.
    use library_model::listing::test_support::last_run_utc;

    #[test]
    fn platform_default_store_paths_and_preference_round_trip() {
        assert_eq!(last_run_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(last_run_utc(951782400), "2000-02-29 00:00 UTC");
        assert_eq!(
            default_store_path().unwrap(),
            image_store::location::default_store_root().unwrap()
        );
        let mut preferences = crate::preferences::Preferences::default();
        preferences.store_root = Some("/synthetic-store".into());
        let restored: crate::preferences::Preferences =
            toml::from_str(&toml::to_string(&preferences).unwrap()).unwrap();
        assert_eq!(restored, preferences);
    }
}
