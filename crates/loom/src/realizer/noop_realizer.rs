// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Endpoint, Service};
use async_trait::async_trait;
use pistack_types::ApiResult;

use crate::realizer::service_realizer::ServiceRealizer;

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
