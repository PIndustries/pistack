// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Routing each balancer to the provider that serves its `spec.provider`.

use super::lb_provider::LbProvider;
use crate::types::Balancer;
use pistack_types::{ApiResult, Id};
use std::sync::Arc;

/// Route each balancer to the provider that serves its `spec.provider`.
#[derive(Clone, Default)]
pub struct Providers {
    inner: Vec<Arc<dyn LbProvider>>,
}

impl Providers {
    pub fn new(list: Vec<Arc<dyn LbProvider>>) -> Self {
        Self { inner: list }
    }

    pub fn for_balancer(&self, lb: &Balancer) -> ApiResult<Arc<dyn LbProvider>> {
        self.inner
            .iter()
            .find(|p| p.serves(lb.spec.provider))
            .cloned()
            .ok_or_else(|| {
                pistack_types::ApiError::unavailable(format!(
                    "no provider for {:?} balancers",
                    lb.spec.provider
                ))
            })
    }

    /// Stop whatever any provider runs for balancer `id`, without touching
    /// the store. A provider with nothing for `id` does nothing.
    pub async fn stop_everywhere(&self, id: &Id) {
        for p in &self.inner {
            if let Err(e) = p.delete_balancer(id).await {
                tracing::warn!(balancer = %id, provider = p.name(), error = %e, "relay: local stop failed");
            }
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.inner.iter().map(|p| p.name()).collect()
    }
}
