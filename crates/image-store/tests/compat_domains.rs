// SPDX-License-Identifier: AGPL-3.0-or-later
//! Independent release domains and the frozen packaging baseline; machine-images §5.6.
use image_store::{
    compat::{
        self, AppVersion as V, Compatibility, Component, Levels, Releases, Requirement,
        Requirements,
    },
    media_type, Descriptor, Digest, Manifest,
};

fn v(patch: u64) -> V {
    V::new(0, 0, patch)
}
fn levels(core: u64, frontend: u64) -> Levels {
    Levels::new(Some((v(core), v(1))), v(frontend), v(1))
}
fn manifest() -> Manifest {
    Manifest::new(
        "synthetic",
        Descriptor::new(media_type::CONFIG, Digest::of(b"config"), 6),
        vec![Descriptor::new(
            media_type::METADATA,
            Digest::of(b"metadata"),
            8,
        )],
    )
    .unwrap()
}
fn required(core: Option<&str>, frontend: Option<&str>) -> Requirements {
    Requirements {
        core: Requirement::of_text(core),
        frontend: Requirement::of_text(frontend),
    }
}

#[test]
fn unequal_versions_never_borrow_the_other_components_level() {
    let core = required(Some("0.0.5"), None);
    assert_eq!(
        core.unmet_reason(levels(3, 9)).as_deref(),
        Some("Needs unemupc 0.0.5 or newer")
    );
    assert!(core.check("core", levels(3, 9)).is_err());
    assert!(core.check("core", levels(9, 3)).is_ok());
    let frontend = required(None, Some("0.0.5"));
    assert_eq!(
        frontend.unmet_reason(levels(9, 3)).as_deref(),
        Some("Needs front end 0.0.5 or newer")
    );
    assert!(frontend.check("frontend", levels(9, 3)).is_err());
    assert!(frontend.check("frontend", levels(3, 9)).is_ok());
    assert!(core
        .check(
            "missing core",
            Levels {
                core: None,
                frontend: v(99)
            }
        )
        .is_err());
}

#[test]
fn development_maxima_stay_with_their_owner() {
    let level = Levels::new(Some((v(2), v(7))), v(3), v(5));
    assert_eq!(level.core, Some(v(7)));
    assert_eq!(level.frontend, v(5));
    let level = Levels::new(Some((v(5), v(3))), v(2), v(9));
    assert_eq!(level.core, Some(v(5)));
    assert_eq!(level.frontend, v(9));
}

#[test]
fn markers_are_lenient_independently_absent_or_malformed() {
    let level = levels(3, 9);
    for key in [compat::MIN_APP_VERSION, compat::MIN_FRONTEND_VERSION] {
        for value in [
            serde_json::json!("soon"),
            serde_json::json!(4),
            serde_json::json!(null),
        ] {
            let bytes = serde_json::to_vec(
                &serde_json::json!({"future": true, "annotations": {key: value}}),
            )
            .unwrap();
            let read = Requirements::read(&bytes);
            assert!(read.check("future", level).is_err(), "{key}");
            assert!(Manifest::decode(&bytes).is_err());
            let other = if key == compat::MIN_APP_VERSION {
                read.frontend
            } else {
                read.core
            };
            assert_eq!(other, Requirement::None);
        }
    }
    let absent = Requirements::read(br#"{"annotations":{}}"#);
    assert!(absent.check("absent", level).is_ok());
    let one = Requirements::read(
        br#"{"annotations":{"unemu.min-app-version":"0.0.2","unemu.min-frontend-version":"bad"}}"#,
    );
    assert_eq!(one.core, Requirement::Release(v(2)));
    assert_eq!(one.frontend, Requirement::Invalid("bad".into()));
}

#[test]
fn stamping_frontend_features_keeps_the_core_marker_and_frozen_bytes() {
    assert_eq!(compat::EXTRACTION_BASELINE, v(3));
    let mut historical = manifest();
    historical
        .annotations
        .insert(compat::MIN_APP_VERSION.into(), "0.0.3".into());
    let bytes = historical.encode().unwrap();
    let digest = Digest::of(&bytes);
    let mut rebuilt = manifest();
    compat::stamp(
        &mut rebuilt,
        Releases {
            core: v(3),
            frontend: v(3),
        },
        None,
        levels(3, 9),
    )
    .unwrap();
    assert_eq!(rebuilt.encode().unwrap(), bytes);
    assert_eq!(Digest::of(&rebuilt.encode().unwrap()), digest);
    compat::floor(
        &mut rebuilt,
        Releases {
            core: v(3),
            frontend: v(7),
        },
        levels(3, 9),
    )
    .unwrap();
    assert_eq!(
        Requirements::of(&rebuilt),
        required(Some("0.0.3"), Some("0.0.7"))
    );
    let mut refused = manifest();
    assert!(compat::stamp(
        &mut refused,
        Releases {
            core: v(3),
            frontend: v(7)
        },
        None,
        levels(9, 3)
    )
    .is_err());
    assert!(compat::raised(v(5), None, Component::Core, Some(v(3))).is_err());
}

#[test]
fn inherited_floors_remain_independent_and_explicit_core_floors_survive() {
    let mut source = manifest();
    compat::stamp(
        &mut source,
        Releases {
            core: v(3),
            frontend: v(7),
        },
        Some(v(5)),
        levels(9, 9),
    )
    .unwrap();
    let mut derived = manifest();
    compat::stamp(
        &mut derived,
        Releases {
            core: v(3),
            frontend: v(8),
        },
        None,
        levels(9, 9),
    )
    .unwrap();
    compat::carry(&mut derived, &source, levels(9, 9)).unwrap();
    assert!(compat::stamp(
        &mut derived.clone(),
        Releases {
            core: v(3),
            frontend: v(1)
        },
        Some(v(4)),
        levels(9, 9)
    )
    .is_err());
    assert_eq!(
        Requirements::of(&derived),
        required(Some("0.0.5"), Some("0.0.8"))
    );
    assert!(compat::stamp(
        &mut manifest(),
        Releases {
            core: v(5),
            frontend: v(1)
        },
        Some(v(3)),
        levels(9, 9)
    )
    .is_err());
}

#[test]
fn selected_core_and_frontend_are_injected_without_store_package_versions() {
    let runtime = Compatibility::new(v(9), v(3)).with_core("synthetic", v(2), v(3));
    assert_eq!(runtime.levels("synthetic"), levels(3, 9));
    assert_eq!(runtime.levels("unknown").core, None);
    let bytes = br#"{"annotations":{"unemu.platform":"unknown","unemu.min-app-version":"0.0.2"}}"#;
    assert!(runtime.check(bytes, "unknown").is_err());
    assert!(runtime
        .check(
            br#"{"annotations":{"unemu.platform":"unknown"}}"#,
            "unmarked unknown"
        )
        .is_err());
    let runtime = Compatibility::new(v(2), v(3)).with_core("synthetic", v(9), v(3));
    assert_eq!(runtime.levels("synthetic"), levels(9, 3));
}

#[test]
fn catalogue_metadata_raises_neither_runnable_requirement() {
    let mut image = manifest();
    let config = b"schema = 1\nplatform = 'synthetic'\n";
    for metadata in [
        br#"{"schema":1,"platform":"synthetic","titles":[]}"#.as_slice(),
        br#"{"schema":2,"platform":"synthetic","titles":[{"id":"work","title":"Work","topics":["puzzle"]}]}"#.as_slice(),
    ] {
        image.layers[0] = Descriptor::new(media_type::METADATA, Digest::of(metadata), metadata.len() as u64);
        let frontend = compat::shared_frontend_release(&image, config).unwrap();
        assert_eq!(frontend, v(1));
        compat::stamp(&mut image, Releases { core: v(1), frontend }, None, levels(9, 9)).unwrap();
        assert_eq!(Requirements::of(&image), required(None, None));
    }
}
