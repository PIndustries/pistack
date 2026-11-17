// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

/// Prefix an error from another module with the document item it came from.
pub(crate) fn about<'a>(
    what: &'static str,
    name: &'a str,
) -> impl FnOnce(ApiError) -> ApiError + 'a {
    move |e| {
        let prefix = format!("{what} {name}:");
        if e.message.starts_with(&prefix) {
            e
        } else {
            ApiError::new(e.code, format!("{prefix} {}", e.message))
        }
    }
}
