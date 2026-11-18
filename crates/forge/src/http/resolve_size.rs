// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Size;
use pistack_types::{ApiError, ApiResult};

use super::app_state::AppState;
use super::sizes::sizes;

pub(crate) fn resolve_size(s: &AppState, name_or_id: &str) -> ApiResult<Size> {
    sizes(s)
        .resolve(None, name_or_id)
        .map_err(|_| ApiError::unprocessable(format!("size '{name_or_id}' not found")))
}
