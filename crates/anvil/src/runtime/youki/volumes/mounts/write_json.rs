// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::write_mode;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use serde::Serialize;
use std::path::Path;

pub(in crate::runtime::youki) fn write_json(
    path: &Path,
    value: &impl Serialize,
    mode: u32,
) -> ApiResult<()> {
    let text = serde_json::to_vec_pretty(value).map_err(|e| {
        ApiError::internal(format!("youki runtime: encoding {}: {e}", path.display()))
    })?;
    write_mode(path, &text, mode)
}
