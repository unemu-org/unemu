// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic firmware permission for isolated desktop integration tests.
//! IBM PC AT Technical Reference, BIOS: production compatibility checks remain
//! mandatory; this scoped permission admits invented bytes only on this thread.
use std::cell::Cell;
thread_local! {static SYNTHETIC:Cell<bool>=const{Cell::new(false)};}
pub(crate) fn permitted() -> bool {
    SYNTHETIC.with(Cell::get)
}
/// Run a headless integration probe with synthetic, unlisted system firmware.
/// The previous permission is restored even when the probe unwinds.
pub fn with_synthetic_firmware<T>(probe: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            SYNTHETIC.with(|value| value.set(self.0));
        }
    }
    let _restore = Restore(SYNTHETIC.with(|value| value.replace(true)));
    probe()
}
