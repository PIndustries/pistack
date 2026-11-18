// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

/// The S3 `NoSuchVersion` answer for a generation that is not there.
pub(crate) fn no_such_version(raw: &str) -> ApiError {
    crate::multipart::s3_err(
        "NoSuchVersion",
        format!("The version ID specified in the request does not exist: {raw}"),
    )
}
