// SPDX-License-Identifier: AGPL-3.0-or-later
//! The media picker: one Insert… per floppy drive, as the approved
//! media-picker mock-up draws it; machine-images §5.2 and §7.
//!
//! The picker's state and view live in `media_ui::media_picker` and are
//! re-exported here, under the path the shell names them by.

pub(crate) use media_ui::media_picker::*;

// The test that checks the picker's size with the library's modal helper,
// which stays with the desktop's library tests.
#[cfg(test)]
pub(crate) mod tests {
    use media_ui::media_picker::test_support::{library, machine, M144};
    pub(crate) use media_ui::media_picker::test_support::{set, set_outcome};
    use media_ui::media_picker::{show, CatalogueDisk, Picker, Source, When};

    /// The picker keeps one size frame after frame, in both sources, with
    /// more sets than its list shows and every list part open, so its rows
    /// stay where the pointer is and Insert stays on screen.
    #[test]
    fn the_picker_keeps_its_size_frame_after_frame() {
        let mut sets = library();
        for n in 0..20 {
            sets.push(set(
                &format!("Shareware {n}"),
                "Apogee Software",
                1990 + n,
                "platformer",
                M144(),
                1,
            ));
        }
        let mut target = machine(None, When::Now);
        target.drives[1].catalogue = vec![CatalogueDisk {
            name: "Blank work disk".into(),
            format: M144(),
            write: "changes kept in DOS games".into(),
            inserted: true,
            index: 0,
            set: None,
        }];
        for source in [Source::Library, Source::Machine] {
            for classic in [true, false] {
                let mut picker = Picker::new(vec![target.clone()], 0, 1, sets.clone(), None);
                picker.show_misfits = true;
                picker.remote_preview = true;
                picker.set_source(source);
                crate::library_test_support::settled_modal(
                    egui::Id::unique("media-picker"),
                    egui::vec2(1040.0, 700.0),
                    |ctx| assert_eq!(show(ctx, &mut picker, classic), None),
                );
            }
        }
    }
}
