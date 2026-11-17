// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Phase};
use serde_json::Value;

use super::about::about;
use super::machine_spec::machine_spec;

/// Kind, image, and size change only while the machine is pending, as
/// forge's own update allows. Reboot requests and, unless the document
/// sets it, desired power are kept from the stored machine.
pub(crate) fn update_machine(
    store: &Store,
    cur: &forge::Machine,
    name: &str,
    raw: &Value,
) -> ApiResult<bool> {
    let mut spec = machine_spec(name, raw)?;
    spec.restarts = cur.spec.restarts;
    if raw.get("power").is_none() {
        spec.power = cur.spec.power;
    }
    if spec == cur.spec {
        return Ok(false);
    }
    if forge::delete_requested(cur) {
        return Err(ApiError::conflict(format!(
            "machine {name}: machine is being deleted"
        )));
    }
    if cur.status.phase != Phase::Pending
        && (spec.kind != cur.spec.kind
            || spec.image != cur.spec.image
            || spec.size != cur.spec.size)
    {
        return Err(ApiError::unprocessable(format!(
            "machine {name}: kind, image and size can only change while the machine is pending"
        )));
    }
    let mut next = cur.clone();
    next.spec = spec;
    store
        .collection::<forge::Machine>()
        .update(next, Some(cur.meta.revision))
        .map_err(|e| about("machine", name)(e.into()))?;
    Ok(true)
}
