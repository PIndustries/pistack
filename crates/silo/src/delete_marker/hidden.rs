// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;
use pistack_types::ApiError;

use super::marker_detail::MARKER_DETAIL;

/// The marker a read of `key` found hiding it. Carried in the error's details
/// so the S3 route can answer `x-amz-delete-marker` and name the marker.
pub(crate) fn hidden(key: &str, marker: &Object) -> ApiError {
    ApiError::not_found(format!("object '{key}' not found"))
        .with_detail(MARKER_DETAIL, marker.status.version.to_string())
}
