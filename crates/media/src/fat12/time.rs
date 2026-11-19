// SPDX-License-Identifier: AGPL-3.0-or-later
//! Directory-entry times; Microsoft FAT Specification, FAT Directory
//! Structure, Date and Time Formats: a date of bits 15–9 years from 1980,
//! 8–5 month, 4–0 day, and a time of bits 15–11 hours, 10–5 minutes, 4–0
//! seconds halved. Times are local, without a zone, as DOS keeps them.

use std::fmt;
use std::str::FromStr;

/// A local date and time a FAT directory entry can hold exactly: a year from
/// 1980 to 2107 and an even second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DosTime {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

impl DosTime {
    /// 1980-01-01 00:00:00, the earliest time an entry holds: the builder's
    /// time for everything that states none.
    pub const EPOCH: Self = Self {
        year: 1980,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };

    /// A checked time. FAT keeps seconds in two-second steps, so an odd
    /// second is refused rather than rounded.
    pub fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Result<Self, &'static str> {
        if !(1980..=2107).contains(&year) {
            return Err("a FAT time's year must be from 1980 to 2107");
        }
        if !(1..=12).contains(&month) || day == 0 || day > days_in(year, month) {
            return Err("no such date");
        }
        if hour > 23 || minute > 59 || second > 59 {
            return Err("no such time of day");
        }
        if !second.is_multiple_of(2) {
            return Err("FAT keeps seconds in two-second steps; the second must be even");
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }

    /// The time a directory entry's date and time words hold, or `None`
    /// when they hold no valid time (a zero date, a day the month lacks, an
    /// hour past 23): the inverse of the words the builder writes.
    pub fn from_words(date: u16, time: u16) -> Option<Self> {
        Self::new(
            1980 + (date >> 9),
            ((date >> 5) & 0x0F) as u8,
            (date & 0x1F) as u8,
            (time >> 11) as u8,
            ((time >> 5) & 0x3F) as u8,
            ((time & 0x1F) * 2) as u8,
        )
        .ok()
    }

    /// The year, 1980 to 2107.
    pub const fn year(self) -> u16 {
        self.year
    }

    /// The month, 1 to 12.
    pub const fn month(self) -> u8 {
        self.month
    }

    /// The day of the month, from 1.
    pub const fn day(self) -> u8 {
        self.day
    }

    /// The hour, 0 to 23.
    pub const fn hour(self) -> u8 {
        self.hour
    }

    /// The minute, 0 to 59.
    pub const fn minute(self) -> u8 {
        self.minute
    }

    /// The second, even, 0 to 58.
    pub const fn second(self) -> u8 {
        self.second
    }

    /// Seconds from 1970-01-01 00:00:00 to this time read as UTC. A FAT
    /// time has no zone; taking it as UTC gives one instant on every host.
    pub fn unix_seconds(self) -> u64 {
        // Days from the civil date (H. Hinnant, "chrono-Compatible Low-Level
        // Date Algorithms", days_from_civil), for years from 1970.
        let (year, month) = (i64::from(self.year), i64::from(self.month));
        let y = if month <= 2 { year - 1 } else { year };
        let era = y / 400;
        let yoe = y - era * 400;
        let mp = (month + 9) % 12;
        let doy = (153 * mp + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        (days * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)) as u64
    }

    /// The entry's date word.
    pub(super) fn date(self) -> u16 {
        ((self.year - 1980) << 9) | (u16::from(self.month) << 5) | u16::from(self.day)
    }

    /// The entry's time word.
    pub(super) fn time(self) -> u16 {
        (u16::from(self.hour) << 11) | (u16::from(self.minute) << 5) | u16::from(self.second / 2)
    }
}

fn days_in(year: u16, month: u8) -> u8 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `YYYY-MM-DDTHH:MM:SS`, the form of a TOML local date-time.
impl FromStr for DosTime {
    type Err = &'static str;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bytes = text.as_bytes();
        let shape = bytes.len() == 19
            && bytes.iter().enumerate().all(|(at, &byte)| match at {
                4 | 7 => byte == b'-',
                10 => byte == b'T',
                13 | 16 => byte == b':',
                _ => byte.is_ascii_digit(),
            });
        if !shape {
            return Err("expected a local date-time YYYY-MM-DDTHH:MM:SS");
        }
        let number = |range: std::ops::Range<usize>| {
            text[range]
                .bytes()
                .fold(0u16, |value, digit| value * 10 + u16::from(digit - b'0'))
        };
        Self::new(
            number(0..4),
            number(5..7) as u8,
            number(8..10) as u8,
            number(11..13) as u8,
            number(14..16) as u8,
            number(17..19) as u8,
        )
    }
}

impl fmt::Display for DosTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}
