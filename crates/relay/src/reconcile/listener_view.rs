// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Assembling what a provider needs to program one listener: its balancer,
//! default pool and active members, its terminated TLS material, and its
//! routes each with their own pool.

use crate::client::Client;
use crate::provider::ListenerView;
use crate::reconcile::store_err;
use crate::types::{Listener, ListenerProtocol};
use pistack_types::ApiResult;

impl Client {
    /// Resolve a listener's balancer, pool and active members. `None` when
    /// the balancer has no VIP yet.
    pub(super) fn listener_view(&self, l: &Listener) -> ApiResult<Option<ListenerView>> {
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
}
