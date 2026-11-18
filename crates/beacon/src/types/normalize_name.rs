// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// Normalise a domain name: lower-case, trailing dot, each label 1-63
/// LDH characters (underscore allowed, for `_acme-challenge` and SRV
/// owners; a leading `*` label for wildcards). 253 characters at most.
pub fn normalize_name(raw: &str) -> ApiResult<String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }
    if s == "." {
        return Err(ApiError::bad_request("the root zone cannot be managed"));
    }
    let body = s.trim_end_matches('.');
    if body.len() > 253 {
        return Err(ApiError::bad_request(format!(
            "name {raw} is longer than 253 characters"
        )));
    }
    let labels: Vec<&str> = body.split('.').collect();
    for (i, label) in labels.iter().enumerate() {
        if label.is_empty() {
            return Err(ApiError::bad_request(format!(
                "name {raw} has an empty label"
            )));
        }
        if label.len() > 63 {
            return Err(ApiError::bad_request(format!(
                "label {label} in {raw} is longer than 63 characters"
            )));
        }
        if *label == "*" && i == 0 {
            continue;
        }
        let ok = label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok || label.starts_with('-') || label.ends_with('-') {
            return Err(ApiError::bad_request(format!(
                "label {label} in {raw} is not a valid hostname label"
            )));
        }
    }
    Ok(format!("{body}."))
}
