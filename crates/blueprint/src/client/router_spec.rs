// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use serde_json::Value;

use super::find_named::find_named;
use super::router_doc::RouterDoc;
use super::spec_is_empty::spec_is_empty;

pub(crate) fn router_spec(
    store: &Store,
    project: Option<&Id>,
    name: &str,
    spec: &Value,
) -> ApiResult<fabric::RouterSpec> {
    if spec_is_empty(spec) {
        return Ok(fabric::RouterSpec::default());
    }
    let doc: RouterDoc = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("router {name}: {e}")))?;
    let external_network = match doc
        .external_network
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(network) => Some(
            find_named::<fabric::Network>(store, project, network, "router", name, "network")?
                .meta
                .id,
        ),
        None => None,
    };
    let mut interfaces = Vec::new();
    for iface in &doc.interfaces {
        let iface = iface.trim();
        if iface.is_empty() {
            return Err(ApiError::bad_request(format!(
                "router {name}: interface name is empty"
            )));
        }
        let subnet = find_named::<fabric::Subnet>(store, project, iface, "router", name, "subnet")?;
        interfaces.push(subnet.meta.id);
    }
    Ok(fabric::RouterSpec {
        external_network,
        vpc: None,
        interfaces,
        routes: doc.routes,
    })
}
