// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use chrono::{DateTime, Utc};
use pistack_types::ApiResult;

pub(crate) fn parse_amz_date(s: &str) -> ApiResult<DateTime<Utc>> {
    let s = s.trim();
    // Literal trailing `Z` (UTC). chrono's `%Z` is a timezone name, not this.
    if s.len() != 16 || !s.ends_with('Z') {
        return Err(s3_err(
            "InvalidRequest",
            "invalid AWS date format, want YYYYMMDDTHHMMSSZ",
        ));
    }
    chrono::NaiveDateTime::parse_from_str(&s[..15], "%Y%m%dT%H%M%S")
        .map(|n| n.and_utc())
        .map_err(|_| {
            s3_err(
                "InvalidRequest",
                "invalid AWS date format, want YYYYMMDDTHHMMSSZ",
            )
        })
}
