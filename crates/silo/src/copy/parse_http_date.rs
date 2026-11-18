// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use chrono::{DateTime, Utc};
use pistack_types::ApiResult;

pub(crate) fn parse_http_date(raw: &str) -> ApiResult<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(raw.trim())
        .or_else(|_| DateTime::parse_from_rfc3339(raw.trim()))
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| {
            s3_err(
                "InvalidArgument",
                "invalid HTTP date in copy-source conditional header",
            )
        })
}
