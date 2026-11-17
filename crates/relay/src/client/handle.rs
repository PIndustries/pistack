// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The handle and the write paths: every method here validates and writes
//! `spec`, and tells the provider before a delete removes the row.

use super::balancer::validate_balancer;
use super::listener::validate_listener;
use super::member::validate_member;
use super::pool::{initial_status, validate_pool};
use super::project::check_same_project;
use crate::provider::{LbProvider, Providers};
use crate::types::*;
use pistack_rpc::CommandQueue;
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};
use std::sync::Arc;

#[derive(Clone)]
pub struct Client {
    store: Store,
    providers: Providers,
    queue: Option<CommandQueue>,
    /// Leave balancers labelled `pistack.io/managed-by=loom` alone in the
    /// reconcile paths. Set on the relay module's background reconciler:
    /// loom binds and closes those sockets on its own providers, so a
    /// second program here would bind the same address twice.
    leave_loom: bool,
}

fn store_err(e: StoreError) -> ApiError {
    e.into()
}

impl Client {
    pub fn new(store: Store, providers: Providers) -> Self {
        Self {
            store,
            providers,
            queue: None,
            leave_loom: false,
        }
    }

    /// Skip loom's balancers when reconciling (see the field doc).
    pub fn leave_loom_managed(mut self) -> Self {
        self.leave_loom = true;
        self
    }

    /// True when this client's reconcile paths must not program `lb`.
    pub(crate) fn leaves(&self, lb: &Balancer) -> bool {
        self.leave_loom && lb.meta.labels.get(MANAGED_LABEL).map(String::as_str) == Some("loom")
    }

    /// Queue used by an edge door. Without one, that door stays unavailable.
    pub fn with_queue(mut self, queue: CommandQueue) -> Self {
        self.queue = Some(queue);
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn queue(&self) -> Option<&CommandQueue> {
        self.queue.as_ref()
    }
    pub fn providers(&self) -> &Providers {
        &self.providers
    }
    pub fn provider_for(&self, lb: &Balancer) -> ApiResult<Arc<dyn LbProvider>> {
        self.providers.for_balancer(lb)
    }

    pub fn balancers(&self) -> Collection<Balancer> {
        self.store.collection()
    }
    pub fn listeners(&self) -> Collection<Listener> {
        self.store.collection()
    }
    pub fn pools(&self) -> Collection<Pool> {
        self.store.collection()
    }
    pub fn members(&self) -> Collection<Member> {
        self.store.collection()
    }

    // ---- lookups ------------------------------------------------------------

    pub fn listeners_of(&self, balancer: &Id) -> ApiResult<Vec<Listener>> {
        Ok(self
            .listeners()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|l| &l.spec.balancer == balancer)
            .collect())
    }

    pub fn pools_of(&self, balancer: &Id) -> ApiResult<Vec<Pool>> {
        Ok(self
            .pools()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|p| &p.spec.balancer == balancer)
            .collect())
    }

    pub fn members_of(&self, pool: &Id) -> ApiResult<Vec<Member>> {
        let mut v: Vec<Member> = self
            .members()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|m| &m.spec.pool == pool)
            .collect();
        v.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
        Ok(v)
    }

    /// Listeners whose default pool or one of whose routes is `pool`.
    pub fn listeners_using_pool(&self, pool: &Id) -> ApiResult<Vec<Listener>> {
        Ok(self
            .listeners()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|l| {
                l.spec.default_pool.as_ref() == Some(pool)
                    || l.spec.routes.iter().any(|r| &r.pool == pool)
            })
            .collect())
    }

    // ---- balancers ------------------------------------------------------------

    pub fn create_balancer(&self, mut meta: Meta, spec: BalancerSpec) -> ApiResult<Balancer> {
        meta.id = Id::new(kind::BALANCER);
        validate_balancer(&spec)?;
        self.providers.for_balancer(&Balancer {
            meta: meta.clone(),
            spec: spec.clone(),
            status: BalancerStatus::default(),
        })?;
        self.balancers()
            .create(Balancer {
                meta,
                spec,
                status: BalancerStatus::default(),
            })
            .map_err(store_err)
    }

    pub fn update_balancer(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Balancer) -> ApiResult<()>,
    ) -> ApiResult<Balancer> {
        let cur = self.balancers().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        validate_balancer(&next.spec)?;
        if next.spec.provider != cur.spec.provider || next.spec.vip_network != cur.spec.vip_network
        {
            return Err(ApiError::bad_request(
                "balancer provider and vip_network are immutable",
            ));
        }
        if next.spec.vip_address.is_some() && next.spec.vip_address != cur.spec.vip_address {
            if let Some(v) = cur.status.vip {
                if next.spec.vip_address != Some(v) {
                    return Err(ApiError::bad_request(
                        "vip_address is immutable once realised",
                    ));
                }
            }
        }
        self.balancers().update(next, expected).map_err(store_err)
    }

    /// Delete a balancer and everything under it (listeners, pools,
    /// members), telling the provider first.
    pub async fn delete_balancer(&self, id: &Id) -> ApiResult<Balancer> {
        let lb = self.balancers().get(id).map_err(store_err)?;
        for l in self.listeners_of(id)? {
            self.delete_listener(&l.meta.id).await?;
        }
        for p in self.pools_of(id)? {
            self.delete_pool(&p.meta.id)?;
        }
        if lb.status.provider_ref.is_some() {
            if let Ok(provider) = self.providers.for_balancer(&lb) {
                if let Err(e) = provider.delete_balancer(id).await {
                    tracing::warn!(balancer = %id, error = %e, "relay: provider delete failed");
                }
            }
        }
        self.balancers().delete(id).map_err(store_err)
    }

    // ---- listeners ----------------------------------------------------------------

    pub fn create_listener(&self, mut meta: Meta, spec: ListenerSpec) -> ApiResult<Listener> {
        meta.id = Id::new(kinds::LISTENER);
        let lb = self.balancers().get(&spec.balancer).map_err(store_err)?;
        check_same_project(&meta, &lb.meta, "balancer")?;
        validate_listener(&lb, &spec, self.listeners_of(&lb.meta.id)?, |pid| {
            self.pools().get(pid).map_err(store_err)
        })?;
        self.listeners()
            .create(Listener {
                meta,
                spec,
                status: ListenerStatus::default(),
            })
            .map_err(store_err)
    }

    pub fn update_listener(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Listener) -> ApiResult<()>,
    ) -> ApiResult<Listener> {
        let cur = self.listeners().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.balancer != cur.spec.balancer {
            return Err(ApiError::bad_request("listener balancer is immutable"));
        }
        let lb = self
            .balancers()
            .get(&cur.spec.balancer)
            .map_err(store_err)?;
        let self_id = id.clone();
        let others = self
            .listeners_of(&cur.spec.balancer)?
            .into_iter()
            .filter(|l| l.meta.id != self_id)
            .collect::<Vec<_>>();
        validate_listener(&lb, &next.spec, others, |pid| {
            self.pools().get(pid).map_err(store_err)
        })?;
        self.listeners().update(next, expected).map_err(store_err)
    }

    pub async fn delete_listener(&self, id: &Id) -> ApiResult<Listener> {
        crate::door::stop(&id.to_string());
        let l = self.listeners().get(id).map_err(store_err)?;
        if l.status.provider_ref.is_some() {
            if let Ok(lb) = self.balancers().get(&l.spec.balancer) {
                if let Ok(provider) = self.providers.for_balancer(&lb) {
                    if let Err(e) = provider.delete_listener(&lb.meta.id, id).await {
                        tracing::warn!(listener = %id, error = %e, "relay: provider delete failed");
                    }
                }
            }
        }
        self.listeners().delete(id).map_err(store_err)
    }

    // ---- pools --------------------------------------------------------------------

    pub fn create_pool(&self, mut meta: Meta, spec: PoolSpec) -> ApiResult<Pool> {
        meta.id = Id::new(kinds::POOL);
        let lb = self.balancers().get(&spec.balancer).map_err(store_err)?;
        check_same_project(&meta, &lb.meta, "balancer")?;
        validate_pool(&spec)?;
        self.pools()
            .create(Pool {
                meta,
                spec,
                status: PoolStatus::default(),
            })
            .map_err(store_err)
    }

    pub fn update_pool(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Pool) -> ApiResult<()>,
    ) -> ApiResult<Pool> {
        let cur = self.pools().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.balancer != cur.spec.balancer {
            return Err(ApiError::bad_request("pool balancer is immutable"));
        }
        if next.spec.protocol != cur.spec.protocol && !self.listeners_using_pool(id)?.is_empty() {
            return Err(ApiError::conflict(
                "pool protocol cannot change while a listener uses it",
            ));
        }
        validate_pool(&next.spec)?;
        self.pools().update(next, expected).map_err(store_err)
    }

    /// Delete a pool and its members. Refused while a listener points at it.
    pub fn delete_pool(&self, id: &Id) -> ApiResult<Pool> {
        let users = self.listeners_using_pool(id)?;
        if let Some(l) = users.first() {
            return Err(ApiError::conflict(format!(
                "pool {id} is used by listener {}",
                l.meta.id
            )));
        }
        for m in self.members_of(id)? {
            self.members().delete(&m.meta.id).map_err(store_err)?;
        }
        self.pools().delete(id).map_err(store_err)
    }

    // ---- members -----------------------------------------------------------------

    pub fn create_member(&self, mut meta: Meta, spec: MemberSpec) -> ApiResult<Member> {
        meta.id = Id::new(kinds::MEMBER);
        let pool = self.pools().get(&spec.pool).map_err(store_err)?;
        check_same_project(&meta, &pool.meta, "pool")?;
        validate_member(&spec)?;
        for other in self.members_of(&spec.pool)? {
            if other.spec.address == spec.address && other.spec.port == spec.port {
                return Err(ApiError::conflict(format!(
                    "{}:{} is already member {}",
                    spec.address, spec.port, other.meta.id
                )));
            }
        }
        let status = MemberStatus {
            operating_status: initial_status(&pool, &spec),
            ..MemberStatus::default()
        };
        self.members()
            .create(Member { meta, spec, status })
            .map_err(store_err)
    }

    pub fn update_member(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Member) -> ApiResult<()>,
    ) -> ApiResult<Member> {
        let cur = self.members().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.pool != cur.spec.pool {
            return Err(ApiError::bad_request("member pool is immutable"));
        }
        validate_member(&next.spec)?;
        if next.spec.address != cur.spec.address || next.spec.port != cur.spec.port {
            for other in self.members_of(&cur.spec.pool)? {
                if other.meta.id != *id
                    && other.spec.address == next.spec.address
                    && other.spec.port == next.spec.port
                {
                    return Err(ApiError::conflict(format!(
                        "{}:{} is already member {}",
                        next.spec.address, next.spec.port, other.meta.id
                    )));
                }
            }
        }
        if !next.spec.admin_up && cur.spec.admin_up {
            next.status.operating_status = OperatingStatus::Draining;
        }
        self.members().update(next, expected).map_err(store_err)
    }

    pub fn delete_member(&self, id: &Id) -> ApiResult<Member> {
        let m = self.members().delete(id).map_err(store_err)?;
        self.touch_pool(&m.spec.pool);
        Ok(m)
    }

    /// Members of a pool that should take traffic right now. Everything the
    /// data planes dial starts here, so no algorithm can reach a member that
    /// is administratively down, `Offline`, `Draining` or weighted zero.
    pub fn active_members(&self, pool: &Pool) -> ApiResult<Vec<Member>> {
        Ok(crate::picker::eligible(&self.members_of(&pool.meta.id)?)
            .into_iter()
            .cloned()
            .collect())
    }

    // ---- for loom ---------------------------------------------------------------------

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
