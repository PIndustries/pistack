// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

use super::about::about;
use super::pod_spec::pod_spec;

/// loom keeps a pod's spec fixed once its machine exists; that rule
/// applies here too.
pub(crate) fn update_pod(
    store: &Store,
    cur: &loom::Pod,
    name: &str,
    raw: &Value,
) -> ApiResult<bool> {
    let mut spec = pod_spec(name, raw)?;
    if spec == cur.spec {
        return Ok(false);
    }
    if cur.meta.annotations.contains_key(loom::DELETE_ANNOTATION) {
        return Err(ApiError::conflict(format!(
            "pod {name}: pod is being deleted"
        )));
    }
    loom::http::Api::validate(&mut spec, Some(cur))
        .map_err(|e| ApiError::new(e.code, format!("pod {name}: {}", e.message)))?;
    let mut next = cur.clone();
    next.spec = spec;
    store
        .collection::<loom::Pod>()
        .update(next, Some(cur.meta.revision))
        .map_err(|e| about("pod", name)(e.into()))?;
    Ok(true)
}
