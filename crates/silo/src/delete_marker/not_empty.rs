// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

/// The answer for a bucket that still holds records when it is being deleted:
/// `left` generations and markers between the caller and an empty bucket.
pub(crate) fn not_empty(left: usize) -> ApiError {
    crate::multipart::s3_err(
        "BucketNotEmpty",
        format!("the bucket still holds {left} generation(s) or delete marker(s)"),
    )
}
