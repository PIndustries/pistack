// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::machine_key::machine_key;
use super::rebuild_key::rebuild_key;
use super::rpc_driver::RpcDriver;
use super::runtime_from_wire::runtime_from_wire;
use crate::driver::{HostDriver, MachineRuntime};
use crate::types::Machine;
use async_trait::async_trait;
use pistack_rpc::proto::{CommandKind, MachineRef, RunningMachine};
use pistack_types::{ApiError, ApiResult, Id};
#[async_trait]
impl HostDriver for RpcDriver {
    async fn start(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        let mut req = self.request_for(m);
        if crate::ovn::enabled() && crate::nic::wants_port(m) {
            let store = self
                .store
                .as_ref()
                .ok_or_else(|| ApiError::internal("ovn placement needs the store"))?;
            req.nics = crate::nic::attachments(store, m)?;
            // The agent plugs the tap with the port's iface-id. A port fabric
            // has not written to OVN yet has no binding to plug with.
            if req.nics.iter().any(|n| n.binding.is_none()) {
                return Err(ApiError::unavailable(format!(
                    "machine {} has a port that is not bound in OVN yet",
                    m.meta.id
                )));
            }
        }
        if crate::overlay::enabled() && crate::nic::wants_port(m) {
            let store = self
                .store
                .as_ref()
                .ok_or_else(|| ApiError::internal("overlay placement needs the store"))?;
            req.nics = crate::nic::attachments(store, m)?;
            let plans = crate::overlay::plans_for_machine(store, m)?;
            for (node, plan) in &plans {
                if node == &self.node {
                    req.overlay = Some(plan.clone());
                } else if let Err(e) = self.call(None, CommandKind::SyncOverlay, plan).await {
                    tracing::warn!(
                        node,
                        error = %e,
                        "forge: overlay peer was not updated; the next pass retries"
                    );
                }
            }
        }
        if crate::nic::wants_port(m) && req.nics.is_empty() {
            return Err(ApiError::unavailable(format!(
                "machine {} has no address on its network",
                m.meta.id
            )));
        }
        if req.overlay.is_none() && req.nics.iter().any(|n| n.segment_id.is_some()) {
            return Err(ApiError::unavailable(format!(
                "machine {} is not on a host that joined its network segment",
                m.meta.id
            )));
        }
        let v = self
            .call(
                Some(&machine_key(&m.meta.id)),
                CommandKind::StartMachine,
                &req,
            )
            .await?;
        let rt: RunningMachine = serde_json::from_value(v).map_err(|e| {
            ApiError::internal(format!(
                "host {} returned a bad start result: {e}",
                self.node
            ))
        })?;
        Ok(runtime_from_wire(rt))
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        self.call(
            Some(&machine_key(id)),
            CommandKind::StopMachine,
            &MachineRef { id: id.clone() },
        )
        .await
        .map(|_| ())
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        self.call(
            Some(&machine_key(id)),
            CommandKind::DeleteMachine,
            &MachineRef { id: id.clone() },
        )
        .await
        .map(|_| ())
    }

    /// The old disk is gone once the host answers, so this is asked once:
    /// the reconciler ends the request in `Error` on a failure instead of
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

    /// Always `Ok(empty)`: there is no list command on the wire. The agent
    /// reports what it runs in every heartbeat, and the [`Registrar`] folds
    /// those reports into machine status instead.
    async fn list(&self) -> ApiResult<Vec<MachineRuntime>> {
        Ok(Vec::new())
    }
}
