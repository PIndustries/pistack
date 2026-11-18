// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::error::ErrorCode;
use pistack_types::ApiError;

/// A store conflict naming an existing bucket row. The create path is the
/// only one that raises it, so the message (`bkt … already exists`) is enough.
pub(crate) fn bucket_exists(err: &ApiError) -> bool {
    err.code == ErrorCode::Conflict
        && err.message.contains("bkt ")
        && err.message.contains("already exists")
}
