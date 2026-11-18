// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Client, Port};
use pistack_rpc::proto::{NicAttachment, NicBinding};
use pistack_types::{ApiError, ApiResult};

use super::gateway_and_prefix::gateway_and_prefix;
use super::is_routed::is_routed;

pub(crate) fn nic_from(client: &Client, port: &Port) -> ApiResult<NicAttachment> {
    let address = port
        .spec
        .fixed_ips
        .iter()
        .find(|ip| ip.is_ipv4())
        .map(|ip| ip.to_string())
        .ok_or_else(|| {
            ApiError::bad_request(format!("port {} has no ipv4 address", port.meta.id))
        })?;
    let subnet_id = port
        .spec
        .subnet
        .clone()
        .ok_or_else(|| ApiError::bad_request(format!("port {} has no subnet", port.meta.id)))?;
    let subnet = client.subnets().get(&subnet_id)?;
    let net = client.networks().get(&port.spec.network)?;
    let (gateway, prefix) = gateway_and_prefix(&subnet)?;
    if port.spec.mac.is_empty() {
        return Err(ApiError::bad_request(format!(
            "port {} has no mac",
            port.meta.id
        )));
    }
    Ok(NicAttachment {
        network_id: port.spec.network.to_string(),
        mac: port.spec.mac.clone(),
        address: address.clone(),
        prefix,
        gateway,
        routed: is_routed(client, &subnet_id),
        segment_id: crate::overlay::segment_for_nic(&net, &address)?,
        binding: port.status.binding.as_ref().map(|b| NicBinding {
            vif_type: b.vif_type.clone(),
            vif_details: b.vif_details.clone(),
        }),
    })
}
