// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The one path `loom` calls: converge a TCP balancer, its listener and its
//! pool's members onto the wanted set, idempotently.

use super::store_err::store_err;
use crate::types::*;
use pistack_store::StoreError;
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};

use super::Client;
impl Client {
    /// Create (or converge) a TCP balancer named `name` in `project` on
    /// `vip_network` with one listener on `port` and one pool holding
    /// `members`. Idempotent: a second call with the same inputs changes
    /// nothing; a different member list is applied as adds and removes.
    /// Returns the balancer; its `status` fills in once this client
    /// reconciles it. The balancer is labelled `pistack.io/managed-by=loom`,
    /// so the relay module's background reconciler leaves it to the caller.
    pub async fn ensure_tcp_balancer(
        &self,
        project: &Id,
        name: &str,
        vip_network: &str,
        port: u16,
        members: &[std::net::SocketAddr],
    ) -> ApiResult<Balancer> {
        let lb = match self.balancers().get_by_name(Some(project), name) {
            Ok(lb) => lb,
            Err(StoreError::NotFound(_)) => {
                let mut meta = Meta::new(kind::BALANCER, name, Some(project.clone()));
                meta.labels.insert(MANAGED_LABEL.into(), "loom".into());
                self.create_balancer(
                    meta,
                    BalancerSpec {
                        vip_network: vip_network.to_owned(),
                        ..BalancerSpec::default()
                    },
                )?
            }
            Err(e) => return Err(store_err(e)),
        };
        if lb.spec.vip_network != vip_network {
            return Err(ApiError::conflict(format!(
                "balancer {name} exists on network {}",
                lb.spec.vip_network
            )));
        }

        let pool = match self.pools_of(&lb.meta.id)?.into_iter().next() {
            Some(p) => p,
            None => self.create_pool(
                Meta::new(kinds::POOL, format!("{name}-pool"), Some(project.clone())),
                PoolSpec {
                    balancer: lb.meta.id.clone(),
                    protocol: PoolProtocol::Tcp,
                    algorithm: Algorithm::RoundRobin,
                    session_persistence: None,
                    health_monitor: Some(HealthMonitor::default()),
                    admin_up: true,
                },
            )?,
        };

        // One listener on `port`; move it if the port changed.
        let listeners = self.listeners_of(&lb.meta.id)?;
        match listeners
            .iter()
            .find(|l| l.spec.default_pool.as_ref() == Some(&pool.meta.id))
        {
            Some(l) if l.spec.port == port => {}
            Some(l) => {
                self.update_listener(&l.meta.id, None, |x| {
                    x.spec.port = port;
                    Ok(())
                })?;
            }
            None => {
                self.create_listener(
                    Meta::new(
                        kinds::LISTENER,
                        format!("{name}-{port}"),
                        Some(project.clone()),
                    ),
                    ListenerSpec {
                        balancer: lb.meta.id.clone(),
                        protocol: ListenerProtocol::Tcp,
                        port,
                        default_pool: Some(pool.meta.id.clone()),
                        connection_limit: 0,
                        admin_up: true,
                        door: None,
                        routes: Vec::new(),
                    },
                )?;
            }
        }

        // Converge members.
        let have = self.members_of(&pool.meta.id)?;
        for m in &have {
            if !members.contains(&m.addr()) {
                self.delete_member(&m.meta.id)?;
            }
        }
        for want in members {
            if !have.iter().any(|m| m.addr() == *want) {
                self.create_member(
                    Meta::new(
                        kinds::MEMBER,
                        format!("{name}-{}-{}", want.ip(), want.port()).replace(':', "-"),
                        Some(project.clone()),
                    ),
                    MemberSpec {
                        pool: pool.meta.id.clone(),
                        address: want.ip(),
                        port: want.port(),
                        weight: 1,
                        backup: false,
                        admin_up: true,
                    },
                )?;
            }
        }
        self.balancers().get(&lb.meta.id).map_err(store_err)
    }
}
