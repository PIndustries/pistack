// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, TimeZone, Utc};

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
