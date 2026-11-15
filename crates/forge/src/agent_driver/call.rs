// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
use pistack_rpc::proto::CommandKind;
use pistack_types::{ApiError, ApiResult};
impl RpcDriver {
    /// Send one command and wait for the host's answer. With `key` the
    /// command is keyed: the reconciler issues it again after a leader
    /// change, it keeps its id, and the host runs it once.
    pub(super) async fn call<T: serde::Serialize>(
        &self,
        key: Option<&str>,
        kind: CommandKind,
        payload: &T,
    ) -> ApiResult<serde_json::Value> {
        let payload =
            serde_json::to_value(payload).map_err(|e| ApiError::internal(e.to_string()))?;
        let ticket = match key {
            Some(key) => self.queue.enqueue_keyed(&self.node, key, kind, payload)?,
            None => self.queue.enqueue(&self.node, kind, payload)?,
        };
        // Timeouts, queue-full and node-removed all map to `unavailable`
        // through `From<RpcError>`; the reconciler retries next pass.
        let result = ticket.wait().await?;
        result.into_api_result()
    }
}
