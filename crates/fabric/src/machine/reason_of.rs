// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::error::ErrorCode;
use pistack_types::ApiError;

/// Reason for the condition a failed create or associate leaves behind.
pub(crate) fn reason_of(e: &ApiError) -> &'static str {
    match e.code {
        ErrorCode::QuotaExceeded => "QuotaExceeded",
        ErrorCode::Forbidden => "ProjectDisabled",
        _ if e.message.contains("no free floating ips") => "PoolExhausted",
        _ => "Failed",
    }
}
