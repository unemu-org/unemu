// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machines: named, local, mutable instances of one image manifest;
//! machine-images §3.2.
//!
//! A machine record lives in `local/machines.toml` (schema 1), keyed by its
//! workspace id: the directory under `workspaces/` that holds the machine's
//! session pin and journals. Records never leave the local store and are never
//! pushed; images stay immutable and are what moves between stores. The store
//! keeps the record and its invariants (valid ids, display names of 1–64
//! characters that are unique ignoring case); a platform provider owns the
//! machine settings overlay, which the store keeps as an opaque table, and
//! everything that reads or writes the workspace.
//!
//! Edits reread the file under the short index lock and replace it atomically,
//! like `local/library.toml`. A newer or invalid file fails closed and stays
//! untouched rather than losing personal data. Lock order is the store lease,
//! a workspace lock if needed, then the index lock.

use crate::{
    filesystem as host, invalid, store::read_bounded, Descriptor, Result, Store, MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io};

/// Longest display name, in characters.
pub const MAX_NAME_CHARS: usize = 64;

/// How a machine came to exist; shown beside its creation time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Made from an image by New machine from image…, Start or the command line.
    New,
    /// Copied from another machine with its changes.
    Duplicate,
    /// Adopted from a workspace that existed before machines did.
    Migration,
}

/// A medium chosen for a drive of a stopped machine. The next Start puts it in
/// the drive and drops the request; the image never changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingInsert {
    /// The drive as the guest names it, such as `A:`.
    pub drive: String,
    /// The installation media set the medium belongs to, pinned by digest.
    pub set: Descriptor,
    /// The `repository:tag` the set was chosen by, for display only.
    #[serde(default)]
    pub reference: String,
    /// The id of the medium to insert, as the set's configuration names it.
    pub medium: String,
    /// Whether every medium of the set joins the drive's catalogue.
    #[serde(default)]
    pub all: bool,
    /// Whether guest writes go to a journal in the machine's workspace;
    /// otherwise the medium is read-only.
    #[serde(default)]
    pub journal: bool,
}

/// One machine. Times are host text supplied by the caller (local library
/// metadata, like an image's last run); nothing here reads a clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    /// Display name: free text, unique ignoring case. Rename changes only this.
    pub name: String,
    /// The image manifest the machine runs, pinned by digest.
    pub image: Descriptor,
    /// The `repository:tag` the machine was made from. Empty when no tag named
    /// the image. A machine never follows a tag that later moves.
    #[serde(default)]
    pub source: String,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub pending: Vec<PendingInsert>,
    /// Provider hardware tables that replace the image's from the next Start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<toml::Table>,
    /// The workspace this machine was adopted from, when migration moved it to
    /// a new id. Kept so an interrupted move completes on the next migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_from: Option<String>,
}

impl Machine {
    /// A machine just made from `image`, with no local data yet.
    pub fn new(name: &str, image: Descriptor, source: &str, origin: Origin) -> Self {
        Self {
            name: name.trim().into(),
            image,
            source: source.into(),
            origin,
            created: None,
            last_run: None,
            favorite: false,
            labels: Vec::new(),
            pending: Vec::new(),
            settings: None,
            migrated_from: None,
        }
    }
}

/// The versioned machine file. A missing file holds no machines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machines {
    schema: u32,
    #[serde(default)]
    machines: BTreeMap<String, Machine>,
}

impl Default for Machines {
    fn default() -> Self {
        Self {
            schema: 1,
            machines: BTreeMap::new(),
        }
    }
}

/// Workspace id prefixes the desktop and Gaming mode give their own
/// workspaces. The desktop recognises those workspaces by the prefix alone,
/// so a machine id never starts with one.
pub const RESERVED_PREFIXES: &[&str] = &["regular-", "gaming-", "disposable-", "fresh-"];

/// Whether `id` starts like a desktop or Gaming-mode workspace.
pub fn reserved_id(id: &str) -> bool {
    RESERVED_PREFIXES
        .iter()
        .any(|prefix| id.starts_with(prefix))
}

/// Invisible formatting characters that can make two names look alike or
/// reorder the text around them (Unicode general category Cf, the characters
/// Unicode Technical Standard #39 lists as spoofing risks): soft hyphen,
/// combining grapheme joiner, Arabic letter mark, Hangul fillers, Khmer and
/// Mongolian invisible vowels and selectors, zero-width characters, the
/// bidirectional embeddings, overrides and isolates (Unicode Standard Annex
/// #9), the byte order mark, interlinear annotation controls and tag
/// characters.
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}'
        | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF9}'..='\u{FFFB}'
        | '\u{E0000}'..='\u{E007F}')
}

/// Check a display name and return it trimmed: 1–64 characters, no control
/// or invisible formatting characters.
pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("a machine name cannot be empty"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(invalid(format!(
            "a machine name has at most {MAX_NAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("a machine name cannot contain control characters"));
    }
    if name.chars().any(invisible) {
        return Err(invalid(
            "a machine name cannot contain invisible formatting characters",
        ));
    }
    Ok(name.into())
}

/// Names compare case-folded: uppercased, then lowercased, so that full case
/// mappings such as ß to SS fold together ("Straße" and "STRASSE" are one
/// name), as well as simple ones.
fn fold(name: &str) -> String {
    name.to_uppercase().to_lowercase()
}

fn same_name(a: &str, b: &str) -> bool {
    fold(a) == fold(b)
}

impl Machines {
    pub fn get(&self, id: &str) -> Option<&Machine> {
        self.machines.get(id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Machine> {
        self.machines.get_mut(id)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Machine)> {
        self.machines.iter()
    }
    pub fn len(&self) -> usize {
        self.machines.len()
    }
    pub fn is_empty(&self) -> bool {
        self.machines.is_empty()
    }
    /// The id of the machine with this name, ignoring case.
    pub fn find_name(&self, name: &str) -> Option<&str> {
        let name = name.trim();
        self.machines
            .iter()
            .find(|(_, m)| same_name(&m.name, name))
            .map(|(id, _)| id.as_str())
    }
    /// `base`, or the first free `base (2)`, `base (3)`, … ignoring case. A long
    /// base is shortened so the suffix still fits the name limit.
    pub fn unique_name(&self, base: &str) -> String {
        let base: String = base.trim().chars().take(MAX_NAME_CHARS).collect();
        let base = if base.is_empty() {
            "Machine".to_owned()
        } else {
            base
        };
        if self.find_name(&base).is_none() {
            return base;
        }
        for n in 2u64.. {
            let suffix = format!(" ({n})");
            let kept: String = base
                .chars()
                .take(MAX_NAME_CHARS - suffix.chars().count())
                .collect();
            let candidate = format!("{}{suffix}", kept.trim_end());
            if self.find_name(&candidate).is_none() {
                return candidate;
            }
        }
        unreachable!("an unbounded counter always finds a free name")
    }
    /// Add a machine under `id`. The id must be new and the name free.
    pub fn insert(&mut self, id: &str, machine: Machine) -> Result<()> {
        if self.machines.contains_key(id) {
            return Err(invalid(format!("machine id {id} exists already")));
        }
        self.machines.insert(id.into(), machine);
        let checked = self.validate();
        if checked.is_err() {
            self.machines.remove(id);
        }
        checked
    }
    /// Change only the display name.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<()> {
        let name = validate_name(name)?;
        if let Some(other) = self.find_name(&name).filter(|other| *other != id) {
            return Err(invalid(format!(
                "a machine named \u{201c}{}\u{201d} exists already; names are unique, ignoring case",
                self.machines[other].name
            )));
        }
        self.machines
            .get_mut(id)
            .ok_or_else(|| invalid(format!("no machine {id}")))?
            .name = name;
        Ok(())
    }
    pub fn remove(&mut self, id: &str) -> Option<Machine> {
        self.machines.remove(id)
    }
    /// Every image a machine pins; roots for sweeps of rebuildable caches.
    pub fn images(&self) -> impl Iterator<Item = &Descriptor> {
        self.machines.values().map(|m| &m.image)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(invalid("unsupported machine file schema"));
        }
        let mut names: BTreeMap<String, &str> = BTreeMap::new();
        for (id, machine) in &self.machines {
            if !crate::workspace::valid_name(id) || reserved_id(id) {
                return Err(invalid(format!("invalid machine id {id}")));
            }
            if validate_name(&machine.name)? != machine.name {
                return Err(invalid("a machine name must be trimmed"));
            }
            if let Some(other) = names.insert(fold(&machine.name), id) {
                return Err(invalid(format!(
                    "machines {other} and {id} share the name \u{201c}{}\u{201d}",
                    machine.name
                )));
            }
            if machine
                .migrated_from
                .as_deref()
                .is_some_and(|from| !crate::workspace::valid_name(from))
            {
                return Err(invalid("invalid migrated workspace id"));
            }
        }
        Ok(())
    }
}

/// A workspace id from a display name: lowercase ASCII letters and digits with
/// single hyphens, at most 56 characters, `machine` when nothing is left. A
/// slug that is or starts like a reserved prefix gets `m-` in front, so no
/// numbered variant can take one either.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_end_matches('-').chars().take(56).collect();
    out.truncate(out.trim_end_matches('-').len());
    if out.is_empty() {
        return "machine".into();
    }
    if RESERVED_PREFIXES
        .iter()
        .any(|prefix| out.starts_with(prefix) || out == prefix.trim_end_matches('-'))
    {
        out.insert_str(0, "m-");
    }
    out
}

/// `seconds` since the Unix epoch as `YYYY-MM-DD HH:MM UTC`, the text the
/// library shows for a last run. Callers read the host clock; this does not.
pub fn utc_text(seconds: u64) -> String {
    // Civil-from-days over the proleptic Gregorian calendar, March-based years.
    let days = i128::from(seconds / 86400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = march_month + if march_month < 10 { 3 } else { -9 };
    let year = year_of_era + era * 400 + i128::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        (seconds / 3600) % 24,
        (seconds / 60) % 60
    )
}

impl Store {
    pub fn read_machines(&self) -> Result<Machines> {
        let _lease = self.lease()?;
        self.read_machines_inner()
    }

    fn read_machines_inner(&self) -> Result<Machines> {
        let bytes = match read_bounded(
            &self.root().join("local/machines.toml"),
            64 * MAX_SMALL_BLOB,
        ) {
            Ok(bytes) => bytes,
            Err(crate::Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Machines::default())
            }
            Err(error) => return Err(error),
        };
        let machines: Machines =
            toml::from_str(std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?)
                .map_err(|e| invalid(e.to_string()))?;
        machines.validate()?;
        Ok(machines)
    }

    /// Apply `edit` to the current records under the index lock and replace the
    /// file atomically when they changed. The edit sees fresh data, so edits
    /// from other processes are never lost. It runs while the lock is held: it
    /// must not call store operations that take the index lock.
    pub fn edit_machines<T>(&self, edit: impl FnOnce(&mut Machines) -> Result<T>) -> Result<T> {
        let _lease = self.lease()?;
        let _lock = host::exclusive_lock(&self.root().join("locks/index.lock"))?;
        let old = self.read_machines_inner()?;
        let mut machines = old.clone();
        let result = edit(&mut machines)?;
        machines.validate()?;
        if machines != old {
            let bytes = toml::to_string(&machines).map_err(|e| invalid(e.to_string()))?;
            if bytes.len() as u64 > 64 * MAX_SMALL_BLOB {
                return Err(invalid("machine file exceeds 64 MiB"));
            }
            fs::create_dir_all(self.root().join("local"))?;
            self.atomic_write(&self.root().join("local/machines.toml"), bytes.as_bytes())?;
            host::sync_dir(self.root())?;
        }
        Ok(result)
    }

    /// A free workspace id for a machine named `name`: its slug, or the slug
    /// with `-2`, `-3`, … when a machine or any workspace directory has it.
    pub fn free_machine_id(&self, machines: &Machines, name: &str) -> Result<String> {
        let base = slug(name);
        for n in 1u64.. {
            let id = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            if machines.get(&id).is_none() && !self.workspace_path(&id)?.exists() {
                return Ok(id);
            }
        }
        unreachable!("an unbounded counter always finds a free id")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Digest;

    fn image() -> Descriptor {
        Descriptor::new(crate::media_type::MANIFEST, Digest::of(b"image"), 5)
    }

    #[test]
    fn names_are_trimmed_bounded_and_unique_ignoring_case() {
        let mut machines = Machines::default();
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"x".repeat(65)).is_err());
        assert_eq!(validate_name(&"é".repeat(64)).unwrap().chars().count(), 64);
        assert!(validate_name("bad\nname").is_err());
        for spoof in [
            "evil\u{202E}txt",
            "zero\u{200B}width",
            "\u{FEFF}bom",
            "tag\u{E0041}",
        ] {
            assert!(validate_name(spoof).is_err(), "{spoof:?}");
        }
        machines
            .insert(
                "dos",
                Machine::new(" DOS games ", image(), "a:1", Origin::New),
            )
            .unwrap();
        assert_eq!(machines.get("dos").unwrap().name, "DOS games");
        assert!(machines
            .insert(
                "dos-2",
                Machine::new("dos GAMES", image(), "a:1", Origin::New)
            )
            .is_err());
        assert_eq!(machines.unique_name("DOS Games"), "DOS Games (2)");
        machines
            .insert("strasse", Machine::new("Straße", image(), "", Origin::New))
            .unwrap();
        assert_eq!(machines.find_name("STRASSE"), Some("strasse"));
        assert!(machines
            .insert(
                "strasse-2",
                Machine::new("strasse", image(), "", Origin::New)
            )
            .is_err());
        assert!(machines
            .insert("fresh-copy", Machine::new("Copy", image(), "", Origin::New))
            .is_err());
        let long = "y".repeat(64);
        machines
            .insert("long", Machine::new(&long, image(), "", Origin::New))
            .unwrap();
        let next = machines.unique_name(&long);
        assert_eq!(next.chars().count(), 64);
        assert!(next.ends_with(" (2)"));
        machines
            .insert("other", Machine::new("Other", image(), "", Origin::New))
            .unwrap();
        assert!(machines.rename("other", "dos games").is_err());
        machines.rename("other", "Other").unwrap();
        machines.rename("dos", "dos games").unwrap();
        assert_eq!(machines.get("dos").unwrap().name, "dos games");
        assert!(machines.rename("missing", "Free").is_err());
    }

    #[test]
    fn slugs_and_utc_text_are_stable() {
        assert_eq!(slug("DOS games"), "dos-games");
        assert_eq!(slug("Win 3.1 test"), "win-3-1-test");
        assert_eq!(slug("  ---  "), "machine");
        assert_eq!(slug("Łódź"), "d");
        assert_eq!(slug(&format!("{}-tail", "a".repeat(55))), "a".repeat(55));
        // Desktop and Gaming-mode prefixes are never handed out.
        assert_eq!(slug("Fresh install"), "m-fresh-install");
        assert_eq!(slug("Regular"), "m-regular");
        assert_eq!(slug("Gaming PC"), "m-gaming-pc");
        assert_eq!(slug("disposable"), "m-disposable");
        assert_eq!(slug("Freshly made"), "freshly-made");
        assert_eq!(slug("Regulars"), "regulars");
        assert!(!reserved_id(&format!("{}-2", slug("Regular"))));
        assert_eq!(utc_text(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_text(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc_text(1_794_738_660), "2026-11-15 10:31 UTC");
    }
}
