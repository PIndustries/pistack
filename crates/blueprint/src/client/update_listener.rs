// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

use super::about::about;
use super::listener_doc::listener_doc;
use super::listener_protocol::listener_protocol;

/// Protocol, port, connection limit, and admin state follow the document.
/// The balancer is fixed once the listener exists.
pub(crate) fn update_listener(
    relay: &relay::Client,
    store: &Store,
    cur: &relay::Listener,
    name: &str,
    raw: &Value,
) -> ApiResult<bool> {
    let doc = listener_doc(name, raw)?;
    let protocol = listener_protocol(name, &doc.protocol)?;
    let balancer = store
        .collection::<relay::Balancer>()
        .get(&cur.spec.balancer)
        .map_err(|e| about("listener", name)(e.into()))?;
    let wanted = match doc.balancer.trim() {
        "" => name,
        given => given,
    };
    if wanted != balancer.meta.name && wanted != balancer.meta.id.to_string() {
        return Err(ApiError::bad_request(format!(
            "listener {name}: balancer is immutable; it is '{}'",
            balancer.meta.name
        )));
    }
    let mut spec = cur.spec.clone();
    spec.protocol = protocol;
    spec.port = doc.port;
    spec.connection_limit = doc.connection_limit;
    spec.admin_up = doc.admin_up;
    if spec == cur.spec {
        return Ok(false);
    }
    relay
        .update_listener(&cur.meta.id, Some(cur.meta.revision), |l| {
            l.spec = spec;
            Ok(())
        })
        .map_err(about("listener", name))?;
    Ok(true)
}
