// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::DEFAULT_SG;
use crate::types::{FloatingIp, Network, Port, Router, SecurityGroup, Subnet, Vpc};
use pistack_store::Store;
use pistack_types::{ApiResult, Id};

pub use gate::quota::{FLOATING_IPS, NETWORKS, PORTS, ROUTERS, SECURITY_GROUPS, SUBNETS, VPCS};

/// How many of `resource` the project has.
pub fn count(store: &Store, project: &Id, resource: &str) -> ApiResult<u64> {
    let p = Some(project);
    let n = match resource {
        NETWORKS => store.collection::<Network>().count(p)?,
        VPCS => store.collection::<Vpc>().count(p)?,
        SUBNETS => store.collection::<Subnet>().count(p)?,
        ROUTERS => store.collection::<Router>().count(p)?,
        FLOATING_IPS => store.collection::<FloatingIp>().count(p)?,
        SECURITY_GROUPS => store
            .collection::<SecurityGroup>()
            .list(p)?
            .iter()
            .filter(|g| g.meta.name != DEFAULT_SG)
            .count(),
        PORTS => store
            .collection::<Port>()
            .list(p)?
            .iter()
            .filter(|port| !port.is_router_port())
            .count(),
        _ => 0,
    };
    Ok(n as u64)
}
