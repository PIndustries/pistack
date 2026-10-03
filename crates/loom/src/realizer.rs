// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the Service controller and whatever makes a cluster IP
//! actually route: OVN load balancers via `fabric`, L4/L7 balancers via
//! `relay`, or nothing at all in dev mode.

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
}

/// Records nothing, routes nothing. Dev mode and tests.
#[derive(Default, Debug, Clone, Copy)]
pub struct NoopRealizer;

#[async_trait]
impl ServiceRealizer for NoopRealizer {
    async fn realize(&self, _service: &Service, _endpoints: &[Endpoint]) -> ApiResult<Vec<String>> {
        Ok(Vec::new())
    }
    async fn remove(&self, _service: &Service) -> ApiResult<()> {
        Ok(())
    }
}
