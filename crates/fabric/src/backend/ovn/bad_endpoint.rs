// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use async_trait::async_trait;

use super::super::ovsdb::Transport;
use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

/// Stands in for a client when the endpoint does not parse, so the error
/// shows on every resource instead of stopping the server.
pub(in crate::backend::ovn) struct BadEndpoint(pub(in crate::backend::ovn) String);

#[async_trait]
impl Transport for BadEndpoint {
    async fn transact(&self, _db: &str, _ops: Vec<Value>) -> ApiResult<Vec<Value>> {
        Err(ApiError::unavailable(self.0.clone()))
    }
}
