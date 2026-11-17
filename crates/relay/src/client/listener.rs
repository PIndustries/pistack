// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Listener validation: the port a balancer may bind, the protocol a
//! provider serves, and the pools a listener's routes may point at.

use crate::types::{
    Balancer, Listener, ListenerProtocol, ListenerSpec, Pool, PoolProtocol, Provider,
};
use pistack_types::{ApiError, ApiResult};

pub(super) fn validate_listener(
    lb: &Balancer,
    spec: &ListenerSpec,
    others: impl IntoIterator<Item = Listener>,
    pool_of: impl Fn(&pistack_types::Id) -> ApiResult<Pool>,
) -> ApiResult<()> {
    if spec.port == 0 {
        return Err(ApiError::bad_request("listener port must be 1-65535"));
    }
    if lb.spec.provider == Provider::Forward && spec.protocol.is_l7() {
        return Err(ApiError::bad_request(
            "provider forward balances L4 only; http listeners need provider relay",
        ));
    }
    let udp = matches!(spec.protocol, ListenerProtocol::Udp);
    for other in others {
        let other_udp = matches!(other.spec.protocol, ListenerProtocol::Udp);
        if other.spec.port == spec.port && other_udp == udp {
            return Err(ApiError::conflict(format!(
                "port {} is already bound by listener {}",
                spec.port, other.meta.id
            )));
        }
    }

    if let Some(pid) = &spec.default_pool {
        let pool = pool_of(pid)?;
        if pool.spec.balancer != lb.meta.id {
            return Err(ApiError::bad_request(format!(
                "pool {pid} belongs to another balancer"
            )));
        }
        let pool_udp = pool.spec.protocol == PoolProtocol::Udp;
        if pool_udp != udp {
            return Err(ApiError::bad_request(
                "listener and pool protocols must both be udp or both be stream",
            ));
        }
    }
    if !spec.routes.is_empty() && !spec.protocol.is_l7() {
        return Err(ApiError::bad_request(
            "routes need an http or terminated listener",
        ));
    }
    for route in &spec.routes {
        validate_route(route)?;
        let pool = pool_of(&route.pool)?;
        if pool.spec.balancer != lb.meta.id {
            return Err(ApiError::bad_request(format!(
                "pool {} belongs to another balancer",
                route.pool
            )));
        }
        if pool.spec.protocol == PoolProtocol::Udp {
            return Err(ApiError::bad_request("a route pool cannot be udp"));
        }
    }
    Ok(())
}

/// A member starts `NoMonitor` when its pool has no health monitor and
/// `Offline` otherwise, until the checker has seen it rise.
/// A route path starts with `/`. A host is lower-case letters, digits,
/// dots and dashes, optionally behind one leading `*.`, with no port.
fn validate_route(route: &crate::types::Route) -> ApiResult<()> {
    if !route.path.starts_with('/') || route.path.contains(char::is_whitespace) {
        return Err(ApiError::bad_request(format!(
            "route path {:?} must start with /",
            route.path
        )));
    }
    if let Some(host) = &route.host {
        let name = host.strip_prefix("*.").unwrap_or(host);
        let ok = !name.is_empty()
            && name.len() <= 253
            && !name.starts_with('.')
            && !name.ends_with('.')
            && !name.contains("..")
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
        if !ok {
            return Err(ApiError::bad_request(format!(
                "route host {host:?} must be a lower-case name, optionally starting with *."
            )));
        }
    }
    Ok(())
}
