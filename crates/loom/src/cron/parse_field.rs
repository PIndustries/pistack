// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::cron::cron_error::CronError;
use crate::cron::parse_value::parse_value;

pub(crate) fn parse_field(
    field: &str,
    min: u32,
    max: u32,
    names: Option<&[&str]>,
) -> Result<u64, CronError> {
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
