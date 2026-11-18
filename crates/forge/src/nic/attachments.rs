// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_rpc::proto::NicAttachment;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};

use crate::types::Machine;

use super::fabric_client::fabric_client;
use super::named::named;
use super::nic_from::nic_from;
use super::resolve_network::resolve_network;

/// The NICs the host should boot. Fails when a named network has no port.
pub fn attachments(store: &Store, m: &Machine) -> ApiResult<Vec<NicAttachment>> {
    let names = named(&m.spec.networks);
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let Some(project) = m.meta.project.as_ref() else {
        return Err(ApiError::bad_request(
            "a project network needs a project on the machine",
        ));
    };
    let client = fabric_client(store);
    let ports = client.ports_of_device(&m.meta.id)?;
    let mut nics = Vec::with_capacity(names.len());
    for name in names {
        let net = resolve_network(&client, project, &name)?;
        let port = ports
            .iter()
            .find(|p| p.spec.network == net.meta.id)
            .ok_or_else(|| {
                ApiError::unavailable(format!("machine {} has no port on {name}", m.meta.id))
            })?;
        nics.push(nic_from(&client, port)?);
    }
    Ok(nics)
}
