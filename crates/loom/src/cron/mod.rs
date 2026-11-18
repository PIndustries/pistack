// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A small five-field cron parser: `minute hour day-of-month month day-of-week`.
//!
//! Supported per field: `*`, `n`, `a-b`, `*/s`, `a-b/s`, lists `a,b,c`,
//! month and weekday names (`jan`, `mon`), `7` as Sunday, and the macros
//! `@hourly`, `@daily`/`@midnight`, `@weekly`, `@monthly`, `@yearly`/`@annually`.
//! Day-of-month and day-of-week combine with OR when both are restricted,
//! as in Vixie cron. All times are UTC.

mod cron_error;
mod days;
mod months;
mod parse_field;
mod parse_value;
mod schedule;
#[cfg(test)]
mod tests;
pub use cron_error::CronError;
pub use schedule::Schedule;
