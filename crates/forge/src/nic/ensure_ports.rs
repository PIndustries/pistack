// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};

use crate::types::Machine;

use super::fabric_client::fabric_client;
use super::named::named;
use super::resolve_groups::resolve_groups;
use super::resolve_network::resolve_network;

/// Create the missing port for each named network. A second call reuses the
/// port. No names means no port, which is how `demo` and `web` stay put.
pub async fn ensure_ports(store: &Store, m: &Machine) -> ApiResult<()> {
    let names = named(&m.spec.networks);
    if names.is_empty() {
        return Ok(());
    }
    let Some(project) = m.meta.project.clone() else {
        return Err(ApiError::bad_request(
            "a project network needs a project on the machine",
        ));
    };
    let client = fabric_client(store);
    let groups = resolve_groups(&client, &project, &m.spec.security_groups)?;
    for name in names {
        let net = resolve_network(&client, &project, &name)?;
        let exists = client
            .ports_of_device(&m.meta.id)?
            .into_iter()
            .any(|p| p.spec.network == net.meta.id);
        if exists {
            continue;
        }
        client
            .create_port_for_device_with(&project, &name, m.meta.id.clone(), &groups)
            .await?;
    }
    Ok(())
}
