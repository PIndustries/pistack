// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiResult, Id};
use serde_json::Value;

use super::about::about;
use super::subnet_spec::subnet_spec;

pub(crate) fn update_subnet(
    fabric: &fabric::Client,
    store: &Store,
    project: Option<&Id>,
    cur: &fabric::Subnet,
    name: &str,
    raw: &Value,
) -> ApiResult<bool> {
    let spec = subnet_spec(store, project, name, raw)?;
    if spec == cur.spec {
        return Ok(false);
    }
    fabric
        .update_subnet(&cur.meta.id, Some(cur.meta.revision), |s| {
            s.spec = spec;
            Ok(())
        })
        .map_err(about("subnet", name))?;
    Ok(true)
}
