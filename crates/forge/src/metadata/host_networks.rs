// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Port, Subnet};
use pistack_rpc::MetadataNetwork;
use pistack_store::Store;
use pistack_types::{ApiResult, Id};
use std::collections::{BTreeSet, HashSet};

use crate::types::Machine;

use super::host_named::host_named;

/// The networks that have a port of a machine placed on host `node`, with
/// each one's metadata port and IPv4 subnets. A host nobody registered has
/// none.
pub fn host_networks(store: &Store, node: &str) -> ApiResult<Vec<MetadataNetwork>> {
    let Some(host) = host_named(store, node)? else {
        return Ok(Vec::new());
    };
    let here: HashSet<Id> = store
        .collection::<Machine>()
        .list(None)?
        .into_iter()
        .filter(|m| m.status.host.as_ref() == Some(&host))
        .map(|m| m.meta.id.clone())
        .collect();
    let networks: BTreeSet<Id> = store
        .collection::<Port>()
        .list(None)?
        .into_iter()
        .filter(|p| p.spec.device.as_ref().is_some_and(|d| here.contains(d)))
        .map(|p| p.spec.network)
        .collect();
    if networks.is_empty() {
        return Ok(Vec::new());
    }
    let subnets = store.collection::<Subnet>().list(None)?;
    Ok(networks
        .into_iter()
        .map(|net| MetadataNetwork {
            network: net.to_string(),
            port: fabric::backend::ovn::metadata_port_name(&net),
            mac: fabric::backend::ovn::metadata_port_mac(&net),
            cidrs: subnets
                .iter()
                .filter(|s| s.spec.network == net && s.spec.cidr.addr().is_ipv4())
                .map(|s| s.spec.cidr.trunc().to_string())
                .collect(),
        })
        .collect())
}
