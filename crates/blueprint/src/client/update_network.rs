// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;
use serde_json::Value;

use super::about::about;
use super::network_spec::network_spec;

/// Update a managed network to the document's spec. `true` when it changed.
pub(crate) fn update_network(
    fabric: &fabric::Client,
    cur: &fabric::Network,
    name: &str,
    raw: &Value,
) -> ApiResult<bool> {
    let spec = network_spec(name, raw)?;
    if spec == cur.spec {
        return Ok(false);
    }
    fabric
        .update_network(&cur.meta.id, Some(cur.meta.revision), |n| {
            n.spec = spec;
            Ok(())
        })
        .map_err(about("network", name))?;
    Ok(true)
}
