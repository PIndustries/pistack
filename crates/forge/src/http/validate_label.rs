// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::SYSTEM_LABEL_PREFIX;
use pistack_types::{ApiError, ApiResult};

pub(crate) fn validate_label(key: &str, value: &str) -> ApiResult<()> {
    if key.starts_with(SYSTEM_LABEL_PREFIX) {
        return Err(ApiError::unprocessable(format!(
            "label '{key}' is set by PiStack and cannot be written"
        )));
    }
    let word = |s: &str| {
        !s.is_empty()
            && s.len() <= 63
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
            && s.starts_with(|c: char| c.is_ascii_alphanumeric())
    };
    if !word(key) || !word(value) {
        return Err(ApiError::bad_request(format!(
            "label '{key}={value}': key and value must be 1-63 chars of [A-Za-z0-9._/-] and start with a letter or digit"
        )));
    }
    Ok(())
}
