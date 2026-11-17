// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use serde_json::Value;

use super::find_network::find_network;
use super::spec_is_empty::spec_is_empty;
use super::subnet_doc::SubnetDoc;

pub(crate) fn subnet_spec(
    store: &Store,
    project: Option<&Id>,
    name: &str,
    spec: &Value,
) -> ApiResult<fabric::SubnetSpec> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "subnet {name}: spec.network and spec.cidr are required"
        )));
    }
    let doc: SubnetDoc = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("subnet {name}: {e}")))?;
    let network_name = doc.network.trim();
    if network_name.is_empty() {
        return Err(ApiError::bad_request(format!(
            "subnet {name}: spec.network is required"
        )));
    }
    let cidr_raw = doc.cidr.trim();
    if cidr_raw.is_empty() {
        return Err(ApiError::bad_request(format!(
            "subnet {name}: spec.cidr is required"
        )));
    }
    let net = find_network(store, project, network_name, name)?;
    let cidr = cidr_raw.parse().map_err(|e| {
        ApiError::bad_request(format!("subnet {name}: invalid cidr '{cidr_raw}': {e}"))
    })?;
    Ok(fabric::SubnetSpec {
        network: net.meta.id.clone(),
        cidr,
        gateway: doc.gateway,
        allocation_pools: doc.allocation_pools,
        dns_nameservers: doc.dns_nameservers,
        enable_dhcp: doc.enable_dhcp,
        host_routes: doc.host_routes,
    })
}
