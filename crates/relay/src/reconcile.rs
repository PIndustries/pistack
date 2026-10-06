// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The reconciler: turns `spec` into provider state and writes `status`.
//!
//! Driven by store events (`handle_event`), a full pass at start
//! (`reconcile_all`) and a periodic retry of anything not `Ready`
//! (`reconcile_pending`). Every status write records
//! `observed_revision = revision + 1`, the revision the write itself
//! produces, so the event that write emits is recognised and skipped.
//!
//! A listener is the unit the provider programs: it carries its pool and
//! the pool's active members. Member and pool changes therefore re-program
//! the listeners that use the pool, and a health-check flip does the same.

use crate::client::Client;
use crate::provider::ListenerView;
use crate::types::*;
use pistack_store::StoreError;
use pistack_types::{kind, ApiResult, Event, EventAction, Id, Phase};

fn store_err(e: StoreError) -> pistack_types::ApiError {
    e.into()
}

impl Client {
    /// React to one store event.
    pub async fn handle_event(&self, ev: &Event) {
        if ev.action == EventAction::Deleted {
            // Deletes talk to the provider inside `Client`, and a member
            // delete touches its pool there, so nothing is left to do.
            return;
        }
        match ev.kind.as_str() {
            kind::BALANCER => self.reconcile_balancer(&ev.id).await,
            kinds::LISTENER => self.reconcile_listener(&ev.id).await,
            kinds::POOL => self.reconcile_pool(&ev.id).await,
            kinds::MEMBER => self.reconcile_member(&ev.id).await,
            _ => {}
        }
    }

    /// Everything, in dependency order.
    pub async fn reconcile_all(&self) {
        let lbs = self.balancers().list(None).unwrap_or_default();
        for lb in lbs {
            self.reconcile_balancer(&lb.meta.id).await;
        }
    }

    /// Everything not `Ready` (or whose spec moved since the last pass).
    pub async fn reconcile_pending(&self) {
        for lb in self.balancers().list(None).unwrap_or_default() {
            if stale(
                lb.status.observed_revision,
                lb.meta.revision,
                lb.status.phase,
            ) {
                self.reconcile_balancer(&lb.meta.id).await;
            }
        }
        for l in self.listeners().list(None).unwrap_or_default() {
            if stale(l.status.observed_revision, l.meta.revision, l.status.phase) {
                self.reconcile_listener(&l.meta.id).await;
            }
        }
        for p in self.pools().list(None).unwrap_or_default() {
            if stale(p.status.observed_revision, p.meta.revision, p.status.phase) {
                self.reconcile_pool(&p.meta.id).await;
            }
        }
    }

    pub async fn reconcile_balancer(&self, id: &Id) {
        let lb = match self.balancers().try_get(id) {
            Ok(Some(lb)) => lb,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(balancer = %id, error = %e, "relay: read failed");
                return;
            }
        };
        if self.leaves(&lb) {
            return;
        }
        if !stale(
            lb.status.observed_revision,
            lb.meta.revision,
            lb.status.phase,
        ) {
            return;
        }
        let result = match self.provider_for(&lb) {
            Ok(p) => p.ensure_balancer(&lb).await,
            Err(e) => Err(e),
        };
        let outcome = self.balancers().modify(id, |b| {
            match &result {
                Ok(r) => {
                    b.status.vip = Some(r.vip);
                    b.status.provider_ref = Some(r.provider_ref.clone());
                    b.status.phase = Phase::Ready;
                    b.status.message.clear();
                }
                Err(e) => {
                    b.status.phase = Phase::Error;
                    b.status.message = e.message.clone();
                }
            }
            b.status.observed_revision = b.meta.revision + 1;
            Ok(())
        });
        if let Err(e) = outcome {
            tracing::warn!(balancer = %id, error = %e, "relay: status write failed");
        }
        if result.is_ok() {
            for l in self.listeners_of(id).unwrap_or_default() {
                self.reconcile_listener(&l.meta.id).await;
            }
        }
    }

    pub async fn reconcile_listener(&self, id: &Id) {
        let l = match self.listeners().try_get(id) {
            Ok(Some(l)) => l,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(listener = %id, error = %e, "relay: read failed");
                return;
            }
        };
        self.program_listener(&l, true).await;
    }

    /// Push a listener's current picture to the provider. With `force`
    /// false the call is skipped when the listener itself is up to date
    /// (used when only its pool's membership changed: the provider still
    /// needs the new member list, so callers pass `true` there too; `false`
    /// is for the retry pass).
    async fn program_listener(&self, l: &Listener, force: bool) {
        if !force && !stale(l.status.observed_revision, l.meta.revision, l.status.phase) {
            return;
        }
        let view = match self.listener_view(l) {
            Ok(Some(v)) => v,
            Ok(None) => {
                // Balancer not realised yet; its own pass will come back.
                return;
            }
            Err(e) => {
                let _ = self.listeners().modify(&l.meta.id, |x| {
                    x.status.phase = Phase::Error;
                    x.status.message = e.message.clone();
                    x.status.observed_revision = x.meta.revision + 1;
                    Ok(())
                });
                return;
            }
        };
        if self.leaves(&view.balancer) {
            return;
        }
        let result = match self.provider_for(&view.balancer) {
            Ok(p) => p.ensure_listener(&view).await,
            Err(e) => Err(e),
        };
        let result = match result {
            Ok(bound) => match view.listener.spec.door {
                Some(kind) => {
                    let upstream = format!("http://{bound}");
                    let listen = view.listener.spec.port.to_string();
                    match crate::door::apply(
                        &view.listener.meta.id.to_string(),
                        kind,
                        &listen,
                        &upstream,
                        crate::door::DoorSide {
                            store: self.store(),
                            queue: self.queue(),
                        },
                    )
                    .await
                    {
                        Ok(_) => Ok(bound),
                        Err(e) => Err(e),
                    }
                }
                None => Ok(bound),
            },
            Err(e) => Err(e),
        };
        let outcome = self.listeners().modify(&l.meta.id, |x| {
            match &result {
                Ok(r) => {
                    x.status.provider_ref = Some(r.clone());
                    x.status.phase = Phase::Ready;
                    x.status.message.clear();
                }
                Err(e) => {
                    x.status.phase = Phase::Error;
                    x.status.message = e.message.clone();
                }
            }
            x.status.observed_revision = x.meta.revision + 1;
            Ok(())
        });
        if let Err(e) = outcome {
            tracing::warn!(listener = %l.meta.id, error = %e, "relay: status write failed");
        }
    }

    /// Resolve a listener's balancer, pool and active members. `None` when
    /// the balancer has no VIP yet.
    fn listener_view(&self, l: &Listener) -> ApiResult<Option<ListenerView>> {
        let balancer = self.balancers().get(&l.spec.balancer).map_err(store_err)?;
        if balancer.status.vip.is_none() {
            return Ok(None);
        }
        let pool = match &l.spec.default_pool {
            Some(pid) => {
                let p = self.pools().get(pid).map_err(store_err)?;
                let members = self.active_members(&p)?;
                Some((p, members))
            }
            None => None,
        };
        let tls = match &l.spec.protocol {
            ListenerProtocol::Terminated { cert } => {
                Some(crate::tls::load_kept(self.store(), cert)?)
            }
            _ => None,
        };
        let mut routes = Vec::with_capacity(l.spec.routes.len());
        for route in &l.spec.routes {
            let pool = self.pools().get(&route.pool).map_err(store_err)?;
            let members = self.active_members(&pool)?;
            routes.push(crate::provider::RouteView {
                route: route.clone(),
                pool,
                members,
            });
        }
        Ok(Some(ListenerView {
            balancer,
            listener: l.clone(),
            pool,
            tls,
            routes,
        }))
    }

    /// A pool change (spec or membership) re-programs its listeners and
    /// refreshes the pool's counters.
    pub async fn reconcile_pool(&self, id: &Id) {
        let p = match self.pools().try_get(id) {
            Ok(Some(p)) => p,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(pool = %id, error = %e, "relay: read failed");
                return;
            }
        };
        if !stale(p.status.observed_revision, p.meta.revision, p.status.phase) {
            return;
        }
        let members = self.members_of(id).unwrap_or_default();
        let online = members
            .iter()
            .filter(|m| {
                matches!(
                    m.status.operating_status,
                    OperatingStatus::Online | OperatingStatus::NoMonitor
                )
            })
            .count() as u32;
        let _ = self.pools().modify(id, |x| {
            x.status.members = members.len() as u32;
            x.status.online = online;
            x.status.phase = Phase::Ready;
            x.status.message.clear();
            x.status.observed_revision = x.meta.revision + 1;
            Ok(())
        });
        for l in self.listeners_using_pool(id).unwrap_or_default() {
            self.program_listener(&l, true).await;
        }
    }

    /// A member change (spec, or the health checker flipping its status)
    /// re-programs the listeners that use its pool.
    pub async fn reconcile_member(&self, id: &Id) {
        let m = match self.members().try_get(id) {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(member = %id, error = %e, "relay: read failed");
                return;
            }
        };
        if m.status.observed_revision == m.meta.revision && m.status.phase == Phase::Ready {
            return;
        }
        let _ = self.members().modify(id, |x| {
            x.status.phase = Phase::Ready;
            x.status.observed_revision = x.meta.revision + 1;
            Ok(())
        });
        self.touch_pool(&m.spec.pool);
    }

    /// Mark a pool stale so its counters and listeners refresh. Used by the
    /// health checker after a status flip and by member deletes.
    pub fn touch_pool(&self, pool: &Id) {
        let _ = self.pools().modify(pool, |p| {
            // Leaving observed_revision behind the new revision makes the
            // pool stale for the event this write emits.
            p.status.message.clear();
            Ok(())
        });
    }
}

fn stale(observed: u64, revision: u64, phase: Phase) -> bool {
    observed != revision || phase == Phase::Error
}
