// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::is_common_prefix_marker::is_common_prefix_marker;
use super::key_after_marker::key_after_marker;

/// Whether an upload survives key-marker / upload-id-marker under optional
/// delimiter (filter **before** collapse so a common-prefix marker does not
/// reappear on the next page).
pub(crate) fn upload_after_markers(
    key: &str,
    upload_id: &str,
    key_marker: Option<&str>,
    upload_id_marker: Option<&str>,
    delimiter: Option<&str>,
) -> bool {
    let Some(km) = key_marker else {
        return true;
    };
    if delimiter.is_some_and(|d| is_common_prefix_marker(km, d)) {
        return key_after_marker(key, km, delimiter);
    }
    match upload_id_marker {
        None => key > km,
        Some(um) => key > km || (key == km && upload_id > um),
    }
}
