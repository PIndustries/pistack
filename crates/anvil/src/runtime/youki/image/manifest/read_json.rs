// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn read_json(path: &Path) -> ApiResult<serde_json::Value> {
    let bytes = std::fs::read(path).map_err(|_| cannot_unpack())?;
    serde_json::from_slice(&bytes).map_err(|_| cannot_unpack())
}
