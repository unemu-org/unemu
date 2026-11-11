// SPDX-License-Identifier: AGPL-3.0-or-later
//! Passive BIOS/DOS screen assertions; execution uses the shipped CLI helpers.
//! See status.md §Port 61h timing re-baseline for the recorded comparisons.

use cli::{BiosRun, ScreenStamp};

/// Stable FNV-1a of canonical console rows, including empty rows.
pub fn screen_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

/// Only guest-rendered RTC/time fields may vary between the timing baselines.
pub fn logical_hash(text: &str) -> u64 {
    let rows: Vec<_> = text
        .split('\n')
        .map(|line| {
            if line.starts_with("RTC (Real Time Clock):") {
                "RTC (Real Time Clock): <clock>"
            } else if line.starts_with("Current time is ") {
                "Current time is <clock>"
            } else {
                line
            }
        })
        .collect();
    screen_hash(&rows.join("\n"))
}

/// Pin timing and complete text, and prove the old logical screen survives
/// after replacing only the guest's displayed RTC/time fields.
pub fn assert_golden(
    run: &BiosRun,
    retired: u64,
    virtual_time: u64,
    text_hash: u64,
    old_logical_hash: u64,
) {
    eprintln!(
        "capture {:?}, POST={:02x?}, COM1={:02x?}\n--- MDA ---\n{}\n--- VGA ---\n{}",
        run.screen_stamp,
        run.post,
        run.com1,
        run.mda.to_dump(),
        run.screen.to_dump()
    );
    assert_eq!(
        run.screen_stamp,
        Some(ScreenStamp {
            virtual_time,
            retired
        })
    );
    assert_eq!(screen_hash(&run.mda.to_dump()), text_hash);
    assert_eq!(logical_hash(&run.mda.to_dump()), old_logical_hash);
    assert!(run.screen.rows_as_strings().iter().all(String::is_empty));
}
