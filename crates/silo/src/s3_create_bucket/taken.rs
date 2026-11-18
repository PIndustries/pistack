// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;

/// The CreateBucket refusal for a name held by someone else.
pub(crate) fn taken() -> pistack_types::ApiError {
    s3_err(
        "BucketAlreadyExists",
        "The requested bucket name is not available. The bucket namespace is shared by all users \
         of the system.",
    )
}
