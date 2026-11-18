// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::StoreError;
use pistack_types::{kind, ApiError, ApiResult, Id, Meta, Phase};
use relay::{
    Algorithm, BalancerSpec, Client, ListenerProtocol, ListenerSpec, PoolProtocol, PoolSpec,
    Provider, Route, MANAGED_LABEL,
};
use std::net::SocketAddr;

use crate::realizer::held::Held;
use crate::realizer::listener_answers::listener_answers;
use crate::realizer::route_plan::RoutePlan;
use crate::realizer::sync_members::sync_members;
use crate::realizer::vip_network::VIP_NETWORK;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn ensure_forward(
    held: &Held,
    client: &Client,
    project: Option<Id>,
    name: &str,
    owner_label: &'static str,
    owner_id: &str,
    provider: Provider,
    listener_protocol: ListenerProtocol,
    pool_protocol: PoolProtocol,
    port: u16,
    bind: Option<SocketAddr>,
    members: &[SocketAddr],
    routes: &[RoutePlan],
) -> ApiResult<String> {
    let term = held.term(owner_label);
    let existing = match client.balancers().get_by_name(project.as_ref(), name) {
        Ok(lb) if lb.spec.vip_network != VIP_NETWORK || lb.spec.provider != provider => {
            return Err(ApiError::unavailable("listener is not available"));
        }
        Ok(lb)
            if bind.is_some()
                && (lb.spec.vip_address != bind.map(|b| b.ip())
                    || lb.spec.vip_port != bind.map(|b| b.port())) =>
        {
            // A node port another server bound (the old leader, or this
            // server under an old address) is rebuilt on this server.
            tracing::info!(balancer = %lb.meta.id, name, "loom: node port moves to this server");
            client.delete_balancer(&lb.meta.id).await?;
            None
        }
        Ok(lb)
            if bind.is_none() && (lb.spec.vip_address.is_some() || lb.spec.vip_port.is_some()) =>
        {
            return Err(ApiError::unavailable("listener is not available"));
        }
        Ok(lb) => Some(lb),
        Err(StoreError::NotFound(_)) => None,
        Err(e) => return Err(e.into()),
    };
    let lb = match existing {
        Some(lb) => lb,
        None => {
            let mut meta = Meta::new(kind::BALANCER, name, project.clone());
            meta.labels.insert(MANAGED_LABEL.into(), "loom".into());
            meta.labels.insert(owner_label.into(), owner_id.to_owned());
            client.create_balancer(
                meta,
                BalancerSpec {
                    vip_network: VIP_NETWORK.into(),
                    vip_address: bind.map(|b| b.ip()),
                    vip_port: bind.map(|b| b.port()),
                    provider,
                    admin_up: true,
                    ..BalancerSpec::default()
                },
            )?
        }
    };

    let pools = client.pools_of(&lb.meta.id)?;
    let default_name = format!("{name}-pool");
    let route_prefix = format!("{name}-route-");
    let find_or_create = |pool_name: &str| -> ApiResult<relay::Pool> {
        if let Some(pool) = pools.iter().find(|p| p.meta.name == pool_name) {
            return Ok(pool.clone());
        }
        client.create_pool(
            Meta::new(relay::types::kinds::POOL, pool_name, project.clone()),
            PoolSpec {
                balancer: lb.meta.id.clone(),
                protocol: pool_protocol,
                algorithm: Algorithm::RoundRobin,
                session_persistence: None,
                health_monitor: None,
                admin_up: true,
            },
        )
    };
    let pool = match pools.iter().find(|p| p.meta.name == default_name) {
        Some(pool) => pool.clone(),
        None if routes.is_empty() && pools.len() == 1 => pools[0].clone(),
        None => find_or_create(&default_name)?,
    };
    let mut route_pools = Vec::with_capacity(routes.len());
    let mut want_routes = Vec::with_capacity(routes.len());
    for (i, plan) in routes.iter().enumerate() {
        let route_pool = find_or_create(&format!("{route_prefix}{i}"))?;
        want_routes.push(Route {
            host: plan.host.clone(),
            path: plan.path.clone(),
            path_match: plan.path_match,
            pool: route_pool.meta.id.clone(),
        });
        route_pools.push(route_pool);
    }

    let listeners = client.listeners_of(&lb.meta.id)?;
    let listener = match listeners
        .into_iter()
        .find(|l| l.spec.default_pool.as_ref() == Some(&pool.meta.id))
    {
        Some(l)
            if l.spec.port == port
                && l.spec.protocol == listener_protocol
                && l.spec.routes == want_routes =>
        {
            l
        }
        Some(l) => {
            let protocol = listener_protocol.clone();
            let next_routes = want_routes.clone();
            client.update_listener(&l.meta.id, None, move |row| {
                row.spec.port = port;
                row.spec.protocol = protocol;
                row.spec.routes = next_routes;
                Ok(())
            })?
        }
        None => client.create_listener(
            Meta::new(
                relay::types::kinds::LISTENER,
                format!("{name}-lsn"),
                project.clone(),
            ),
            ListenerSpec {
                balancer: lb.meta.id.clone(),
                protocol: listener_protocol,
                port,
                default_pool: Some(pool.meta.id.clone()),
                connection_limit: 0,
                admin_up: true,
                door: None,
                routes: want_routes,
            },
        )?,
    };

    // Route pools past the current rule count are no longer referenced.
    for old in &pools {
        let Some(index) = old.meta.name.strip_prefix(&route_prefix) else {
            continue;
        };
        if index.parse::<usize>().is_ok_and(|i| i >= routes.len()) {
            client.delete_pool(&old.meta.id)?;
        }
    }

    let changed = sync_members(client, &project, name, &pool.meta.id, members)?;
    let mut stale_routes = Vec::new();
    for (route_pool, plan) in route_pools.iter().zip(routes) {
        if sync_members(
            client,
            &project,
            &route_pool.meta.name,
            &route_pool.meta.id,
            &plan.members,
        )? {
            stale_routes.push(route_pool.meta.id.clone());
        }
    }

    let provider_handle = client.provider_for(&lb)?;
    let programmed_here = held.ready_here(&lb.meta.id);
    // Recorded before the bind, so losing the lead closes this socket even
    // when the rest of the pass fails.
    held.opening(&lb.meta.id, owner_label);
    let id = lb.meta.id.clone();
    let result: ApiResult<String> = async {
        let live = provider_handle.ensure_balancer(&lb).await?;
        let lb = client
            .balancers()
            .get(&lb.meta.id)
            .map_err(ApiError::from)?;
        let listener = client
            .listeners()
            .get(&listener.meta.id)
            .map_err(ApiError::from)?;
        let aligned = programmed_here
            && lb.status.phase == Phase::Ready
            && lb.status.provider_ref.as_deref() == Some(live.provider_ref.as_str())
            && listener.status.phase == Phase::Ready
            && listener.status.provider_ref.as_deref() == Some(live.provider_ref.as_str());
        if !aligned {
            client.update_balancer(&lb.meta.id, None, |_| Ok(()))?;
            client.reconcile_balancer(&lb.meta.id).await;
        }
        if changed || !aligned {
            client.touch_pool(&pool.meta.id);
            client.reconcile_pool(&pool.meta.id).await;
        }
        for route_pool in &route_pools {
            if !aligned || stale_routes.contains(&route_pool.meta.id) {
                client.touch_pool(&route_pool.meta.id);
                client.reconcile_pool(&route_pool.meta.id).await;
            }
        }
        #[cfg(test)]
        held.paused().await;

        let lb = client
            .balancers()
            .get(&lb.meta.id)
            .map_err(ApiError::from)?;
        let listener = client
            .listeners()
            .get(&listener.meta.id)
            .map_err(ApiError::from)?;
        if listener.status.phase != Phase::Ready {
            let msg = if !listener.status.message.is_empty() {
                listener.status.message
            } else if lb.status.phase != Phase::Ready && !lb.status.message.is_empty() {
                lb.status.message
            } else {
                "listener is not available".into()
            };
            return Err(ApiError::unavailable(msg));
        }
        held.mark_ready(&lb.meta.id, owner_label);
        let addr = listener
            .status
            .provider_ref
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::unavailable("listener is not available"))?;
        // A udp socket cannot be probed with a connect; its bind is the proof.
        if listener.spec.protocol != ListenerProtocol::Udp && !listener_answers(&addr).await {
            return Err(ApiError::unavailable("listener is not available"));
        }
        Ok(addr)
    }
    .await;
    if held.term(owner_label) != term {
        // The owner let go while this pass was binding: whatever it opened
        // belongs to a term that is over.
        held.ids().remove(&id);
        client.providers().stop_everywhere(&id).await;
        tracing::info!(balancer = %id, name, "loom: closed a listener opened while losing the lead");
        return Err(ApiError::unavailable(
            "lead was lost while the listener was opening",
        ));
    }
    result
}
