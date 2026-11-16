// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::state_dir::state_dir;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use pistack_types::Id;
use std::path::PathBuf;

pub(super) fn bundle_dir(id: &Id) -> ApiResult<PathBuf> {
    let name = id.to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ApiError::bad_request(format!(
            "youki runtime: machine id {name} cannot name a bundle"
        )));
    }
    Ok(state_dir().join(name))
}
