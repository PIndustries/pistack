// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Id, Meta};

use super::create::Create;

pub(crate) fn new_meta<S>(k: &'static str, c: &Create<S>, project: Option<Id>) -> ApiResult<Meta> {
    if c.name.trim().is_empty() || c.name.len() > 128 || c.name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let mut m = Meta::new(k, c.name.trim(), project);
    m.description = c.description.clone();
    m.labels = c.labels.clone();
    m.annotations = c.annotations.clone();
    Ok(m)
}
