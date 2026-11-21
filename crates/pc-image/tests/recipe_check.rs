// SPDX-License-Identifier: AGPL-3.0-or-later
//! The static checks of recipes (recipes.md §6.11) with the `ibm-pc`
//! provider: the corpus of recipes.md §4.6, one fixture per rule with its
//! exact findings, and the JSON Schema written from the providers.

use image_store::metadata::{self, Metadata};
use image_store::needs::{Form, Kind, Vocabulary};
use pc_image::recipe::PcRecipes;
use recipe::provider::{CatalogueEntry, FirmwareProfile, MediaProfile, Provider, VolumeProfile};
use recipe::Providers;
use std::path::{Path, PathBuf};

const CORPUS: &str = "../../backlog/docs/design/recipe-corpus";
const TEMPLATE_RECIPES: &str = "data/recipes";
const SCHEMA: &str = "../../backlog/docs/design/recipe-schema-1.0.json";
const FIXTURES: &str = "tests/recipe-fixtures";

fn check(paths: &[&str], providers: &[&dyn Provider]) -> recipe::Report {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    recipe::check_paths(&paths, Providers(providers)).unwrap()
}

/// The findings of a fixture, with the fixture folder left out of the file
/// names.
fn findings(fixture: &str) -> String {
    let report = check(&[&format!("{FIXTURES}/{fixture}")], &[&PcRecipes]);
    report
        .findings
        .iter()
        .map(|f| {
            let line = f.to_string();
            format!(
                "{}\n",
                line.strip_prefix(&format!("{FIXTURES}/")).unwrap_or(&line)
            )
        })
        .collect()
}

macro_rules! fixture {
    ($($test:ident: $fixture:literal,)*) => {$(
        #[test]
        fn $test() {
            let expected = std::fs::read_to_string(format!(
                "{FIXTURES}/{}.expected",
                $fixture.trim_end_matches('/')
            ))
            .unwrap();
            assert_eq!(findings($fixture), expected);
        }
    )*};
}

fixture! {
    rule_1_a_file_that_is_not_toml: "rule01-not-toml.media.toml",
    rule_1_unknown_and_missing_keys: "rule01-keys.media.toml",
    rule_1_toml_1_1_forms: "rule01-toml-1.1.media.toml",
    rule_1_types_and_values: "rule01-values.media.toml",
    rule_1_empty_credits_and_genres: "rule01-empty-credits.media.toml",
    rule_2_identity_and_kind: "rule02-kind.media.toml",
    rule_2_no_identity_table: "rule02-no-identity.media.toml",
    rule_3_names_and_references: "rule03-references.media.toml",
    rule_3_work_files: "work",
    rule_4_unused_inputs_and_steps: "rule04-unused.media.toml",
    rule_6_open_set: "rule06-license.media.toml",
    rule_6_expression_and_extras: "rule06-expression.media.toml",
    rule_6_user_supplied_name: "rule06-private.media.toml",
    rule_6_firmware_and_catalogue: "rule06-firmware.firmware.toml",
    rule_7_vocabularies: "rule07-vocabulary.media.toml",
    rule_7_firmware_vocabularies: "rule07-firmware.firmware.toml",
    rule_7_platform_without_provider: "rule07-platform.media.toml",
    rule_7_retired_platform_id: "rule07-retired.media.toml",
    rule_8_digest_without_lock: "rule08-lock.media.toml",
    rule_9_limits: "rule09-limits.media.toml",
    rule_10_size_the_format_refuses: "rule10-size.media.toml",
    rule_10_fat_format_against_medium: "rule10-fat-format.media.toml",
    rule_10_names_a_volume_refuses: "rule10-names.media.toml",
    rule_10_rom_against_catalogue: "rule10-rom.firmware.toml",
    rule_6_user_supplied_rom_known_by_its_chips: "rule06-by-hash.firmware.toml",
    rule_7_retired_id_stops_the_provider_rules: "rule07-retired-firmware.firmware.toml",
    rule_10_label_is_never_changed: "rule10-label.media.toml",
}

/// A recipe written to a temporary folder, for fixtures too large or with
/// bytes too fragile to keep as files.
fn generated(name: &str, text: &str) -> recipe::Report {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("recipe-generated");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    check(&[path.to_str().unwrap()], &[&PcRecipes])
}

fn head(name: &str) -> String {
    format!(
        "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/{name}\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n[title]\ntitle = \"Demo\"\n"
    )
}

fn input(id: &str) -> String {
    format!(
        "\n[input.{id}]\nname = \"{id}.img\"\nsize = 1474560\nsha256 = \"{}\"\n",
        "a".repeat(64)
    )
}

fn medium(id: &str, from: &str) -> String {
    format!(
        "\n[[medium]]\nid = \"{id}\"\nlabel = \"{id}\"\nformat = \"1.44M\"\nfrom = \"{from}\"\n"
    )
}

fn limits(report: &recipe::Report) -> Vec<String> {
    report
        .findings
        .iter()
        .filter(|f| f.rule == recipe::Rule::Limit)
        .map(|f| format!("{}:{} {}: {}", f.line, f.column, f.path, f.message))
        .collect()
}

/// Rule 9: more than 4096 inputs (no output uses them; only the count is
/// asserted here).
#[test]
fn rule_9_more_inputs_than_allowed() {
    let mut text = head("inputs");
    for n in 0..=recipe::MAX_INPUTS {
        text.push_str(&input(&format!("i{n}")));
    }
    let report = generated("inputs.media.toml", &text);
    assert_eq!(
        limits(&report),
        ["3:1 media: the recipe has 4097 inputs; at most 4096 are allowed"]
    );
}

/// Rule 9: more than 256 outputs.
#[test]
fn rule_9_more_outputs_than_allowed() {
    let mut text = head("outputs");
    text.push_str(&input("disk"));
    for n in 0..=recipe::MAX_OUTPUTS {
        text.push_str(&medium(&format!("disk-{n}"), "disk"));
    }
    let report = generated("outputs.media.toml", &text);
    assert_eq!(
        limits(&report),
        ["3:1 media: the recipe has 257 outputs; at most 256 are allowed"]
    );
    assert_eq!(report.findings.len(), 1, "{}", report.render());
}

/// A recipe saved with CR LF line ends passes, a multi-line string
/// included, whose CR LF is one line break; a CR anywhere else is refused.
#[test]
fn rule_1_crlf_line_ends_are_line_breaks_and_a_bare_cr_is_not() {
    let mut text = head("crlf");
    text.push_str("description = \"\"\"\nFirst line.\nSecond line.\"\"\"\n");
    text.push_str(&input("disk"));
    text.push_str(&medium("disk-1", "disk"));
    let crlf = text.replace('\n', "\r\n");
    let report = generated("crlf.media.toml", &crlf);
    assert!(report.is_clean(), "{}", report.render());
    let bare = crlf.replace("First line.", "First\\rline.");
    let report = generated("cr.media.toml", &bare);
    let found: Vec<String> = report
        .findings
        .iter()
        .map(|f| format!("{}:{} {}", f.line, f.column, f.path))
        .collect();
    assert_eq!(found, ["10:15 title.description"], "{}", report.render());
}

/// Every fixture file is named by a test above, so none is left unchecked.
#[test]
fn every_fixture_has_its_expected_findings() {
    let files = recipe::files(&[PathBuf::from(FIXTURES)]).unwrap();
    for file in files {
        let relative = file.strip_prefix(FIXTURES).unwrap();
        let fixture = match relative.parent() {
            Some(parent) if parent != Path::new("") => parent.to_path_buf(),
            _ => relative.to_path_buf(),
        };
        let expected = PathBuf::from(format!("{FIXTURES}/{}.expected", fixture.display()));
        assert!(expected.exists(), "{}", expected.display());
        let source = include_str!("recipe_check.rs");
        assert!(
            source.contains(&format!("\"{}\"", fixture.display())),
            "{} has no test",
            fixture.display()
        );
    }
}

/// A lock folder alone no longer satisfies rule 8: the lock must hold the
/// manifest the digest names (recipes §3.8, point 1). A lock that agrees
/// with its recipe passes; `recipe_lock` tests that with a real pin.
#[test]
fn a_digest_with_an_empty_lock_is_reported() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("recipe-lock");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("x.media.toml.lock")).unwrap();
    std::fs::copy(
        format!("{FIXTURES}/rule08-lock.media.toml"),
        dir.join("x.media.toml"),
    )
    .unwrap();
    let report = check(&[dir.to_str().unwrap()], &[&PcRecipes]);
    let findings: Vec<String> = report.findings.iter().map(|f| f.message.clone()).collect();
    assert_eq!(findings.len(), 1, "{}", report.render());
    assert!(
        findings[0].ends_with("x.media.toml.lock: no sha256 folder"),
        "{}",
        report.render()
    );
    assert_eq!(report.findings[0].rule, recipe::Rule::Lock);
}

/// With the `ibm-pc` provider, every recipe of the corpus passes except the
/// five of the two platforms this build has no provider for, each refused
/// once at its platform (rule 7); nothing behind the platform is reported.
#[test]
fn the_corpus_passes_on_ibm_pc_and_names_the_platforms_it_lacks() {
    let report = check(&[CORPUS, TEMPLATE_RECIPES], &[&PcRecipes]);
    let lines: Vec<String> = report
        .findings
        .iter()
        .map(|f| {
            format!(
                "{}:{}:{} {} {} {}",
                Path::new(&f.file).file_name().unwrap().to_str().unwrap(),
                f.line,
                f.column,
                f.rule,
                f.path,
                f.message
            )
        })
        .collect();
    let refused = |file: &str, line: usize, platform: &str| {
        format!("{file}:{line}:12 6.11.7 media.platform no provider of platform `{platform}` in this build; it has ibm-pc")
    };
    assert_eq!(
        lines,
        [
            refused("amdemo.media.toml", 9, "amiga"),
            refused("contra-europe.media.toml", 10, "nes"),
            refused("contra-japan.media.toml", 10, "nes"),
            refused("contra-usa-rev1.media.toml", 10, "nes"),
            refused("contra-usa.media.toml", 10, "nes"),
        ]
    );
    assert_eq!(report.files.len(), 20, "17 recipes and 3 work files");
}

/// Rule 5: Windows 3.0's manual is the one input only an attachment uses.
#[test]
fn the_split_names_the_inputs_only_an_attachment_uses() {
    let report = check(
        &[&format!("{CORPUS}/windows-3.0/windows-3.0-1.2m.media.toml")],
        &[&PcRecipes],
    );
    let split = report.files[0].split.as_ref().unwrap();
    assert_eq!(
        split.required,
        ["disk1", "disk2", "disk3", "disk4", "disk5"]
    );
    assert_eq!(split.optional, ["users-guide"]);
    assert!(report.render().contains(
        "required inputs: disk1, disk2, disk3, disk4, disk5; optional inputs: users-guide"
    ));
}

/// A provider for a platform this build does not have, with the words the
/// corpus marks ASSUMED for it.
struct Future {
    platform: &'static str,
    formats: &'static [&'static str],
    /// The exact size of every format, when it has one.
    exact: Option<u64>,
    software: &'static [&'static str],
    kinds: &'static [Kind],
}

impl Provider for Future {
    fn platform(&self) -> &str {
        self.platform
    }
    fn media(&self) -> &dyn MediaProfile {
        self
    }
    fn firmware(&self) -> &dyn FirmwareProfile {
        self
    }
    fn needs(&self) -> Option<&dyn Vocabulary> {
        Some(self)
    }
    fn software_platforms(&self) -> &[&str] {
        self.software
    }
    fn work_id_catalogues(&self) -> &[&str] {
        &["wikidata"]
    }
}

impl Vocabulary for Future {
    fn platform(&self) -> &str {
        self.platform
    }
    fn kinds(&self) -> &[Kind] {
        self.kinds
    }
}

impl MediaProfile for Future {
    fn formats(&self) -> &[&str] {
        self.formats
    }
    fn size_fault(&self, format: &str, size: u64) -> Option<String> {
        let exact = self.exact?;
        (size != exact).then(|| format!("a {format} medium is exactly {exact} bytes"))
    }
    fn volume_profile(&self, _format: &str) -> Option<&dyn VolumeProfile> {
        None
    }
    fn dos_name_fault(&self, _name: &str) -> Option<String> {
        Some("this platform has no DOS names".into())
    }
}

impl FirmwareProfile for Future {
    fn roles(&self) -> &[&str] {
        &[]
    }
    fn lanes(&self) -> &[&str] {
        &[]
    }
    fn boards(&self) -> &[&str] {
        &[]
    }
    fn machine_classes(&self) -> &[&str] {
        &[]
    }
    fn entry(&self, _id: &str) -> Option<CatalogueEntry> {
        None
    }
    fn identify(&self, _sha256: &str, _size: u64) -> Option<(String, CatalogueEntry)> {
        None
    }
}

/// The NES as the corpus assumes it.
fn nes() -> Future {
    Future {
        platform: "nes",
        formats: &["ines"],
        exact: None,
        software: &["NES"],
        kinds: &[Kind {
            name: "tv_system",
            form: Form::Choice(&["ntsc", "pal"]),
        }],
    }
}

/// The whole corpus passes once the two future platforms have providers:
/// the engine's part of the format needs nothing of either (recipes §10).
#[test]
fn the_whole_corpus_passes_with_a_provider_for_every_platform() {
    let nes = nes();
    let amiga = Future {
        platform: "amiga",
        formats: &["880K"],
        exact: Some(901_120),
        software: &["AmigaOS"],
        kinds: &[
            Kind {
                name: "chipset",
                form: Form::Choice(&["ocs", "ecs", "aga"]),
            },
            Kind {
                name: "kickstart",
                form: Form::Ordered(&["1.2", "1.3", "2.0", "3.0", "3.1"]),
            },
            Kind {
                name: "memory",
                form: Form::Quantity,
            },
        ],
    };
    let report = check(&[CORPUS, TEMPLATE_RECIPES], &[&PcRecipes, &nes, &amiga]);
    assert!(report.is_clean(), "{}", report.render());
    assert!(report.render().ends_with("20 files checked, 0 findings\n"));
}

/// The JSON Schema in the design folder is the one the key tables and the
/// providers write. Regenerate it with
/// `UNEMUPC_WRITE_RECIPE_SCHEMA=1 cargo test -p pc-image --test recipe_check`.
#[test]
fn the_published_schema_is_the_generated_one() {
    let generated = recipe::schema::render(&[&PcRecipes]);
    if std::env::var_os("UNEMUPC_WRITE_RECIPE_SCHEMA").is_some() {
        std::fs::write(SCHEMA, &generated).unwrap();
    }
    let published = std::fs::read_to_string(SCHEMA).unwrap();
    assert!(
        published == generated,
        "{SCHEMA} is not the generated schema; regenerate it with UNEMUPC_WRITE_RECIPE_SCHEMA=1"
    );
}

/// The first round's full title record of a corpus recipe: the `title`,
/// `needs` and `ids` blocks of its `.hcl` copy, which the corpus keeps as
/// that round wrote it, before the work files (corpus README, second
/// round). The blocks are flat `key = value` lines, which read as TOML.
fn first_round(hcl: &str) -> (metadata::Title, Vec<String>) {
    let text = std::fs::read_to_string(format!("{CORPUS}/{hcl}")).unwrap();
    let mut toml = String::new();
    let mut inside = false;
    for line in text.lines() {
        if let Some(block) = ["title", "needs", "ids"]
            .into_iter()
            .find(|b| line == format!("{b} {{"))
        {
            toml.push_str(&format!("[{block}]\n"));
            inside = true;
        } else if line == "}" {
            inside = false;
        } else if inside {
            toml.push_str(line);
            toml.push('\n');
        }
    }
    let table: toml::Table = toml.parse().unwrap();
    let title = table["title"].as_table().unwrap();
    let text = |key: &str| title.get(key).map(|v| v.as_str().unwrap().to_owned());
    let list = |key: &str| -> Vec<String> {
        title.get(key).map_or_else(Vec::new, |v| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect()
        })
    };
    let known = [
        "title",
        "series",
        "version",
        "edition",
        "year",
        "publishers",
        "developers",
        "genres",
        "software_platform",
        "regions",
        "players",
        "labels",
        "description",
    ];
    for key in title.keys() {
        assert!(known.contains(&key.as_str()), "{hcl}: unmapped field {key}");
    }
    let values = |table: Option<&toml::Value>| {
        table
            .map(|t| t.as_table().unwrap().clone())
            .unwrap_or_default()
    };
    let record = metadata::Title {
        title: text("title").unwrap(),
        series: text("series"),
        version: text("version"),
        edition: text("edition"),
        release_year: title.get("year").map(|y| y.as_integer().unwrap()),
        publishers: list("publishers"),
        developers: list("developers"),
        genres: list("genres"),
        platform: text("software_platform"),
        regions: list("regions"),
        players: title.get("players").map(|p| metadata::Players {
            min: p["min"].as_integer().unwrap() as u32,
            max: p["max"].as_integer().unwrap() as u32,
        }),
        description: text("description"),
        needs: values(table.get("needs"))
            .into_iter()
            .map(|(kind, v)| {
                let values = match v {
                    toml::Value::String(one) => vec![one],
                    v => v
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap().to_owned())
                        .collect(),
                };
                (kind, values)
            })
            .collect(),
        work_ids: values(table.get("ids"))
            .into_iter()
            .map(|(catalogue, id)| (catalogue, id.as_str().unwrap().to_owned()))
            .collect(),
        ..metadata::Title::default()
    };
    (record, list("labels"))
}

fn recipe_path(relative: &str) -> PathBuf {
    let root = if relative.starts_with("freedos-1.4-floppy/") {
        TEMPLATE_RECIPES
    } else {
        CORPUS
    };
    Path::new(root).join(relative)
}

fn merged(recipe: &str, providers: &[&dyn Provider]) -> recipe::merge::Merged {
    let path = recipe_path(recipe);
    match recipe::merge_path(&path, Providers(providers)).unwrap() {
        recipe::Shown::Merged(merged) => *merged,
        other => panic!("{recipe}: {other:?}"),
    }
}

/// The work file's synopsis, read as written.
fn synopsis(work: &str) -> String {
    let text = std::fs::read_to_string(recipe_path(work)).unwrap();
    let table: toml::Table = text.parse().unwrap();
    table["title"]["synopsis"].as_str().unwrap().to_owned()
}

/// Each recipe of the three corpus folders with a work file, merged with
/// it, gives the title record its first-round copy stated in full, field by
/// field, with the real recipes' cited corrections, and its labels as a set.
/// What the merge adds is the work's own
/// fields of metadata schema 2, the work id and the synopsis, and the
/// record's id, which is the last component of the recipe's `name`
/// (recipes §6.4). The two releases added with the work files have no
/// first round.
#[test]
fn the_corpus_work_files_merge_into_the_first_rounds_full_records() {
    let nes = nes();
    let providers: [&dyn Provider; 2] = [&PcRecipes, &nes];
    let cases = [
        ("freedos-1.4-floppy/freedos-1.4", "freedos"),
        ("freedos-1.4-floppy/freedos-1.4-1.2m", "freedos"),
        ("freedos-1.4-floppy/freedos-1.4-720k", "freedos"),
        ("kenslab-2.01/kenslab-2.01", "kenslab"),
        ("contra-nes/contra-usa", "contra"),
        ("contra-nes/contra-japan", "contra"),
        ("contra-nes/contra-europe", "contra"),
    ];
    for (recipe, work) in cases {
        let folder = recipe.split('/').next().unwrap();
        let (mut expected, labels) = first_round(&format!("{recipe}.media.hcl"));
        expected.work_id = Some(work.into());
        expected.synopsis = Some(synopsis(&format!("{folder}/{work}.work.toml")));
        // The HCL snapshots retain assumptions the real work records corrected.
        match work {
            "freedos" => {
                expected.needs.remove("cpu");
                expected.needs.remove("memory");
                expected.requirements = Some("This edition should run on any hardware that can run FreeDOS and has EGA or better graphics".into());
                expected
                    .work_ids
                    .insert("wikidata".into(), "Q473751".into());
            }
            "kenslab" => {
                expected.needs.insert("memory".into(), vec!["512K".into()]);
                expected.needs.insert(
                    "sound".into(),
                    vec!["sb".into(), "adlib".into(), "none".into()],
                );
                expected.needs.insert(
                    "input".into(),
                    vec!["joystick".into(), "mouse".into(), "none".into()],
                );
                expected.requirements = Some("IBM AT-Compatible computer (386 or 486 highly recommended); VGA Graphics; 512K of total memory (at least 365K available); About 1 megabyte of hard disk space".into());
                expected.work_ids = [
                    ("wikidata".into(), "Q3814453".into()),
                    ("mobygames".into(), "kens-labyrinth".into()),
                    ("igdb".into(), "kens-labyrinth".into()),
                ]
                .into();
            }
            _ => {}
        }
        let merged = merged(&format!("{recipe}.media.toml"), &providers);
        let title = merged.title();
        // The record's id, read from the recipe's own `name`.
        let text = std::fs::read_to_string(recipe_path(&format!("{recipe}.media.toml"))).unwrap();
        let table: toml::Table = text.parse().unwrap();
        let name = table["media"]["name"].as_str().unwrap();
        assert_eq!(title.id, name.rsplit('/').next().unwrap(), "{recipe}");
        assert_eq!(title.id, work, "{recipe}");
        assert_eq!(
            metadata::Title {
                id: String::new(),
                ..title
            },
            expected,
            "{recipe}"
        );
        let mut merged_labels = merged.metadata().labels;
        let mut labels = labels;
        merged_labels.sort();
        labels.sort();
        assert_eq!(merged_labels, labels, "{recipe}");
    }
    // The union puts the work file's labels first: the full version's own
    // label follows the work's two, where the first round had it second.
    assert_eq!(
        merged("kenslab-2.01/kenslab-2.01.media.toml", &providers)
            .metadata()
            .labels,
        ["game", "first-person-shooter", "freeware"]
    );
    assert_eq!(
        first_round("kenslab-2.01/kenslab-2.01.media.hcl").1,
        ["game", "freeware", "first-person-shooter"]
    );
}

/// Every recipe of the three folders, the two new releases included, maps
/// onto metadata schema 2 that encodes and decodes to the same record, and
/// encodes again to the same bytes.
#[test]
fn the_merged_corpus_records_round_trip_through_schema_2_metadata() {
    let nes = nes();
    let providers: [&dyn Provider; 2] = [&PcRecipes, &nes];
    for recipe in [
        "freedos-1.4-floppy/freedos-1.4",
        "freedos-1.4-floppy/freedos-1.4-1.2m",
        "freedos-1.4-floppy/freedos-1.4-720k",
        "kenslab-2.01/kenslab-2.01",
        "kenslab-2.01/kenslab-1.0-shareware",
        "contra-nes/contra-usa",
        "contra-nes/contra-usa-rev1",
        "contra-nes/contra-japan",
        "contra-nes/contra-europe",
    ] {
        let metadata = merged(&format!("{recipe}.media.toml"), &providers).metadata();
        let bytes = metadata.encode(&[]).unwrap();
        let decoded = Metadata::decode(&bytes, &[]).unwrap();
        assert_eq!(decoded.schema(), 2, "{recipe}");
        assert_eq!(
            (&decoded.platform, &decoded.labels, &decoded.titles),
            (&metadata.platform, &metadata.labels, &metadata.titles),
            "{recipe}"
        );
        assert_eq!(decoded.encode(&[]).unwrap(), bytes, "{recipe}");
    }
}
