// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::any::Any;

use super::machine_runtime::MachineRuntime;

#[async_trait]
pub trait HostDriver: Any + Send + Sync + 'static {
    /// Create (if needed) and power on. Idempotent: starting a running
    /// machine returns its current runtime.
    async fn start(&self, m: &Machine) -> ApiResult<MachineRuntime>;
    /// Power off, keep the definition and disks.
    async fn stop(&self, id: &Id) -> ApiResult<()>;
    /// Tear down everything. Deleting an unknown machine is not an error.
    async fn delete(&self, id: &Id) -> ApiResult<()>;
    /// Everything this host currently runs.
    async fn list(&self) -> ApiResult<Vec<MachineRuntime>>;

    /// Replace `m`'s root disk with a fresh copy of its image and start it
    /// again. First-boot setup is written again, so the guest runs it as it
    /// did on a new machine. When this returns the old disk is gone, whether
    /// the new one was written or not, so a failure is final and must end
    /// the request rather than be retried. Default: this host cannot rebuild.
    async fn rebuild(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        let _ = m;
        Err(ApiError::not_implemented(
            "this host cannot rebuild a root disk",
        ))
    }
}
