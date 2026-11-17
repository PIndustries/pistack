// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::error::ErrorCode;
use pistack_types::{ApiError, ApiResult};

pub(crate) fn already_gone(err: ApiError) -> ApiResult<()> {
    if err.code == ErrorCode::NotFound {
        Ok(())
    } else {
        Err(err)
    }
}
