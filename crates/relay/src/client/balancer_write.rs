// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The balancer write paths. Each validates the spec, refuses a change to
//! what the provider has already realised, and tells the provider before a
//! delete removes the row.

use super::balancer::validate_balancer;
use super::store_err::store_err;
use crate::types::{Balancer, BalancerSpec, BalancerStatus};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};

use super::Client;
impl Client {
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
}
