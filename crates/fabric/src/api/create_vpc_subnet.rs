// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::vpc;
use axum::{
    extract::{Extension, Path, Query, State},
    response::Response,
    Json,
};
use pistack_types::{kind, ApiError, ApiResult, Caller, Meta};

use super::create_project::create_project;
use super::created::created;
use super::scope_query::ScopeQuery;
use super::visible::visible;
use super::vpc_subnet::VpcSubnet;
use crate::types::NetworkSpec;
use crate::types::SubnetSpec;

/// The short road: a network and its one subnet, named alike, joined to the
/// vpc. `cidr` is a block or a prefix length to take from the vpc.
pub(crate) async fn create_vpc_subnet(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(mut body): Json<VpcSubnet>,
) -> ApiResult<Response> {
    // The vpc is the one the path names, and what is made lives in its
    // project: a vpc's networks and subnets never live apart from it.
    let asked = create_project(&caller, &sq.project)?;
    let path = visible(&caller, c.vpcs().resolve(asked.as_ref(), &id)?)?;
    let project = match &path.meta.project {
        Some(p) if asked.as_ref().is_some_and(|a| a != p) && !caller.is_admin() => {
            return Err(ApiError::forbidden(
                "the vpc belongs to another project than the token's",
            ));
        }
        Some(p) => Some(p.clone()),
        None => asked,
    };
    let name = body.name.trim().to_owned();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let cidr = vpc::carve(&path, &c.subnets_of_vpc(&path.meta.id)?, &body.cidr)?;
    let dns = std::mem::take(&mut body.dns_nameservers);
    let gateway = body.gateway;
    let dhcp = body.enable_dhcp;
    let net = c.create_network(
        Meta::new(kind::NETWORK, name.clone(), project.clone()),
        NetworkSpec {
            vpc: Some(path.meta.id.clone()),
            dns_domain: path.spec.dns_domain.clone(),
            ..NetworkSpec::default()
        },
    )?;
    let made = c.create_subnet(
        Meta::new(kind::SUBNET, name, project),
        SubnetSpec {
            network: net.meta.id.clone(),
            cidr,
            gateway,
            allocation_pools: Vec::new(),
            dns_nameservers: dns,
            enable_dhcp: dhcp,
            host_routes: Vec::new(),
        },
    );
    match made {
        Ok(sub) => Ok(created(sub)),
        Err(e) => {
            let _ = c.delete_network(&net.meta.id);
            Err(e)
        }
    }
}
