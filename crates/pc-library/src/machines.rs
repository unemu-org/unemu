// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Machines collection's model: named, local, mutable instances of
//! images, their facets, search and sort, the names the store accepts and
//! the settings that differ from the image; machine-images §3.2 and §7. The
//! worker lists machines through `pc_image::machine` and runs every change
//! there; the desktop draws the list, the details pane and the dialogs.

use crate::snapshot::Filter;
use image_store::summary::ImageSummary;
use library_model::listing::{human_size, ImageRow};
use std::collections::{BTreeMap, BTreeSet};

/// A machine as the library lists it.
#[derive(Clone, Debug)]
pub struct MachineRow {
    pub listing: pc_image::machine::Listing,
    /// The image's library summary: its catalogue titles and labels.
    pub summary: Option<ImageSummary>,
    /// The image's title, for search and the details pane.
    pub title: Option<String>,
    /// The floppy drives the machine runs with, by unit.
    pub drives: Vec<(u8, assembly::FloppyGeometry)>,
    /// Disks Create from files… made that its drives take at its next
    /// Start, kept in its workspace.
    pub created: Vec<pc_media::created_media::CreatedInsert>,
    /// Why the record of those disks cannot be read, when it cannot.
    pub created_problem: Option<String>,
    /// The floppy catalogues it had at its last Stop, which its drives take
    /// back at its next Start, kept in its workspace.
    pub kept: Vec<pc_media::kept_catalogue::KeptDrive>,
}

fn repository(reference: &str) -> &str {
    reference
        .split('@')
        .next()
        .unwrap_or_default()
        .rsplit_once(':')
        .map_or(reference, |(repo, _)| repo)
}

impl MachineRow {
    pub fn id(&self) -> &str {
        &self.listing.id
    }
    pub fn name(&self) -> &str {
        &self.listing.machine.name
    }
    /// The tag every image action of the machine uses: the tag it was made
    /// from while that still names its image, else another tag that does,
    /// else the repository at the digest.
    pub fn reference(&self) -> String {
        let source = &self.listing.machine.source;
        if self.listing.tags.iter().any(|tag| tag == source) {
            source.clone()
        } else if let Some(tag) = self.listing.tags.first() {
            tag.clone()
        } else {
            let repo = repository(source);
            format!(
                "{}@{}",
                if repo.is_empty() { "machine" } else { repo },
                self.listing.machine.image.digest
            )
        }
    }
    /// The source image as the Source image column names it.
    pub fn source_label(&self) -> String {
        if self.listing.detached() {
            let repo = repository(&self.listing.machine.source);
            format!("{} (no tag)", if repo.is_empty() { "image" } else { repo })
        } else {
            self.reference()
        }
    }
    /// The image's row, for readiness checks and the image actions.
    pub fn image_row(&self) -> ImageRow {
        ImageRow {
            reference: self.reference(),
            aliases: Vec::new(),
            descriptor: self.listing.machine.image.clone(),
            summary: self.summary.clone(),
            failure: None,
        }
    }
    /// The machines mock-up's badges after the source image: the tag the
    /// machine came from moved, or no tag names its image any more.
    pub fn source_badges(&self) -> Vec<String> {
        let mut badges = Vec::new();
        if self.listing.moved_to.is_some() {
            let tag = self
                .listing
                .machine
                .source
                .rsplit_once(':')
                .map_or("tag", |(_, tag)| tag);
            badges.push(format!("⚑ {tag} moved"));
        }
        if self.listing.detached() {
            badges.push("detached".into());
        }
        badges
    }
    /// The catalogue entries the machine runs: its own firmware choice
    /// where its settings make one, else its image's.
    pub fn firmware(
        &self,
    ) -> Vec<(
        crate::firmware_choices::Slot,
        &'static assembly::FirmwareEntry,
    )> {
        crate::firmware_choices::machine_firmware(
            self.listing.machine.settings.as_ref(),
            self.summary.as_ref(),
        )
    }
    /// The firmware the machine's own settings choose instead of its
    /// image's, by slot.
    /// The firmware the machine's own settings choose instead of its
    /// image's, by slot, each with the mark of the package blob the machine
    /// binds (open only when every chip is), else the catalogue's.
    pub fn own_firmware(&self) -> Vec<crate::firmware_choices::Own> {
        let Some(settings) = self.listing.machine.settings.as_ref() else {
            return Vec::new();
        };
        let image = self
            .summary
            .as_ref()
            .map(crate::firmware_choices::in_summary)
            .unwrap_or_default();
        crate::firmware_choices::own_choices(settings)
            .into_iter()
            .filter(|own| {
                !image
                    .iter()
                    .any(|(s, e)| *s == own.slot && e.id == own.entry.id)
            })
            .collect()
    }
    /// The superseded firmware the machine runs: the "newer version
    /// available" hint and the row badge.
    pub fn newer(&self) -> Vec<crate::firmware_choices::Newer> {
        crate::firmware_choices::newer_of(&self.firmware())
    }
    pub fn state_text(&self) -> String {
        let mut text = self.listing.state.to_string();
        for insert in &self.listing.machine.pending {
            text.push_str(&format!(" · {} insert pending", insert.drive));
        }
        for insert in &self.created {
            if !self
                .listing
                .machine
                .pending
                .iter()
                .any(|p| p.drive == insert.drive)
            {
                text.push_str(&format!(" · {} insert pending", insert.drive));
            }
        }
        if self.created_problem.is_some() {
            text.push_str(" · created disks unreadable");
        }
        text
    }
    pub fn last_run(&self) -> &str {
        self.listing.machine.last_run.as_deref().unwrap_or("Never")
    }
    fn searched(&self, text: &str) -> bool {
        let text = text.trim().to_lowercase();
        text.is_empty()
            || format!(
                "{} {} {} {} {}",
                self.name(),
                self.id(),
                self.source_label(),
                self.title.as_deref().unwrap_or_default(),
                self.listing.machine.labels.join(" ")
            )
            .to_lowercase()
            .contains(&text)
    }
    /// The values a facet of the Machines collection takes for this machine:
    /// its own labels, else its image's catalogue metadata.
    fn facet_values(&self, kind: &str) -> Vec<String> {
        if kind == "Labels" {
            return self.listing.machine.labels.clone();
        }
        let titles = self.summary.iter().flat_map(|s| s.titles.iter());
        titles
            .flat_map(|t| match kind {
                "Genre" => t.genres.clone(),
                "Platform" => t.platform.iter().cloned().collect(),
                "Year" => t.year.map(|y| y.to_string()).into_iter().collect(),
                "Developer" => t.developers.clone(),
                "Publisher" => t.publishers.clone(),
                "Series" => t.series.iter().cloned().collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    pub fn cells(&self) -> [String; 5] {
        let mut source = format!(
            "{} {}",
            self.source_label(),
            &self.listing.machine.image.digest.hex()[..8]
        );
        for badge in self.source_badges() {
            source.push_str(" · ");
            source.push_str(&badge);
        }
        let mut state = self.state_text();
        if !self.newer().is_empty() {
            state.push_str(" · ");
            state.push_str(crate::firmware_choices::BADGE);
        }
        [
            self.name().to_owned(),
            source,
            state,
            self.last_run().to_owned(),
            human_size(self.listing.change_bytes),
        ]
    }
    /// The State cell's tooltip: the hint of the quiet badge, when it shows.
    pub fn hints(&self) -> [Option<String>; 5] {
        [
            None,
            None,
            crate::firmware_choices::badge_tooltip(&self.newer()),
            None,
            None,
        ]
    }
}

/// The facets of the Machines collection, in the tree's order.
pub const FACETS: [&str; 7] = [
    "Labels",
    "Platform",
    "Genre",
    "Year",
    "Developer",
    "Publisher",
    "Series",
];

pub fn facets(rows: &[MachineRow]) -> BTreeMap<String, BTreeSet<String>> {
    let mut facets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        for kind in FACETS {
            facets
                .entry(kind.into())
                .or_default()
                .extend(row.facet_values(kind));
        }
    }
    facets
}

/// The machines a filter and a search show, sorted by a column. Recently
/// played always lists the last run first.
pub fn visible<'a>(
    rows: &'a [MachineRow],
    filter: &Filter,
    text: &str,
    sort: usize,
    descending: bool,
) -> Vec<&'a MachineRow> {
    let mut shown: Vec<&MachineRow> = rows
        .iter()
        .filter(|row| match filter {
            Filter::Recent => row.listing.machine.last_run.is_some(),
            Filter::Favorites => row.listing.machine.favorite,
            Filter::MachineFacet(kind, value) => row.facet_values(kind).contains(value),
            _ => true,
        })
        .filter(|row| row.searched(text))
        .collect();
    let (sort, descending) = if *filter == Filter::Recent {
        (3, true)
    } else {
        (sort, descending)
    };
    shown.sort_by_cached_key(|row| match sort {
        1 => row.source_label().to_lowercase(),
        2 => row.state_text().to_lowercase(),
        3 => row.listing.machine.last_run.clone().unwrap_or_default(),
        4 => format!("{:020}", row.listing.change_bytes),
        _ => row.name().to_lowercase(),
    });
    if descending {
        shown.reverse();
    }
    shown
}

/// Why a machine name cannot be used: the store's rules, checked before it
/// refuses. `taken` lists the other machines' names.
pub fn name_problem<'a>(value: &str, taken: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let name = value.trim();
    if name.is_empty() {
        return Some("Type a name.".into());
    }
    let count = name.chars().count();
    if count > image_store::machine::MAX_NAME_CHARS {
        return Some(format!("At most 64 characters ({count} now)."));
    }
    if let Err(error) = image_store::machine::validate_name(name) {
        return Some(error.to_string());
    }
    let fold = |s: &str| s.to_uppercase().to_lowercase();
    taken
        .into_iter()
        .find(|other| fold(other) == fold(name))
        .map(|other| {
            format!(
                "A machine named \u{201c}{other}\u{201d} exists already. Names are unique, ignoring case."
            )
        })
}

/// The workspace id the store would give a new machine of this name.
pub fn id_for<'a>(name: &str, ids: impl IntoIterator<Item = &'a str> + Clone) -> String {
    let slug = image_store::machine::slug(if name.trim().is_empty() {
        "machine"
    } else {
        name
    });
    let taken = |id: &str| ids.clone().into_iter().any(|other| other == id);
    if !taken(&slug) {
        return slug;
    }
    (2u32..)
        .map(|n| format!("{slug}-{n}"))
        .find(|id| !taken(id))
        .unwrap_or(slug)
}

/// A row of "Settings that differ from the image": the setting, this
/// machine's value and the image's.
pub type SettingDiff = (String, String, String);

/// The settings of a machine that differ from its image, by the property
/// sheet's field names.
pub fn setting_diffs(settings: &pc_image::machine::Settings) -> Result<Vec<SettingDiff>, String> {
    use assembly::capabilities as caps;
    let image = crate::image_settings::describe(&settings.image)?;
    let machine = crate::image_settings::describe(&settings.machine)?;
    let before = caps::fields(&image);
    Ok(caps::fields(&machine)
        .into_iter()
        .filter_map(|field| {
            let old = before.iter().find(|old| old.path == field.path);
            if old.is_some_and(|old| old.value == field.value) {
                return None;
            }
            Some((
                field.label.clone(),
                caps::value_label(&machine, &field.path, &field.value),
                old.map_or_else(
                    || "—".to_owned(),
                    |old| caps::value_label(&image, &old.path, &old.value),
                ),
            ))
        })
        .collect())
}

/// A pending insert as the details pane and the drive chips name it.
pub fn pending_name(insert: &image_store::machine::PendingInsert) -> String {
    let set = if insert.reference.is_empty() {
        "a set".to_owned()
    } else {
        insert.reference.clone()
    };
    if insert.all {
        format!("{set} {} (all disks join the catalogue)", insert.medium)
    } else {
        format!("{set} {}", insert.medium)
    }
}
