// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub fn s3_code_of(err: &ApiError) -> Option<&str> {
    err.details.get("s3_code").and_then(|v| v.as_str())
}
