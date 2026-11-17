// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Id, Meta};

use super::create_body::Create;

pub(crate) fn new_meta<S>(
    kind_: &'static str,
    body: &Create<S>,
    project: Option<Id>,
) -> ApiResult<Meta> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let mut meta = Meta::new(kind_, name, project);
    meta.description = body.description.clone();
    meta.labels = body.labels.clone();
    meta.annotations = body.annotations.clone();
    Ok(meta)
}
