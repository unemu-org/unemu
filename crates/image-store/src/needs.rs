// SPDX-License-Identifier: AGPL-3.0-or-later
//! What a title needs from its platform family, stated as features, never as
//! machine models; recipes §6.7, machine-images §2.1.
//!
//! A title record's `needs` maps a **kind** to its alternatives, best first.
//! Every kind stated is required and any one of its values satisfies it; a
//! kind not stated is unknown, not absent. Shared code knows kinds only as
//! data: the platform provider supplies its vocabulary through
//! [`Vocabulary`], and [`check`] refuses a kind or value it does not know
//! when metadata is written or a recipe is checked. Readers never consult
//! the vocabulary, so a value a later release adds never stops an artifact
//! from running; the decoder checks the shape alone ([`shape`]).
//!
//! The manifest repeats the needs of an artifact's title in the annotation
//! [`ANNOTATION`], a compact form ([`annotation`]) a registry browser reads
//! without fetching the metadata (machine-images §2).

use crate::{invalid, metadata::Metadata, Result};
use std::collections::BTreeMap;

/// Kind to alternatives, in the author's order of preference.
pub type Needs = BTreeMap<String, Vec<String>>;

/// Manifest annotation: the needs of the artifact's one title, in the
/// compact form of [`annotation`]. A hint for browsing, never an authority:
/// the metadata is.
pub const ANNOTATION: &str = "unemu.needs";

/// The value of every choice kind meaning "also works without any feature of
/// this kind": `sound = ["sb", "none"]` reads "uses a Sound Blaster, runs
/// without sound".
pub const NONE: &str = "none";

/// How a provider reads the values of one kind (recipes §6.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A point on the provider's scale, oldest first. A trailing `+` means
    /// "this one or a later one"; without it the value means exactly that
    /// one, for titles that break on a faster machine.
    Ordered(&'static [&'static str]),
    /// A whole number of KiB (`K`) or MiB (`M`), read as a minimum. A
    /// quantity states one value: a list of minimums would mean nothing.
    Quantity,
    /// Names from the provider's list, and [`NONE`].
    Choice(&'static [&'static str]),
}

/// One kind of a provider's vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    pub name: &'static str,
    pub form: Form,
}

/// A platform provider's needs vocabulary. It is append-only: a kind or a
/// value, once released, keeps its name and meaning for good, because
/// published metadata and recipes name it.
pub trait Vocabulary {
    /// The platform family the vocabulary belongs to.
    fn platform(&self) -> &str;
    /// Every kind the provider knows.
    fn kinds(&self) -> &[Kind];
}

/// A kind: lowercase ASCII letters, digits and `_`, starting with a letter,
/// at most 32 bytes (`cpu`, `tv_system`).
pub fn valid_kind(kind: &str) -> bool {
    kind.len() <= 32
        && kind.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A value: ASCII letters, digits, `+`, `.`, `_` and `-`, starting with a
/// letter or digit, at most 32 bytes (`286+`, `640K`, `pc-speaker`). The
/// separators of the compact form never occur in one.
pub fn valid_value(value: &str) -> bool {
    value.len() <= 32
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+._-".contains(&b))
}

/// The shape every reader checks: valid kinds, and for each at least one
/// valid value, none repeated. The vocabulary is not consulted.
pub fn shape(needs: &Needs) -> Result<()> {
    for (kind, values) in needs {
        if !valid_kind(kind) {
            return Err(invalid(format!(
                "need kind {kind:?}: lowercase ASCII letters, digits and '_', starting with a letter, at most 32 characters"
            )));
        }
        if values.is_empty() {
            return Err(invalid(format!("need {kind} states no value")));
        }
        for (index, value) in values.iter().enumerate() {
            if !valid_value(value) {
                return Err(invalid(format!(
                    "need {kind}: {value:?}: ASCII letters, digits, '+', '.', '_' and '-', starting with a letter or digit, at most 32 characters"
                )));
            }
            if values[..index].contains(value) {
                return Err(invalid(format!("need {kind}: {value:?} is repeated")));
            }
        }
    }
    Ok(())
}

/// Refuse a kind or value `vocabulary` does not know (recipes §6.11, rule 7).
pub fn check(needs: &Needs, vocabulary: &dyn Vocabulary) -> Result<()> {
    shape(needs)?;
    for (kind, values) in needs {
        let Some(known) = vocabulary.kinds().iter().find(|k| k.name == kind) else {
            let names: Vec<&str> = vocabulary.kinds().iter().map(|k| k.name).collect();
            return Err(invalid(format!(
                "need kind {kind:?} is not one of {}",
                names.join(", ")
            )));
        };
        if known.form == Form::Quantity && values.len() > 1 {
            return Err(invalid(format!(
                "need {kind}: a quantity is one minimum, not {} values",
                values.len()
            )));
        }
        for value in values {
            if !known_value(known.form, value) {
                return Err(invalid(format!(
                    "need {kind}: {value:?} is not {}",
                    expected(known.form)
                )));
            }
        }
    }
    Ok(())
}

/// [`check`] for every title of `metadata` against its platform provider's
/// vocabulary ([`crate::ConfigValidator::needs`]), whose platform must be
/// the metadata's. A provider without a vocabulary takes no needs.
pub fn check_metadata(metadata: &Metadata, vocabulary: Option<&dyn Vocabulary>) -> Result<()> {
    if metadata.titles.iter().all(|t| t.needs.is_empty()) {
        return Ok(());
    }
    let Some(vocabulary) = vocabulary else {
        return Err(invalid(format!(
            "needs: platform {} has no needs vocabulary",
            metadata.platform
        )));
    };
    if !crate::platform::same(&metadata.platform, vocabulary.platform()) {
        return Err(invalid(format!(
            "needs of platform {} checked against the vocabulary of {}",
            metadata.platform,
            vocabulary.platform()
        )));
    }
    for title in &metadata.titles {
        check(&title.needs, vocabulary)?;
    }
    Ok(())
}

fn known_value(form: Form, value: &str) -> bool {
    match form {
        Form::Ordered(scale) => scale.contains(&value.strip_suffix('+').unwrap_or(value)),
        Form::Quantity => quantity(value),
        Form::Choice(names) => value == NONE || names.contains(&value),
    }
}

/// A positive whole number without a leading zero, of at most seven digits,
/// then `K` or `M`.
fn quantity(value: &str) -> bool {
    let Some(number) = value.strip_suffix(['K', 'M']) else {
        return false;
    };
    (1..=7).contains(&number.len())
        && number.bytes().all(|b| b.is_ascii_digit())
        && !number.starts_with('0')
}

fn expected(form: Form) -> String {
    match form {
        Form::Ordered(scale) => format!(
            "one of {}, each optionally followed by + (or later)",
            scale.join(", ")
        ),
        Form::Quantity => "a whole number followed by K or M".into(),
        Form::Choice(names) => {
            let mut names: Vec<&str> = names.to_vec();
            if !names.contains(&NONE) {
                names.push(NONE);
            }
            format!("one of {}", names.join(", "))
        }
    }
}

/// The compact form of the [`ANNOTATION`]: each kind in sorted order as
/// `kind=value,value`, the values in the author's order, kinds joined by
/// `;` (`cpu=286+;memory=640K;sound=sb,adlib,none;video=vga`). `None` when
/// nothing is stated, so a title without needs adds no annotation.
pub fn annotation(needs: &Needs) -> Option<String> {
    (!needs.is_empty()).then(|| {
        needs
            .iter()
            .map(|(kind, values)| format!("{kind}={}", values.join(",")))
            .collect::<Vec<_>>()
            .join(";")
    })
}

/// Read the compact form leniently: `None` for text that is not exactly
/// what [`annotation`] writes for needs of a valid shape, which a reader then
/// ignores, as it ignores any annotation it cannot use.
pub fn parse_annotation(text: &str) -> Option<Needs> {
    let mut needs = Needs::new();
    for part in text.split(';') {
        let (kind, values) = part.split_once('=')?;
        let values: Vec<String> = values.split(',').map(str::to_owned).collect();
        if needs.insert(kind.to_owned(), values).is_some() {
            return None;
        }
    }
    shape(&needs).ok()?;
    (annotation(&needs).as_deref() == Some(text)).then_some(needs)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Test;
    impl Vocabulary for Test {
        fn platform(&self) -> &str {
            "c64"
        }
        fn kinds(&self) -> &[Kind] {
            &[
                Kind {
                    name: "cpu",
                    form: Form::Ordered(&["slow", "fast"]),
                },
                Kind {
                    name: "memory",
                    form: Form::Quantity,
                },
                Kind {
                    name: "sound",
                    form: Form::Choice(&["chip", "sampler"]),
                },
            ]
        }
    }

    fn needs(pairs: &[(&str, &[&str])]) -> Needs {
        pairs
            .iter()
            .map(|(kind, values)| {
                (
                    (*kind).to_owned(),
                    values.iter().map(|v| (*v).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_vocabulary_accepts_its_forms_and_none_in_every_choice_kind() {
        for good in [
            needs(&[("cpu", &["slow"])]),
            needs(&[("cpu", &["slow+"])]),
            needs(&[("cpu", &["slow", "fast+"])]),
            needs(&[("memory", &["640K"])]),
            needs(&[("memory", &["16M"])]),
            needs(&[("memory", &["9999999K"])]),
            needs(&[("sound", &["sampler", "chip", "none"])]),
            needs(&[("sound", &["none"])]),
            Needs::new(),
        ] {
            assert!(check(&good, &Test).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn an_unknown_kind_or_value_is_refused_naming_what_is_known() {
        for (bad, message) in [
            (
                needs(&[("video", &["vga"])]),
                "need kind \"video\" is not one of cpu, memory, sound",
            ),
            (
                needs(&[("sound", &["gus"])]),
                "need sound: \"gus\" is not one of chip, sampler, none",
            ),
            (
                needs(&[("cpu", &["medium+"])]),
                "need cpu: \"medium+\" is not one of slow, fast, each optionally followed by + (or later)",
            ),
            (
                needs(&[("cpu", &["none"])]),
                "need cpu: \"none\" is not one of slow, fast",
            ),
            (
                needs(&[("memory", &["none"])]),
                "need memory: \"none\" is not a whole number followed by K or M",
            ),
            (needs(&[("memory", &["640"])]), "need memory: \"640\""),
            (needs(&[("memory", &["640k"])]), "need memory: \"640k\""),
            (needs(&[("memory", &["0640K"])]), "need memory: \"0640K\""),
            (needs(&[("memory", &["0K"])]), "need memory: \"0K\""),
            (needs(&[("memory", &["10000000K"])]), "need memory: \"10000000K\""),
            (needs(&[("sound", &["chip+"])]), "need sound: \"chip+\""),
            (
                needs(&[("memory", &["640K", "512K"])]),
                "need memory: a quantity is one minimum, not 2 values",
            ),
        ] {
            let error = check(&bad, &Test).unwrap_err().to_string();
            assert!(error.contains(message), "{bad:?}: {error}");
        }
    }

    #[test]
    fn the_shape_is_checked_without_a_vocabulary() {
        for bad in [
            needs(&[("Video", &["vga"])]),
            needs(&[("1cpu", &["286"])]),
            needs(&[("", &["286"])]),
            needs(&[("tv-system", &["pal"])]),
            needs(&[(&"k".repeat(33), &["x"])]),
            needs(&[("cpu", &[])]),
            needs(&[("cpu", &["286", "286"])]),
            needs(&[("cpu", &[""])]),
            needs(&[("cpu", &["+286"])]),
            needs(&[("sound", &["sb,adlib"])]),
            needs(&[("sound", &["sb;video=vga"])]),
            needs(&[("sound", &["a=b"])]),
            needs(&[("sound", &["pc speaker"])]),
            needs(&[("sound", &[&"v".repeat(33)])]),
        ] {
            assert!(shape(&bad).is_err(), "{bad:?}");
            assert!(check(&bad, &Test).is_err(), "{bad:?}");
        }
        // A kind the shape allows and no vocabulary knows is still a valid
        // shape: readers keep it, only writers refuse it.
        let future = needs(&[("tv_system", &["pal", "ntsc"])]);
        assert!(shape(&future).is_ok());
        assert!(check(&future, &Test).is_err());
    }

    #[test]
    fn metadata_needs_take_their_own_platforms_vocabulary_only() {
        use crate::metadata::{Metadata, Title};
        let mut metadata = Metadata::new("c64");
        metadata.titles.push(Title {
            id: "a".into(),
            title: "A".into(),
            ..Title::default()
        });
        assert!(check_metadata(&metadata, None).is_ok());
        metadata.titles[0].needs = needs(&[("cpu", &["slow"])]);
        assert!(check_metadata(&metadata, Some(&Test)).is_ok());
        assert_eq!(
            check_metadata(&metadata, None).unwrap_err().to_string(),
            "invalid image: needs: platform c64 has no needs vocabulary"
        );
        metadata.platform = "nes".into();
        assert!(check_metadata(&metadata, Some(&Test)).is_err());
    }

    #[test]
    fn the_annotation_is_compact_sorted_by_kind_and_keeps_value_order() {
        let stated = needs(&[
            ("sound", &["sb", "adlib", "pc-speaker", "none"]),
            ("cpu", &["286+"]),
            ("video", &["vga"]),
            ("memory", &["640K"]),
        ]);
        let text = annotation(&stated).unwrap();
        assert_eq!(
            text,
            "cpu=286+;memory=640K;sound=sb,adlib,pc-speaker,none;video=vga"
        );
        assert_eq!(parse_annotation(&text), Some(stated));
        assert_eq!(annotation(&Needs::new()), None);
        for bad in [
            "",
            "cpu",
            "cpu=",
            "cpu=286,",
            "cpu=286;",
            ";cpu=286",
            "video=vga;cpu=286",
            "cpu=286;cpu=386",
            "cpu=286,286",
            "Cpu=286",
            "cpu=286 ",
            "cpu=2=86",
        ] {
            assert_eq!(parse_annotation(bad), None, "{bad:?}");
        }
    }
}
