// SPDX-License-Identifier: AGPL-3.0-or-later
//! The IBM PC compatible needs vocabulary: the kinds of feature a title may
//! need from the platform family and their values; recipes §6.7, accepted by
//! the maintainer on 2026-11-17.
//!
//! A title states features, never machine models: a CPU class, a coprocessor,
//! a display adapter class, a sound device, memory and input devices. The
//! values name the hardware classes DOS software of the period asks for on
//! its box. `none`, the shared rule's value of every choice kind, means the
//! title also works without any feature of that kind.
//!
//! The vocabulary is **append-only**: a kind or value, once released, keeps
//! its name and meaning for good, because published metadata and recipes
//! name it (recipes §9, row 7). A new value is appended to its list; nothing
//! is renamed, reordered or removed. Writers refuse a kind or value missing
//! here ([`image_store::needs::check`]); readers never consult it, so a
//! title that names a value a later release adds still runs here.
//! Whether a machine serves a statement (a VGA card for a title written for
//! EGA, say) is the provider's judgement, made by the library, not by this
//! table.

use image_store::needs::{Form, Kind, Vocabulary};

/// The kinds, in the order they were accepted.
pub const KINDS: &[Kind] = &[
    // A CPU class, oldest first; `286+` is "an 80286 or later".
    Kind {
        name: "cpu",
        form: Form::Ordered(&["8088", "286", "386", "486", "pentium"]),
    },
    // A numeric coprocessor.
    Kind {
        name: "fpu",
        form: Form::Choice(&["none", "287", "387"]),
    },
    // A display adapter class.
    Kind {
        name: "video",
        form: Form::Choice(&["mda", "hercules", "cga", "ega", "mcga", "vga", "svga"]),
    },
    // A sound device: the PC speaker, Tandy sound, an AdLib card, the Sound
    // Blaster family, the Gravis UltraSound and a Roland MT-32.
    Kind {
        name: "sound",
        form: Form::Choice(&[
            "pc-speaker",
            "tandy",
            "adlib",
            "sb",
            "sbpro",
            "sb16",
            "gus",
            "mt32",
            "none",
        ]),
    },
    // Conventional memory, a minimum such as `640K`.
    Kind {
        name: "memory",
        form: Form::Quantity,
    },
    // Expanded memory (EMS), a minimum.
    Kind {
        name: "ems",
        form: Form::Quantity,
    },
    // Extended memory (XMS), a minimum.
    Kind {
        name: "xms",
        form: Form::Quantity,
    },
    // Input devices beyond the keyboard.
    Kind {
        name: "input",
        form: Form::Choice(&["mouse", "joystick"]),
    },
];

/// The vocabulary of [`crate::PLATFORM`].
pub struct PcNeeds;

impl Vocabulary for PcNeeds {
    fn platform(&self) -> &str {
        crate::PLATFORM
    }
    fn kinds(&self) -> &[Kind] {
        KINDS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_store::needs::{check, Needs};

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

    /// The vocabulary accepted on 2026-11-17, frozen: a later release may only
    /// append kinds after it and values after each kind's list.
    #[test]
    fn the_accepted_vocabulary_is_a_frozen_prefix() {
        let accepted: &[(&str, Form)] = &[
            (
                "cpu",
                Form::Ordered(&["8088", "286", "386", "486", "pentium"]),
            ),
            ("fpu", Form::Choice(&["none", "287", "387"])),
            (
                "video",
                Form::Choice(&["mda", "hercules", "cga", "ega", "mcga", "vga", "svga"]),
            ),
            (
                "sound",
                Form::Choice(&[
                    "pc-speaker",
                    "tandy",
                    "adlib",
                    "sb",
                    "sbpro",
                    "sb16",
                    "gus",
                    "mt32",
                    "none",
                ]),
            ),
            ("memory", Form::Quantity),
            ("ems", Form::Quantity),
            ("xms", Form::Quantity),
            ("input", Form::Choice(&["mouse", "joystick"])),
        ];
        assert!(KINDS.len() >= accepted.len());
        for (kind, (name, form)) in KINDS.iter().zip(accepted) {
            assert_eq!(kind.name, *name);
            match (kind.form, form) {
                (Form::Ordered(now), Form::Ordered(then))
                | (Form::Choice(now), Form::Choice(then)) => {
                    assert!(now.starts_with(then), "{name}: {now:?}")
                }
                (Form::Quantity, Form::Quantity) => {}
                (now, then) => panic!("{name}: {now:?} was {then:?}"),
            }
        }
        assert_eq!(PcNeeds.platform(), "ibm-pc");
    }

    /// Writers reach the vocabulary through the provider traits, whichever
    /// kind of artifact they write.
    #[test]
    fn the_pc_providers_offer_this_vocabulary() {
        use crate::media_set::PcMediaFormats;
        use image_store::{
            media_set::{MediaFormats, Validator},
            ConfigValidator,
        };
        for vocabulary in [
            crate::PcProvider.needs(),
            PcMediaFormats.needs(),
            Validator(&PcMediaFormats).needs(),
        ] {
            let vocabulary = vocabulary.expect("the PC needs vocabulary");
            assert_eq!(vocabulary.platform(), crate::PLATFORM);
            assert_eq!(vocabulary.kinds(), KINDS);
        }
    }

    #[test]
    fn the_recipe_example_and_none_in_every_choice_kind_are_accepted() {
        for good in [
            needs(&[
                ("cpu", &["286+"]),
                ("memory", &["640K"]),
                ("video", &["vga"]),
                ("sound", &["sb", "adlib", "pc-speaker", "none"]),
            ]),
            needs(&[("cpu", &["8088", "286"])]),
            needs(&[("fpu", &["387", "none"])]),
            needs(&[("video", &["ega", "vga", "none"])]),
            needs(&[("input", &["mouse", "none"])]),
            needs(&[("ems", &["1M"]), ("xms", &["4096K"])]),
        ] {
            assert!(check(&good, &PcNeeds).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn an_unknown_kind_or_value_is_refused() {
        for (bad, message) in [
            (
                needs(&[("chipset", &["aga"])]),
                "need kind \"chipset\" is not one of cpu, fpu, video, sound, memory, ems, xms, input",
            ),
            (
                needs(&[("video", &["8514"])]),
                "need video: \"8514\" is not one of mda, hercules, cga, ega, mcga, vga, svga, none",
            ),
            (needs(&[("cpu", &["v20"])]), "need cpu: \"v20\""),
            (needs(&[("cpu", &["none"])]), "need cpu: \"none\""),
            (needs(&[("sound", &["sb16+"])]), "need sound: \"sb16+\""),
            (needs(&[("memory", &["640"])]), "need memory: \"640\""),
            (needs(&[("input", &["keyboard"])]), "need input: \"keyboard\""),
        ] {
            let error = check(&bad, &PcNeeds).unwrap_err().to_string();
            assert!(error.contains(message), "{bad:?}: {error}");
        }
    }
}
