// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the library frame asks the worker to list (the selected collection
//! or facet, the search text, each list's sort and the open repository
//! groups) and the snapshot the worker answers with; machine-images §§2.1
//! and 7. The snapshot folds the Images, Firmware and Installation media
//! collections, their facets and the firmware replacement hints into one
//! answer.

use image_store::{
    summary::{ArtifactKind, Facets, ImageSummary, SummaryCache},
    Digest,
};
use library_model::listing::{listed, source, ImageRow};
use std::collections::{BTreeMap, BTreeSet};

/// The collection, list or facet the tree selects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    /// The Machines collection, where the library opens.
    Machines,
    /// The Images collection; the worker's first snapshot lists its rows.
    #[default]
    All,
    /// Machines run before, the last run first.
    Recent,
    /// Favourite machines.
    Favorites,
    /// A facet of the Machines collection: the machine's own labels or its
    /// source image's catalogue metadata.
    MachineFacet(String, String),
    MachineFiles,
    Firmware,
    Facet(String, String),
    Media,
    /// A facet of the Installation media collection; its kinds share names
    /// with machine facets, so the collection is part of the filter.
    MediaFacet(String, String),
}
impl Filter {
    /// The Firmware collection and its facets list packages instead of machines.
    pub fn is_firmware(&self) -> bool {
        match self {
            Self::Firmware => true,
            Self::Facet(kind, _) => crate::firmware::is_facet(kind),
            _ => false,
        }
    }
    /// The Installation media collection and its facets list sets instead of machines.
    pub fn is_media(&self) -> bool {
        matches!(self, Self::Media | Self::MediaFacet(_, _))
    }
    /// The Machines collection, its Recently played and Favourites lists and
    /// its facets list machines.
    pub fn is_machines(&self) -> bool {
        matches!(
            self,
            Self::Machines | Self::Recent | Self::Favorites | Self::MachineFacet(_, _)
        )
    }
}
/// What the frame asks the worker to list.
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub text: String,
    pub filter: Filter,
    pub sort: usize,
    pub descending: bool,
    pub expanded: BTreeSet<String>,
    /// The Firmware list has other columns, so it keeps its own sort.
    pub firmware_sort: usize,
    pub firmware_descending: bool,
    /// So does the Installation media list.
    pub media_sort: usize,
    pub media_descending: bool,
    /// And the Machines list.
    pub machine_sort: usize,
    pub machine_descending: bool,
}

/// A row of the Images list: an image, or a repository's group row.
#[derive(Clone)]
pub struct ViewRow {
    pub image: ImageRow,
    /// A repository's group row, with its number of images.
    pub group: Option<usize>,
    pub child: bool,
}
/// The worker's answer to a query: every list's rows, the facets and the
/// library's totals.
#[derive(Default)]
pub struct Snapshot {
    pub rows: Vec<ViewRow>,
    pub facets: BTreeMap<String, BTreeSet<String>>,
    pub total: usize,
    pub matched: usize,
    pub pending: usize,
    pub nominal_bytes: u64,
    pub storage: Option<(u64, u64)>,
    pub warning: Option<String>,
    pub firmware_rows: Vec<crate::firmware::FirmwareRow>,
    pub firmware_total: usize,
    pub media_rows: Vec<crate::install_media::MediaRow>,
    pub media_total: usize,
    /// Set summaries still being built; `pending` counts every artifact.
    pub media_pending: usize,
    /// Kept apart from machine facets, whose kinds have the same names.
    pub media_facets: BTreeMap<String, BTreeSet<String>>,
    /// The worker read the store, so an empty library is really empty.
    pub read: bool,
    /// Every local tag with the manifest it names, for the save forms' checks.
    pub tags: Vec<(String, Digest)>,
    /// Every installation media set, whatever the collection, for the media
    /// picker.
    pub all_sets: Vec<crate::install_media::MediaRow>,
    /// The library's firmware package names by the raw identity of each ROM
    /// and chip they hold, for naming the ROMs of an image.
    pub rom_names: BTreeMap<Digest, String>,
    /// Every machine image, whatever the collection, search or open groups,
    /// that binds a release replacing a superseded one: the "newer version
    /// available" hint links to one of them.
    pub replacements: Vec<ImageRow>,
}

fn facets_for(filter: &Filter) -> Facets {
    let mut f = Facets::default();
    if let Filter::Facet(kind, value) | Filter::MediaFacet(kind, value) = filter {
        match kind.as_str() {
            "Genre" => f.genres.push(value.clone()),
            "Platform" => f.platforms.push(value.clone()),
            "Year" => {
                if let Ok(y) = value.parse() {
                    f.years.push(y);
                }
            }
            "Developer" => f.developers.push(value.clone()),
            "Publisher" => f.publishers.push(value.clone()),
            "Series" => f.series.push(value.clone()),
            "Labels" => f.labels.push(value.clone()),
            "Role" => f.firmware_roles.push(value.clone()),
            "Board" => f.boards.push(value.clone()),
            "Machine class" => f.machine_classes.push(value.clone()),
            _ => (),
        }
    }
    f
}
fn sort_value(row: &ImageRow, column: usize) -> String {
    let s = row.summary.as_ref();
    match column {
        1 => source(&row.descriptor).text(),
        2 => row.reference.clone(),
        3 => row.descriptor.digest.hex().to_owned(),
        5 => s.map(redistribution).unwrap_or_default().into(),
        6 => format!("{:020}", s.map_or(0, |s| s.stored_size)),
        _ => row.name().to_lowercase(),
    }
}
pub fn snapshot(cache: &SummaryCache, q: &Query) -> Snapshot {
    // Counts are of rows, so a manifest under several tags counts once.
    let listed = listed(cache.tags());
    let mut result = Snapshot {
        storage: cache.storage_totals(),
        total: listed.len(),
        warning: cache.library_error().map(str::to_owned),
        read: true,
        tags: cache
            .tags()
            .iter()
            .map(|tag| (tag.name.clone(), tag.manifest.digest.clone()))
            .collect(),
        ..Default::default()
    };
    let matches: BTreeSet<_> = cache
        .query(&q.text, &facets_for(&q.filter))
        .into_iter()
        .map(|r| r.reference)
        .collect();
    let mut groups: BTreeMap<String, Vec<ImageRow>> = BTreeMap::new();
    let mut counted = BTreeSet::new();
    let firmware_mode = q.filter.is_firmware();
    let media_mode = q.filter.is_media();
    let mut packages = Vec::new();
    let mut sets = Vec::new();
    let pending: BTreeSet<_> = cache
        .pending()
        .into_iter()
        .map(|tag| tag.name.as_str())
        .collect();
    let mut machine_roms = crate::firmware::MachineRoms::default();
    for row in &listed {
        let tag = row.tag;
        let s = cache.summary(&tag.manifest.digest);
        result.pending += usize::from(pending.contains(tag.name.as_str()));
        let kind = s.map_or_else(
            || match tag.manifest.artifact_type.as_deref() {
                Some(image_store::media_type::MEDIA_SET) => ArtifactKind::MediaSet,
                Some(image_store::media_type::FIRMWARE) => ArtifactKind::Firmware,
                _ => ArtifactKind::Machine,
            },
            |summary| summary.kind,
        );
        // Media sets and firmware packages have no launch action; they belong
        // to their own collections, not to the machine rows.
        if kind != ArtifactKind::Machine {
            result.total -= 1;
            if kind == ArtifactKind::Firmware {
                result.firmware_total += 1;
                if let Some(s) = s {
                    crate::firmware::add_facets(&mut result.facets, s);
                }
                packages.push(row);
            } else {
                result.media_total += 1;
                result.media_pending += usize::from(pending.contains(tag.name.as_str()));
                if let Some(s) = s {
                    crate::install_media::add_facets(&mut result.media_facets, s);
                }
                sets.push(row);
            }
            continue;
        }
        if let Some(s) = s.filter(|_| firmware_mode) {
            machine_roms.add(&row.references(), s);
        }
        if s.is_some_and(|s| {
            crate::firmware_choices::in_summary(s)
                .iter()
                .any(|(_, e)| crate::firmware_choices::replaces_any(e))
        }) {
            result.replacements.push(row.row(cache));
        }
        if let Some(s) = s {
            if counted.insert(s.image.clone()) {
                result.nominal_bytes += s.stored_size;
            }
            for label in &s.labels {
                result
                    .facets
                    .entry("Labels".into())
                    .or_default()
                    .insert(label.clone());
            }
            for t in &s.titles {
                for (kind, values) in [
                    ("Genre", t.genres.clone()),
                    ("Platform", t.platform.iter().cloned().collect()),
                    ("Year", t.year.map(|y| y.to_string()).into_iter().collect()),
                    ("Developer", t.developers.clone()),
                    ("Publisher", t.publishers.clone()),
                    ("Series", t.series.iter().cloned().collect()),
                ] {
                    result.facets.entry(kind.into()).or_default().extend(values);
                }
            }
        }
        if firmware_mode || media_mode {
            continue;
        }
        let text_match = if s.is_some() {
            row.matched(&matches)
        } else {
            row.named(&q.text)
        };
        let filter_match = match &q.filter {
            Filter::MachineFiles => false,
            filter if filter.is_machines() => false,
            Filter::Facet(_, _) => row.matched(&matches),
            _ => true,
        };
        if !text_match || !filter_match {
            continue;
        }
        let image = row.row(cache);
        groups
            .entry(image.repository().into())
            .or_default()
            .push(image);
        result.matched += 1;
    }
    if firmware_mode {
        result.firmware_rows = crate::firmware::rows(cache, &packages, &matches, q, &machine_roms);
        result.matched = result.firmware_rows.len();
    }
    if media_mode {
        result.media_rows = crate::install_media::rows(cache, &sets, &matches, q);
        result.matched = result.media_rows.len();
    }
    result.all_sets = crate::install_media::all_rows(cache, &sets);
    for package in &packages {
        let row = package.row(cache);
        let Some(f) = row.summary.as_ref().and_then(|s| s.firmware.as_ref()) else {
            continue;
        };
        for rom in &f.roms {
            for sha256 in std::iter::once(&rom.sha256).chain(rom.parts.iter().map(|p| &p.sha256)) {
                if let Some(digest) = crate::firmware::pin(sha256) {
                    result
                        .rom_names
                        .entry(digest)
                        .or_insert_with(|| row.name().to_owned());
                }
            }
        }
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    for images in &mut groups {
        images.sort_by_cached_key(|row| sort_value(row, q.sort));
        if q.descending {
            images.reverse();
        }
    }
    groups.sort_by_cached_key(|rows| sort_value(&rows[0], q.sort));
    if q.descending {
        groups.reverse();
    }
    for images in groups {
        let count = images.len();
        if count > 1 {
            let first = images[0].clone();
            let expanded = q.expanded.contains(first.repository());
            // The header counts the repository's images, one per manifest,
            // as the machines mock-up's group rows do.
            result.rows.push(ViewRow {
                image: first,
                group: Some(count),
                child: false,
            });
            if !expanded {
                continue;
            }
        }
        result.rows.extend(images.into_iter().map(|image| ViewRow {
            image,
            group: None,
            child: count > 1,
        }));
    }
    result
}

/// An image's redistribution class, as the push guard judges a whole image:
/// open only when its configuration and every layer are open. The summary's
/// blobs also hold the manifest's own descriptor, which carries no mark.
pub fn image_rights(summary: &ImageSummary) -> crate::firmware::Redistribution {
    crate::firmware::Redistribution::of_payloads(summary, |d| {
        d.media_type != image_store::media_type::MANIFEST
    })
}
/// An image's redistribution class in the list's words.
pub fn redistribution(summary: &ImageSummary) -> &'static str {
    if image_rights(summary).open {
        "open"
    } else {
        "user-supplied"
    }
}
