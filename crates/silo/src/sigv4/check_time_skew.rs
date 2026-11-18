// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use chrono::{DateTime, Utc};
use pistack_types::ApiResult;

use super::max_skew_secs::MAX_SKEW_SECS;

pub(crate) fn check_time_skew(amz_date: DateTime<Utc>, expires_secs: Option<i64>) -> ApiResult<()> {
    let now = Utc::now();
    match expires_secs {
        Some(exp) => {
            let end = amz_date + chrono::Duration::seconds(exp);
            if now < amz_date - chrono::Duration::seconds(MAX_SKEW_SECS) || now > end {
                return Err(s3_err("AccessDenied", "Request has expired"));
            }
        }
        None => {
            let delta = (now - amz_date).num_seconds().abs();
            if delta > MAX_SKEW_SECS {
                return Err(s3_err(
                    "RequestTimeTooSkewed",
                    "The difference between the request time and the current time is too large."
                        .to_string(),
                ));
            }
        }
    }
    Ok(())
}
