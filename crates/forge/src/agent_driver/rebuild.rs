// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rebuild_key::rebuild_key;
use super::rpc_driver::RpcDriver;
use super::runtime_from_wire::runtime_from_wire;
use crate::driver::MachineRuntime;
use crate::types::Machine;
use pistack_rpc::proto::{CommandKind, RunningMachine};
use pistack_types::{ApiError, ApiResult};
// The body is reached through the `HostDriver` impl in `start.rs`, which
// calls these by name; in this module alone they look unused.
#[allow(dead_code)]
impl RpcDriver {
    /// sending it again.
    async fn rebuild(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        let req = self.request_for(m);
        let v = self
            .call(
                Some(&rebuild_key(&m.meta.id)),
                CommandKind::RebuildMachine,
                &req,
            )
            .await?;
        let rt: RunningMachine = serde_json::from_value(v).map_err(|e| {
            ApiError::internal(format!(
                "host {} returned a bad rebuild result: {e}",
                self.node
            ))
        })?;
        Ok(runtime_from_wire(rt))
    }
}
