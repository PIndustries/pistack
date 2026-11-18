// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// `None`/empty → `max`; negative or non-numeric → 400; else clamp to
/// `1..=max`.
pub(crate) fn parse_limit(raw: Option<&str>, name: &str, max: usize) -> ApiResult<usize> {
    match raw {
        None | Some("") => Ok(max),
        Some(s) => {
            let n: i64 = s
                .parse()
                .map_err(|_| ApiError::bad_request(format!("{name} must be a number")))?;
            if n < 0 {
                return Err(ApiError::bad_request(format!("{name} must be >= 0")));
            }
            Ok((n as usize).clamp(1, max))
        }
    }
}
