// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Endpoint, Service};
use async_trait::async_trait;
use pistack_types::ApiResult;

#[async_trait]
pub trait ServiceRealizer: Send + Sync + 'static {
    /// Make `service.status.cluster_ip` (and node ports / load balancer, by
    /// kind) forward to `endpoints`. Called whenever the set changes.
    /// Idempotent. May return external addresses for `LoadBalancer`
    /// services; they land in `status.load_balancer`.
    async fn realize(&self, service: &Service, endpoints: &[Endpoint]) -> ApiResult<Vec<String>>;
    /// Tear down whatever `realize` built. Unknown services are not an error.
    async fn remove(&self, service: &Service) -> ApiResult<()>;
    /// This node stopped leading: close every service listener this process
    /// holds open, leaving the store alone for the next leader.
    async fn release(&self) {}
}
