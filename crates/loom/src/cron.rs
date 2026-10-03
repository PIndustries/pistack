// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A small five-field cron parser: `minute hour day-of-month month day-of-week`.
//!
//! Supported per field: `*`, `n`, `a-b`, `*/s`, `a-b/s`, lists `a,b,c`,
//! month and weekday names (`jan`, `mon`), `7` as Sunday, and the macros
//! `@hourly`, `@daily`/`@midnight`, `@weekly`, `@monthly`, `@yearly`/`@annually`.
//! Day-of-month and day-of-week combine with OR when both are restricted,
//! as in Vixie cron. All times are UTC.

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid cron expression: {0}")]
pub struct CronError(pub String);

/// A parsed schedule. Each field is a 64-bit set of allowed values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    minute: u64,
    hour: u64,
    dom: u64,
    month: u64,
    dow: u64,
    dom_star: bool,
    dow_star: bool,
    source: String,
}

impl fmt::Display for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

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

    fn day_matches(&self, t: &DateTime<Utc>) -> bool {
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

fn parse_field(field: &str, min: u32, max: u32, names: Option<&[&str]>) -> Result<u64, CronError> {
    if field.is_empty() {
        return Err(CronError("empty field".into()));
    }
    let mut set: u64 = 0;
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s
                    .parse()
                    .map_err(|_| CronError(format!("bad step '{s}' in '{field}'")))?;
                if step == 0 {
                    return Err(CronError(format!("step must be > 0 in '{field}'")));
                }
                (r, step)
            }
            None => (part, 1),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (
                parse_value(a, min, max, names)?,
                parse_value(b, min, max, names)?,
            )
        } else {
            let v = parse_value(range, min, max, names)?;
            // `5/15` means "from 5 to max every 15".
            if part.contains('/') {
                (v, max)
            } else {
                (v, v)
            }
        };
        if lo > hi {
            return Err(CronError(format!(
                "range {lo}-{hi} is inverted in '{field}'"
            )));
        }
        let mut v = lo;
        while v <= hi {
            set |= 1 << v;
            v += step;
        }
    }
    Ok(set)
}

fn parse_value(s: &str, min: u32, max: u32, names: Option<&[&str]>) -> Result<u32, CronError> {
    let v = match s.parse::<u32>() {
        Ok(v) => v,
        Err(_) => {
            let lower = s.to_ascii_lowercase();
            let idx = names
                .and_then(|n| {
                    n.iter()
                        .position(|x| lower.starts_with(x) && lower.len() >= 3)
                })
                .ok_or_else(|| CronError(format!("bad value '{s}'")))?;
            // Month names are 1-based; day names are 0-based.
            if min == 1 {
                idx as u32 + 1
            } else {
                idx as u32
            }
        }
    };
    if v < min || v > max {
        return Err(CronError(format!(
            "value {v} out of range {min}-{max} in '{s}'"
        )));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn parses_fields() {
        let s = Schedule::parse("*/15 9-17 1,15 * mon-fri").unwrap();
        assert_eq!(s.minute, (1 << 0) | (1 << 15) | (1 << 30) | (1 << 45));
        assert_eq!(s.hour, (9..=17).fold(0, |a, h| a | (1 << h)));
        assert_eq!(s.dom, (1 << 1) | (1 << 15));
        assert_eq!(s.dow, (1..=5).fold(0, |a, d| a | (1 << d)));
        assert!(Schedule::parse("@daily").is_ok());
        assert_eq!(
            Schedule::parse("0 0 * * 7").unwrap().dow,
            Schedule::parse("0 0 * * sun").unwrap().dow
        );
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "* * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "*/0 * * * *",
            "5-1 * * * *",
            "x * * * *",
            "* * * * * *",
        ] {
            assert!(Schedule::parse(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn next_every_minute_and_hourly() {
        let s = Schedule::parse("* * * * *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 10, 2, 12, 30)),
            Some(at(2026, 10, 2, 12, 31))
        );
        let s = Schedule::parse("0 * * * *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 10, 2, 12, 30)),
            Some(at(2026, 10, 2, 13, 0))
        );
        // Strictly after: a time exactly on a firing yields the next one.
        assert_eq!(
            s.next_after(at(2026, 10, 2, 13, 0)),
            Some(at(2026, 10, 2, 14, 0))
        );
    }

    #[test]
    fn next_crosses_days_months_years() {
        let s = Schedule::parse("30 4 1 * *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 10, 2, 0, 0)),
            Some(at(2026, 11, 1, 4, 30))
        );
        let s = Schedule::parse("0 0 1 1 *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 10, 2, 0, 0)),
            Some(at(2027, 1, 1, 0, 0))
        );
        // Leap day.
        let s = Schedule::parse("0 0 29 2 *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 1, 1, 0, 0)),
            Some(at(2028, 2, 29, 0, 0))
        );
        // Never fires.
        assert_eq!(
            Schedule::parse("0 0 30 2 *")
                .unwrap()
                .next_after(at(2026, 1, 1, 0, 0)),
            None
        );
    }

    #[test]
    fn dom_dow_or_semantics() {
        // 2026-10-02 is a Friday. "1st of month OR Monday".
        let s = Schedule::parse("0 0 1 * mon").unwrap();
        assert_eq!(
            s.next_after(at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 5, 0, 0))
        );
        let s = Schedule::parse("0 0 * * mon").unwrap();
        assert_eq!(
            s.next_after(at(2026, 10, 2, 0, 0)),
            Some(at(2026, 10, 5, 0, 0))
        );
        assert!(s.matches(&at(2026, 10, 5, 0, 0)));
        assert!(!s.matches(&at(2026, 10, 6, 0, 0)));
    }
}
