// SPDX-License-Identifier: AGPL-3.0-or-later
//! The rows of the library's image lists, as the store's tags and summaries
//! give them; machine-images §§2.1, 3 and 7. One row per manifest of a
//! repository, led by the tag that names it forever, with where the tag came
//! from, and the small texts the lists and panes share: sizes, short digests
//! and the time of a machine's last run. Platform-neutral and drawn by the
//! desktop's views.

use image_store::{
    summary::{ImageSummary, SummaryCache},
    Descriptor, Digest, Store, Tag,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct ImageRow {
    /// The tag every action on the row uses.
    pub reference: String,
    /// The other tags of this repository that name the same manifest. They
    /// share the row, so a version tag and its `latest` alias are listed once.
    pub aliases: Vec<String>,
    pub descriptor: Descriptor,
    pub summary: Option<ImageSummary>,
    pub failure: Option<String>,
}
impl ImageRow {
    /// Rows are one per manifest of a repository, whichever tag leads them.
    pub fn same(&self, other: &Self) -> bool {
        self.repository() == other.repository() && self.descriptor.digest == other.descriptor.digest
    }
    pub fn has_tag(&self, reference: &str) -> bool {
        self.reference == reference || self.aliases.iter().any(|a| a == reference)
    }
    /// Every tag of the row, the acting one first: "1.1.0, latest".
    pub fn tags(&self) -> String {
        tags(&self.reference, &self.aliases)
    }
    /// The repository with every tag of the row: "open-at:1.1.0, latest".
    pub fn references(&self) -> String {
        format!("{}:{}", self.repository(), self.tags())
    }
    pub fn name(&self) -> &str {
        self.summary
            .as_ref()
            .and_then(|s| {
                if s.name.is_empty() {
                    s.titles.first().map(|t| t.title.as_str())
                } else {
                    Some(s.name.as_str())
                }
            })
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.reference)
    }
    pub fn repository(&self) -> &str {
        self.reference
            .rsplit_once(':')
            .map_or(self.reference.as_str(), |(r, _)| r)
    }
}
fn tags(reference: &str, aliases: &[String]) -> String {
    std::iter::once(reference)
        .chain(aliases.iter().map(String::as_str))
        .map(|r| r.rsplit_once(':').map_or(r, |(_, tag)| tag))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where a local tag came from, as the library lists' Source column says it
/// (the remotes mock-up, revision 2; machine-images §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source<'a> {
    /// The registry host a pull wrote the tag from.
    Pulled(&'a str),
    /// Made in this library.
    Local,
    /// Written before the store recorded where tags come from; never guessed.
    Unknown,
}

impl Source<'_> {
    /// The cell text: the host, "local", or a dash when unknown.
    pub fn text(self) -> String {
        match self {
            Self::Pulled(host) => host.to_owned(),
            Self::Local => "local".into(),
            Self::Unknown => "—".into(),
        }
    }

    /// The cell's tooltip.
    pub fn hint(self) -> String {
        match self {
            Self::Pulled(host) => format!("Pulled from {host}"),
            Self::Local => "Made in this library".into(),
            Self::Unknown => "Source not recorded: the tag was written before the library recorded where tags come from".into(),
        }
    }
}

/// The source the store records on a local tag's index descriptor: the
/// registry a pull wrote it from ([`image_store::PULL_REGISTRY`]), or that a
/// write here made it ([`image_store::MADE_HERE`]); a tag with neither is
/// unknown.
pub fn source(descriptor: &Descriptor) -> Source<'_> {
    let pulled = descriptor
        .annotations
        .get(image_store::PULL_REGISTRY)
        .filter(|host| Store::validate_registry_host(host).is_ok());
    let made_here = descriptor
        .annotations
        .get(image_store::MADE_HERE)
        .is_some_and(|value| value == "true");
    match pulled {
        Some(host) => Source::Pulled(host),
        None if made_here => Source::Local,
        None => Source::Unknown,
    }
}
/// One manifest of one repository as the library lists it, with every tag
/// there that names it.
pub struct Listed<'a> {
    pub tag: &'a Tag,
    pub aliases: Vec<String>,
}
impl Listed<'_> {
    fn names(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.tag.name).chain(&self.aliases)
    }
    /// The summary search matched one of the row's tags.
    pub fn matched(&self, matches: &BTreeSet<String>) -> bool {
        self.names().any(|name| matches.contains(name))
    }
    /// Before its summary exists a row matches the search by its tag names.
    pub fn named(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.names().any(|name| name.to_lowercase().contains(&text))
    }
    pub fn references(&self) -> String {
        let name = &self.tag.name;
        let repository = name.rsplit_once(':').map_or(name.as_str(), |(r, _)| r);
        format!("{repository}:{}", tags(name, &self.aliases))
    }
    pub fn row(&self, cache: &SummaryCache) -> ImageRow {
        let digest = &self.tag.manifest.digest;
        ImageRow {
            reference: self.tag.name.clone(),
            aliases: self.aliases.clone(),
            descriptor: self.tag.manifest.clone(),
            summary: cache.summary(digest).cloned(),
            failure: cache.failure(digest).map(|f| f.reason.clone()),
        }
    }
}
/// Tags of one repository that name the same manifest, such as a version tag
/// and its `latest` alias, or a set's revision tag and its release alias,
/// form one row. A version tag or a revision tag (`1.4-r1`) leads its row
/// because it names that manifest forever (machine-images §3 and §5.2); the
/// other tags follow by name. Rows keep the order of their first tag in
/// `tags`.
pub fn listed(tags: &[Tag]) -> Vec<Listed<'_>> {
    let mut rows: Vec<Vec<&Tag>> = Vec::new();
    let mut index: BTreeMap<(&str, &Digest), usize> = BTreeMap::new();
    for tag in tags {
        let repository = tag
            .name
            .rsplit_once(':')
            .map_or(tag.name.as_str(), |(r, _)| r);
        match index.entry((repository, &tag.manifest.digest)) {
            std::collections::btree_map::Entry::Occupied(row) => rows[*row.get()].push(tag),
            std::collections::btree_map::Entry::Vacant(row) => {
                row.insert(rows.len());
                rows.push(vec![tag]);
            }
        }
    }
    let alias = |tag: &Tag| {
        !tag.name.rsplit_once(':').is_some_and(|(_, version)| {
            Store::is_version_tag(version) || image_store::tag::is_revision(version)
        })
    };
    rows.into_iter()
        .map(|mut tags| {
            tags.sort_by(|a, b| (alias(a), &a.name).cmp(&(alias(b), &b.name)));
            Listed {
                tag: tags[0],
                aliases: tags[1..].iter().map(|t| t.name.clone()).collect(),
            }
        })
        .collect()
}

/// Remove the row's acting tag, if it still names the row's manifest.
pub fn delete_tag(store: &Store, row: &ImageRow) -> Result<(), String> {
    store
        .remove_tag(&row.reference, &row.descriptor.digest)
        .map_err(|e| e.to_string())
}

/// The host's current time as a machine's last run is recorded.
pub fn host_last_run() -> String {
    // This timestamp is local catalogue metadata and never enters virtual time.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(|_| "Unknown".into(), |d| last_run_utc(d.as_secs()))
}

/// A byte count in the largest binary unit it reaches, to one decimal.
pub fn human_size(bytes: u64) -> String {
    for (unit, divisor) in [
        ("GiB", 1u64 << 30),
        ("MiB", 1u64 << 20),
        ("KiB", 1u64 << 10),
    ] {
        if bytes >= divisor {
            return format!("{:.1} {unit}", bytes as f64 / divisor as f64);
        }
    }
    format!("{bytes} B")
}
/// A Unix time in seconds as `YYYY-MM-DD HH:MM UTC`, by the proleptic
/// Gregorian calendar.
pub(crate) fn last_run_utc(seconds: u64) -> String {
    let days = i128::from(seconds / 86400) + 719468;
    let era = days / 146097;
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = march_month + if march_month < 10 { 3 } else { -9 };
    year += i128::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        (seconds / 3600) % 24,
        (seconds / 60) % 60
    )
}

/// A long hex digest shortened to its first eight and last four digits.
pub fn short(hash: &str) -> String {
    if hash.len() > 12 {
        format!("{}…{}", &hash[..8], &hash[hash.len() - 4..])
    } else {
        hash.into()
    }
}

/// What the tests of the views built on the listing need from it.
#[cfg(feature = "test-support")]
pub mod test_support {
    /// A Unix time in seconds as a machine's last run is shown.
    pub fn last_run_utc(seconds: u64) -> String {
        super::last_run_utc(seconds)
    }
}
