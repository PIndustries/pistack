// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::Client;
use pistack_types::{ApiError, ApiResult, Id};

pub(crate) fn resolve_groups(
    client: &Client,
    project: &Id,
    names: &[String],
) -> ApiResult<Vec<Id>> {
    let mut out = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let sg = client
            .security_groups()
            .resolve(Some(project), name)
            .map_err(|_| ApiError::unprocessable(format!("security group '{name}' not found")))?;
        out.push(sg.meta.id);
    }
    Ok(out)
}
