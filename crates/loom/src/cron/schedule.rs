// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use std::fmt;

use crate::cron::cron_error::CronError;
use crate::cron::days::DAYS;
use crate::cron::months::MONTHS;
use crate::cron::parse_field::parse_field;

/// A parsed schedule. Each field is a 64-bit set of allowed values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub(crate) minute: u64,
    pub(crate) hour: u64,
    pub(crate) dom: u64,
    pub(crate) month: u64,
    pub(crate) dow: u64,
    pub(crate) dom_star: bool,
    pub(crate) dow_star: bool,
    pub(crate) source: String,
}

impl fmt::Display for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl Schedule {
    pub fn parse(expr: &str) -> Result<Schedule, CronError> {
        let expr = expr.trim();
        let expanded = match expr {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronError(format!(
                "expected 5 fields, found {} in '{expr}'",
                fields.len()
            )));
        }
        let minute = parse_field(fields[0], 0, 59, None)?;
        let hour = parse_field(fields[1], 0, 23, None)?;
        let dom = parse_field(fields[2], 1, 31, None)?;
        let month = parse_field(fields[3], 1, 12, Some(&MONTHS))?;
        let mut dow = parse_field(fields[4], 0, 7, Some(&DAYS))?;
        // 7 is Sunday too.
        if dow & (1 << 7) != 0 {
            dow |= 1;
            dow &= !(1u64 << 7);
        }
        Ok(Schedule {
            minute,
            hour,
            dom,
            month,
            dow,
            dom_star: fields[2] == "*",
            dow_star: fields[4] == "*",
            source: expr.to_owned(),
        })
    }

    pub(crate) fn day_matches(&self, t: &DateTime<Utc>) -> bool {
        let dom_ok = self.dom & (1 << t.day()) != 0;
        let dow_ok = self.dow & (1 << t.weekday().num_days_from_sunday()) != 0;
        match (self.dom_star, self.dow_star) {
            (true, true) => true,
            (true, false) => dow_ok,
            (false, true) => dom_ok,
            (false, false) => dom_ok || dow_ok,
        }
    }

    /// Does the schedule fire at this minute?
    pub fn matches(&self, t: &DateTime<Utc>) -> bool {
        self.month & (1 << t.month()) != 0
            && self.day_matches(t)
            && self.hour & (1 << t.hour()) != 0
            && self.minute & (1 << t.minute()) != 0
    }

    /// The first firing strictly after `after`. `None` only for schedules
    /// that can never fire (e.g. `0 0 30 2 *`).
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut t = Utc
            .with_ymd_and_hms(
                after.year(),
                after.month(),
                after.day(),
                after.hour(),
                after.minute(),
                0,
            )
            .single()?
            + Duration::minutes(1);
        // Bounded: a valid schedule fires within 8 years (leap day every 4,
        // plus dom/dow combos).
        let limit = after + Duration::days(366 * 8);
        while t < limit {
            if self.month & (1 << t.month()) == 0 {
                let (y, m) = if t.month() == 12 {
                    (t.year() + 1, 1)
                } else {
                    (t.year(), t.month() + 1)
                };
                t = Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0).single()?;
                continue;
            }
            if !self.day_matches(&t) {
                t = Utc
                    .with_ymd_and_hms(t.year(), t.month(), t.day(), 0, 0, 0)
                    .single()?
                    + Duration::days(1);
                continue;
            }
            if self.hour & (1 << t.hour()) == 0 {
                t = Utc
                    .with_ymd_and_hms(t.year(), t.month(), t.day(), t.hour(), 0, 0)
                    .single()?
                    + Duration::hours(1);
                continue;
            }
            if self.minute & (1 << t.minute()) == 0 {
                t += Duration::minutes(1);
                continue;
            }
            return Some(t);
        }
        None
    }
}
