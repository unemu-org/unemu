// SPDX-License-Identifier: AGPL-3.0-or-later
//! A Mediafile and its work file merged into one description record, field
//! by field as recipes §6.16 rules ("Which field lives where"), and that
//! record as the title metadata a build writes (metadata schema 2,
//! machine-images §2.1).
//!
//! The rules, in short: a single value is the recipe's when it states one,
//! else the work file's; a list of credits or genres is the recipe's whole
//! list when it states one, else the work file's, never a mix of both;
//! labels are the union, the work file's first; `[ids]` and `[needs]` merge
//! per key, the recipe's value winning. Release fields come only from the
//! recipe and the synopsis only from the work file; the description is the
//! recipe's, or the synopsis when the recipe states none.
//!
//! A recipe's empty list of credits or genres is refused by the checks: it
//! could only mean "none", which metadata cannot hold, as it writes no empty
//! list and absent means unknown there. A recipe leaves the key out instead,
//! and the work file's list stands.
//!
//! Nothing here judges a value; the checks do (recipes §6.11), and
//! [`crate::merge_path`] merges only a recipe that passes them.

use crate::model::{self, At, Need, Players, Recipe, WorkFile, WorkId};
use image_store::metadata::{self, Metadata};

/// Which file a merged value was written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Recipe,
    Work,
}

/// A merged value and the file it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub from: Origin,
}

/// The merged description of one Mediafile: its identity, its title record
/// with the work file's shared fields, its needs and its ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    /// The output's repository (`unemu/media/ibm-pc/freedos`).
    pub name: String,
    /// The platform id as written.
    pub platform: String,
    pub release: Option<String>,
    pub revision: i64,
    /// The locked output's manifest digest; `None` for a draft.
    pub digest: Option<String>,
    /// The work the recipe names, the last component of `name`.
    pub work: Option<String>,
    /// The work file's name, `<id>.work.toml`, when the recipe names one.
    pub work_file: Option<String>,
    pub title: Sourced<String>,
    pub sort_title: Option<Sourced<String>>,
    pub series: Option<Sourced<String>>,
    pub software_platform: Option<Sourced<String>>,
    pub players: Option<Sourced<Players>>,
    pub publishers: Option<Sourced<Vec<String>>>,
    pub developers: Option<Sourced<Vec<String>>>,
    pub genres: Option<Sourced<Vec<String>>>,
    /// The work file's labels, then the recipe's that it lacks; each once.
    pub labels: Vec<Sourced<String>>,
    /// What the work is: the work file's own field.
    pub synopsis: Option<String>,
    /// What this release is: the recipe's, or the synopsis when it states
    /// none.
    pub description: Option<Sourced<String>>,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub year: Option<i64>,
    pub languages: Vec<String>,
    pub regions: Vec<String>,
    pub source_media: Option<String>,
    pub requirements: Option<String>,
    /// `[ids]` by catalogue; a catalogue both state takes the recipe's id.
    pub ids: Vec<(String, Sourced<String>)>,
    /// `[needs]` by kind, the same way; each kind's alternatives best first.
    pub needs: Vec<(String, Sourced<Vec<String>>)>,
}

fn value<T: Clone>(at: &Option<At<T>>) -> Option<T> {
    at.as_ref().map(|a| a.value.clone())
}

fn strings(list: &[At<String>]) -> Vec<String> {
    list.iter().map(|a| a.value.clone()).collect()
}

/// The nearer value wins: the recipe's, else the work file's.
fn single<T: Clone>(mine: &Option<At<T>>, work: Option<&Option<At<T>>>) -> Option<Sourced<T>> {
    match (value(mine), work.and_then(value)) {
        (Some(value), _) => Some(Sourced {
            value,
            from: Origin::Recipe,
        }),
        (None, Some(value)) => Some(Sourced {
            value,
            from: Origin::Work,
        }),
        (None, None) => None,
    }
}

/// The recipe's whole list replaces the work file's. A list without items
/// (refused by the checks in a recipe, allowed in a work file) states
/// nothing.
fn replaced(mine: &[At<String>], work: Option<&[At<String>]>) -> Option<Sourced<Vec<String>>> {
    if !mine.is_empty() {
        return Some(Sourced {
            value: strings(mine),
            from: Origin::Recipe,
        });
    }
    work.filter(|w| !w.is_empty()).map(|w| Sourced {
        value: strings(w),
        from: Origin::Work,
    })
}

/// The union: the work file's labels, then each of the recipe's it lacks.
fn union(mine: &[At<String>], work: &[At<String>]) -> Vec<Sourced<String>> {
    let mut out: Vec<Sourced<String>> = Vec::new();
    let tagged = work
        .iter()
        .map(|l| (l, Origin::Work))
        .chain(mine.iter().map(|l| (l, Origin::Recipe)));
    for (label, from) in tagged {
        if !out.iter().any(|o| o.value == label.value) {
            out.push(Sourced {
                value: label.value.clone(),
                from,
            });
        }
    }
    out
}

/// Per key, the recipe's value; sorted by key, as the metadata keeps them.
fn per_key<T: Clone + PartialEq>(
    mine: Vec<(String, T)>,
    work: Vec<(String, T)>,
) -> Vec<(String, Sourced<T>)> {
    let mut out: Vec<(String, Sourced<T>)> = work
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                Sourced {
                    value,
                    from: Origin::Work,
                },
            )
        })
        .collect();
    for (key, value) in mine {
        let sourced = Sourced {
            value,
            from: Origin::Recipe,
        };
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => *slot = sourced,
            None => out.push((key, sourced)),
        }
    }
    out.sort_by(|(a, _), (b, _)| a.cmp(b));
    out
}

fn ids(ids: &[WorkId]) -> Vec<(String, String)> {
    ids.iter()
        .map(|i| (i.catalogue.value.clone(), i.id.value.clone()))
        .collect()
}

fn needs(needs: &[Need]) -> Vec<(String, Vec<String>)> {
    needs
        .iter()
        .map(|n| (n.kind.value.clone(), strings(&n.values)))
        .collect()
}

/// Merge a Mediafile with the work file it names (`None` when it names
/// none). `None` when the recipe lacks what the checks require of it: an
/// identity with name, platform and revision, and a title stated by it or
/// its work file.
pub(crate) fn merge(recipe: &Recipe, work: Option<&WorkFile>) -> Option<Merged> {
    let identity = &recipe.identity;
    let shared = work.and_then(|w| w.title.as_ref());
    let own = recipe.title.as_ref();
    let empty: &[At<String>] = &[];
    // A recipe without [title] states each field as absent.
    let mine = |f: fn(&model::Title) -> &Option<At<String>>| own.map(f).cloned().flatten();
    let list = |f: fn(&model::Title) -> &Vec<At<String>>| own.map_or(empty, |t| f(t).as_slice());
    let theirs = |f: fn(&model::Title) -> &Vec<At<String>>| shared.map(|t| f(t).as_slice());
    let synopsis = shared.and_then(|t| value(&t.synopsis));
    let description = match (own.and_then(|t| value(&t.description)), &synopsis) {
        (Some(value), _) => Some(Sourced {
            value,
            from: Origin::Recipe,
        }),
        (None, Some(synopsis)) => Some(Sourced {
            value: synopsis.clone(),
            from: Origin::Work,
        }),
        (None, None) => None,
    };
    Some(Merged {
        name: value(&identity.name)?,
        platform: value(&identity.platform)?,
        release: value(&identity.release),
        revision: value(&identity.revision)?,
        digest: value(&identity.digest),
        work: value(&recipe.work),
        work_file: recipe
            .work
            .as_ref()
            .map(|w| format!("{}.work.toml", w.value)),
        title: single(&mine(|t| &t.title), shared.map(|t| &t.title))?,
        sort_title: single(&mine(|t| &t.sort_title), shared.map(|t| &t.sort_title)),
        series: single(&mine(|t| &t.series), shared.map(|t| &t.series)),
        software_platform: single(
            &mine(|t| &t.software_platform),
            shared.map(|t| &t.software_platform),
        ),
        players: single(
            &own.and_then(|t| t.players.clone()),
            shared.map(|t| &t.players),
        ),
        publishers: replaced(list(|t| &t.publishers), theirs(|t| &t.publishers)),
        developers: replaced(list(|t| &t.developers), theirs(|t| &t.developers)),
        genres: replaced(list(|t| &t.genres), theirs(|t| &t.genres)),
        labels: union(list(|t| &t.labels), theirs(|t| &t.labels).unwrap_or(empty)),
        synopsis,
        description,
        version: value(&mine(|t| &t.version)),
        edition: value(&mine(|t| &t.edition)),
        year: own.and_then(|t| value(&t.year)),
        languages: strings(list(|t| &t.languages)),
        regions: strings(list(|t| &t.regions)),
        source_media: value(&mine(|t| &t.source_media)),
        requirements: value(&mine(|t| &t.requirements)),
        ids: per_key(
            ids(&recipe.ids),
            work.map(|w| ids(&w.ids)).unwrap_or_default(),
        ),
        needs: per_key(
            needs(&recipe.needs),
            work.map(|w| needs(&w.needs)).unwrap_or_default(),
        ),
    })
}

impl Merged {
    /// The tag the output is published under: `<release>-r<revision>`, or
    /// `r<revision>` without a release (recipes §6.3).
    pub fn tag(&self) -> String {
        match &self.release {
            Some(release) => format!("{release}-r{}", self.revision),
            None => format!("r{}", self.revision),
        }
    }

    /// The record as the title metadata of the set: the stable id is the
    /// last component of `name` (recipes §6.4), `year` is `release_year`,
    /// `software_platform` is the title's `platform`, the needs and ids are
    /// maps by kind and catalogue, and the work id and synopsis are their
    /// own fields (metadata schema 2). Labels belong to the envelope
    /// ([`Self::metadata`]), not to the title.
    pub fn title(&self) -> metadata::Title {
        let take = |v: &Option<Sourced<String>>| v.as_ref().map(|s| s.value.clone());
        let list = |v: &Option<Sourced<Vec<String>>>| {
            v.as_ref().map(|s| s.value.clone()).unwrap_or_default()
        };
        metadata::Title {
            id: self.name.rsplit('/').next().unwrap_or_default().to_owned(),
            title: self.title.value.clone(),
            sort_title: take(&self.sort_title),
            series: take(&self.series),
            genres: list(&self.genres),
            developers: list(&self.developers),
            publishers: list(&self.publishers),
            platform: take(&self.software_platform),
            release_year: self.year,
            players: self.players.as_ref().and_then(|p| {
                Some(metadata::Players {
                    min: u32::try_from(p.value.min).ok()?,
                    max: u32::try_from(p.value.max).ok()?,
                })
            }),
            languages: self.languages.clone(),
            regions: self.regions.clone(),
            description: take(&self.description),
            version: self.version.clone(),
            edition: self.edition.clone(),
            source_media: self.source_media.clone(),
            needs: self
                .needs
                .iter()
                .map(|(kind, values)| (kind.clone(), values.value.clone()))
                .collect(),
            requirements: self.requirements.clone(),
            work_id: self.work.clone(),
            synopsis: self.synopsis.clone(),
            work_ids: self
                .ids
                .iter()
                .map(|(catalogue, id)| (catalogue.clone(), id.value.clone()))
                .collect(),
            ..metadata::Title::default()
        }
    }

    /// The set's whole metadata: the platform by its current id, the merged
    /// labels as the envelope's labels in merged order, and the one title
    /// record ([`Self::title`]).
    pub fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::new(image_store::platform::canonical(&self.platform));
        metadata.labels = self.labels.iter().map(|l| l.value.clone()).collect();
        metadata.titles.push(self.title());
        metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::{Sink, Source};
    use crate::model::Document;

    fn document(text: &str) -> Document {
        let source = Source::new("x.toml", text.to_owned());
        let mut sink = Sink::new(&source);
        let document = crate::syntax::parse(&mut sink)
            .and_then(|root| crate::parse::document(&mut sink, &root))
            .unwrap();
        assert!(sink.findings.is_empty(), "{:?}", sink.findings);
        document
    }

    const HEAD: &str = "recipe = \"0.1\"\nwork = \"w\"\n\n[media]\nname = \"private/media/ibm-pc/w\"\nplatform = \"ibm-pc\"\nrelease = \"1.0\"\nrevision = 2\n\n";
    const MEDIUM: &str = "\n[input.disk]\nname = \"x.img\"\nsize = 1474560\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n\n[[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n";

    /// Merge a recipe whose tables are `recipe` with a work file whose
    /// tables are `work` (`None`: the recipe names no work).
    fn merged(recipe: &str, work: Option<&str>) -> Merged {
        let head = if work.is_some() {
            HEAD.to_owned()
        } else {
            HEAD.replace("work = \"w\"\n", "")
        };
        let Document::Recipe(recipe) = document(&format!("{head}{recipe}{MEDIUM}")) else {
            panic!("not a recipe");
        };
        let work = work.map(|w| {
            let Document::Work(work) =
                document(&format!("recipe = \"0.1\"\n\n[work]\nid = \"w\"\n\n{w}"))
            else {
                panic!("not a work file");
            };
            work
        });
        merge(&recipe, work.as_deref()).unwrap()
    }

    fn sourced<T>(value: T, from: Origin) -> Sourced<T> {
        Sourced { value, from }
    }

    fn vec(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }

    #[test]
    fn a_single_value_is_the_recipes_when_both_state_it() {
        let m = merged(
            "[title]\ntitle = \"Probotector\"\nseries = \"Contra\"\n",
            Some("[title]\ntitle = \"Contra\"\nsort_title = \"Contra 1\"\nseries = \"Contra Series\"\nsoftware_platform = \"DOS\"\nplayers = { min = 1, max = 2 }\n"),
        );
        assert_eq!(m.title, sourced("Probotector".into(), Origin::Recipe));
        assert_eq!(m.series, Some(sourced("Contra".into(), Origin::Recipe)));
        // Stated by the work file only: the work file's.
        assert_eq!(m.sort_title, Some(sourced("Contra 1".into(), Origin::Work)));
        assert_eq!(
            m.software_platform,
            Some(sourced("DOS".into(), Origin::Work))
        );
        assert_eq!(
            m.players,
            Some(sourced(Players { min: 1, max: 2 }, Origin::Work))
        );
        let m = merged(
            "[title]\nplayers = { min = 1, max = 4 }\nsoftware_platform = \"Windows 3.x\"\nsort_title = \"B\"\n",
            Some("[title]\ntitle = \"W\"\nsort_title = \"A\"\nsoftware_platform = \"DOS\"\nplayers = { min = 1, max = 2 }\n"),
        );
        assert_eq!(m.title, sourced("W".into(), Origin::Work));
        assert_eq!(m.sort_title, Some(sourced("B".into(), Origin::Recipe)));
        assert_eq!(
            m.software_platform,
            Some(sourced("Windows 3.x".into(), Origin::Recipe))
        );
        assert_eq!(
            m.players,
            Some(sourced(Players { min: 1, max: 4 }, Origin::Recipe))
        );
    }

    #[test]
    fn a_list_of_credits_or_genres_is_replaced_whole_never_mixed() {
        let work = "[title]\ntitle = \"W\"\npublishers = [\"Konami\"]\ndevelopers = [\"Konami\", \"KCEJ\"]\ngenres = [\"run and gun\"]\n";
        let m = merged(
            "[title]\npublishers = [\"Palcom\"]\ndevelopers = [\"Konami\"]\ngenres = [\"action\", \"run and gun\"]\n",
            Some(work),
        );
        assert_eq!(
            m.publishers,
            Some(sourced(vec(&["Palcom"]), Origin::Recipe))
        );
        assert_eq!(
            m.developers,
            Some(sourced(vec(&["Konami"]), Origin::Recipe))
        );
        assert_eq!(
            m.genres,
            Some(sourced(vec(&["action", "run and gun"]), Origin::Recipe))
        );
        // A list the recipe leaves out is the work file's.
        let m = merged("[title]\n", Some(work));
        assert_eq!(m.publishers, Some(sourced(vec(&["Konami"]), Origin::Work)));
        assert_eq!(
            m.developers,
            Some(sourced(vec(&["Konami", "KCEJ"]), Origin::Work))
        );
        assert_eq!(m.genres, Some(sourced(vec(&["run and gun"]), Origin::Work)));
        assert_eq!(merged("[title]\ntitle = \"T\"\n", None).publishers, None);
    }

    #[test]
    fn labels_are_the_union_the_work_files_first_each_once() {
        let m = merged(
            "[title]\nlabels = [\"freeware\", \"game\", \"dos\", \"freeware\"]\n",
            Some("[title]\ntitle = \"W\"\nlabels = [\"game\", \"first-person-shooter\"]\n"),
        );
        assert_eq!(
            m.labels,
            [
                sourced("game".to_owned(), Origin::Work),
                sourced("first-person-shooter".to_owned(), Origin::Work),
                sourced("freeware".to_owned(), Origin::Recipe),
                sourced("dos".to_owned(), Origin::Recipe),
            ]
        );
        assert_eq!(
            m.metadata().labels,
            ["game", "first-person-shooter", "freeware", "dos"]
        );
        // Either side alone.
        let only_work = merged(
            "[title]\n",
            Some("[title]\ntitle = \"W\"\nlabels = [\"game\"]\n"),
        );
        assert_eq!(only_work.labels, [sourced("game".to_owned(), Origin::Work)]);
        let only_recipe = merged(
            "[title]\nlabels = [\"game\"]\n",
            Some("[title]\ntitle = \"W\"\n"),
        );
        assert_eq!(
            only_recipe.labels,
            [sourced("game".to_owned(), Origin::Recipe)]
        );
    }

    #[test]
    fn ids_merge_per_catalogue_the_recipes_id_winning() {
        let m = merged(
            "[title]\n\n[ids]\nmobygames = \"contra\"\nwikidata = \"Q2\"\n",
            Some("[title]\ntitle = \"W\"\n\n[ids]\nwikidata = \"Q1\"\nigdb = \"contra\"\n"),
        );
        assert_eq!(
            m.ids,
            [
                (
                    "igdb".to_owned(),
                    sourced("contra".to_owned(), Origin::Work)
                ),
                (
                    "mobygames".to_owned(),
                    sourced("contra".to_owned(), Origin::Recipe)
                ),
                (
                    "wikidata".to_owned(),
                    sourced("Q2".to_owned(), Origin::Recipe)
                ),
            ]
        );
    }

    #[test]
    fn needs_merge_per_kind_the_recipes_alternatives_winning_whole() {
        let m = merged(
            "[title]\n\n[needs]\nvideo = [\"ega\"]\nsound = \"none\"\n",
            Some("[title]\ntitle = \"W\"\n\n[needs]\ncpu = \"286+\"\nvideo = [\"vga\", \"ega\"]\n"),
        );
        assert_eq!(
            m.needs,
            [
                ("cpu".to_owned(), sourced(vec(&["286+"]), Origin::Work)),
                ("sound".to_owned(), sourced(vec(&["none"]), Origin::Recipe)),
                ("video".to_owned(), sourced(vec(&["ega"]), Origin::Recipe)),
            ]
        );
    }

    #[test]
    fn release_fields_come_from_the_recipe_and_the_synopsis_from_the_work_file() {
        let m = merged(
            "[title]\nversion = \"2.01\"\nedition = \"Full\"\nyear = 1993\nlanguages = [\"en\"]\nregions = [\"USA\"]\nsource_media = \"1 x 1.44M\"\nrequirements = \"286, VGA\"\ndescription = \"This release.\"\n",
            Some("[title]\ntitle = \"W\"\nsynopsis = \"The work.\"\n"),
        );
        assert_eq!(m.version.as_deref(), Some("2.01"));
        assert_eq!(m.edition.as_deref(), Some("Full"));
        assert_eq!(m.year, Some(1993));
        assert_eq!(m.languages, ["en"]);
        assert_eq!(m.regions, ["USA"]);
        assert_eq!(m.source_media.as_deref(), Some("1 x 1.44M"));
        assert_eq!(m.requirements.as_deref(), Some("286, VGA"));
        assert_eq!(m.synopsis.as_deref(), Some("The work."));
        assert_eq!(
            m.description,
            Some(sourced("This release.".into(), Origin::Recipe))
        );
        assert_eq!(m.work.as_deref(), Some("w"));
        assert_eq!(m.work_file.as_deref(), Some("w.work.toml"));
    }

    #[test]
    fn without_a_description_of_its_own_a_release_is_described_by_the_synopsis() {
        let m = merged(
            "[title]\n",
            Some("[title]\ntitle = \"W\"\nsynopsis = \"The work.\"\n"),
        );
        assert_eq!(
            m.description,
            Some(sourced("The work.".into(), Origin::Work))
        );
        assert_eq!(m.synopsis.as_deref(), Some("The work."));
        assert_eq!(m.title().description.as_deref(), Some("The work."));
        assert_eq!(m.title().synopsis.as_deref(), Some("The work."));
        let m = merged("[title]\n", Some("[title]\ntitle = \"W\"\n"));
        assert_eq!((m.description, m.synopsis), (None, None));
    }

    #[test]
    fn a_recipe_without_a_work_file_is_its_own_record() {
        let m = merged(
            "[title]\ntitle = \"T\"\npublishers = [\"P\"]\nlabels = [\"game\"]\n\n[needs]\ncpu = \"8088+\"\n",
            None,
        );
        assert_eq!(m.title, sourced("T".into(), Origin::Recipe));
        assert_eq!(m.publishers, Some(sourced(vec(&["P"]), Origin::Recipe)));
        assert_eq!(m.labels, [sourced("game".to_owned(), Origin::Recipe)]);
        assert_eq!(
            m.needs,
            [("cpu".to_owned(), sourced(vec(&["8088+"]), Origin::Recipe))]
        );
        assert_eq!((m.work, m.work_file, m.synopsis), (None, None, None));
    }

    #[test]
    fn the_tag_has_the_release_when_there_is_one() {
        let m = merged("[title]\ntitle = \"T\"\n", None);
        assert_eq!(m.tag(), "1.0-r2");
        let m = Merged { release: None, ..m };
        assert_eq!(m.tag(), "r2");
    }

    #[test]
    fn the_record_maps_onto_schema_2_metadata_and_round_trips() {
        let m = merged(
            "[title]\ntitle = \"Probotector\"\nyear = 1990\nregions = [\"Europe\"]\nlanguages = [\"en\"]\nversion = \"1\"\nedition = \"E\"\nsource_media = \"1 cartridge\"\nrequirements = \"a PAL machine\"\nlabels = [\"pal\"]\n\n[needs]\nvideo = [\"vga\"]\n",
            Some("[title]\ntitle = \"Contra\"\nsort_title = \"Contra\"\nseries = \"Contra\"\npublishers = [\"Konami\"]\ndevelopers = [\"Konami\"]\ngenres = [\"run and gun\"]\nsoftware_platform = \"DOS\"\nplayers = { min = 1, max = 2 }\nlabels = [\"game\"]\nsynopsis = \"The work.\"\n\n[ids]\nwikidata = \"Q1\"\n\n[needs]\ncpu = \"286+\"\n"),
        );
        let title = m.title();
        let expected = metadata::Title {
            id: "w".into(),
            title: "Probotector".into(),
            sort_title: Some("Contra".into()),
            series: Some("Contra".into()),
            genres: vec(&["run and gun"]),
            developers: vec(&["Konami"]),
            publishers: vec(&["Konami"]),
            platform: Some("DOS".into()),
            release_year: Some(1990),
            players: Some(metadata::Players { min: 1, max: 2 }),
            languages: vec(&["en"]),
            regions: vec(&["Europe"]),
            description: Some("The work.".into()),
            version: Some("1".into()),
            edition: Some("E".into()),
            source_media: Some("1 cartridge".into()),
            needs: [
                ("cpu".to_owned(), vec(&["286+"])),
                ("video".to_owned(), vec(&["vga"])),
            ]
            .into(),
            requirements: Some("a PAL machine".into()),
            work_id: Some("w".into()),
            synopsis: Some("The work.".into()),
            work_ids: [("wikidata".to_owned(), "Q1".to_owned())].into(),
            ..metadata::Title::default()
        };
        assert_eq!(title, expected);
        let metadata = m.metadata();
        assert_eq!(metadata.platform, "ibm-pc");
        assert_eq!(metadata.labels, ["game", "pal"]);
        let bytes = metadata.encode(&[]).unwrap();
        let decoded = Metadata::decode(&bytes, &[]).unwrap();
        assert_eq!(decoded.schema(), 2);
        assert!(!decoded.is_read_only());
        assert_eq!(decoded.labels, metadata.labels);
        assert_eq!(decoded.titles, [expected]);
        assert_eq!(decoded.encode(&[]).unwrap(), bytes);
    }

    #[test]
    fn a_retired_platform_id_is_written_by_its_current_id() {
        let m = merged("[title]\ntitle = \"T\"\n", None);
        for (retired, current) in image_store::platform::ALIASES {
            let m = Merged {
                platform: (*retired).to_owned(),
                ..m.clone()
            };
            assert_eq!(m.metadata().platform, *current);
        }
    }
}
