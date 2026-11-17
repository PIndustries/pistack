// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Building a `Meta` from a create body's name and metadata.

use pistack_types::{ApiError, ApiResult, Meta};
use std::collections::BTreeMap;

pub(super) fn new_meta(
    kind_name: &'static str,
    name: &str,
    description: String,
    labels: BTreeMap<String, String>,
    annotations: BTreeMap<String, String>,
    project: Option<pistack_types::Id>,
) -> ApiResult<Meta> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let mut meta = Meta::new(kind_name, name, project);
    meta.description = description;
    meta.labels = labels;
    meta.annotations = annotations;
    Ok(meta)
}
