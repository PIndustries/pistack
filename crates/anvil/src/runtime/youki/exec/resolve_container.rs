// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::runtime::Saved;
use pistack_types::ApiError;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) fn resolve_container<'a>(
    saved: &'a Saved,
    name: &str,
) -> ApiResult<&'a str> {
    if name.is_empty() {
        return Err(ApiError::bad_request(
            "youki runtime: container name is empty",
        ));
    }
    if let Some(index) = saved.container_names.iter().position(|n| n == name) {
        if let Some(id) = saved.containers.get(index) {
            return Ok(id);
        }
    }
    if let Some(id) = saved.containers.iter().find(|id| id.as_str() == name) {
        return Ok(id);
    }
    Err(ApiError::not_found(format!(
        "youki runtime: container {name} is not in this pod"
    )))
}
