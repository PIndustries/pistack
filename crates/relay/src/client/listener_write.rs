// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The listener write paths: a listener must belong to a balancer in this
//! project, and its routes may only point at pools on the same balancer.

use super::listener::validate_listener;
use super::project::check_same_project;
use super::store_err::store_err;
use crate::types::{kinds, Listener, ListenerSpec};
use pistack_types::{ApiError, ApiResult, Id, Meta};

use super::Client;
use crate::types::ListenerStatus;
impl Client {
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
}
