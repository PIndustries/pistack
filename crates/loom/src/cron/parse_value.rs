// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::cron::cron_error::CronError;

pub(crate) fn parse_value(
    s: &str,
    min: u32,
    max: u32,
    names: Option<&[&str]>,
) -> Result<u32, CronError> {
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
