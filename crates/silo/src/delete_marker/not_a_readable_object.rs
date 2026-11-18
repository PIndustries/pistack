// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;
use pistack_types::ApiError;

use super::marker_detail::MARKER_DETAIL;

/// The answer to a read that named a marker as its `versionId`. S3 refuses to
/// send a marker as an object.
pub(crate) fn not_a_readable_object(marker: &Object) -> ApiError {
    crate::multipart::s3_err(
        "MethodNotAllowed",
        format!(
            "object '{}' is a delete marker: version {}",
            marker.spec.key, marker.status.version
        ),
    )
    .with_detail(MARKER_DETAIL, marker.status.version.to_string())
}
