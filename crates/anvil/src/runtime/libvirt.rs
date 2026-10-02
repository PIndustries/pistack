// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! libvirt/KVM runtime. Stub: no FFI yet. Every call returns
//! `not_implemented` naming what is missing, so a misconfigured host fails
//! loudly instead of silently running nothing.

use super::{MachineRequest, MachineRuntime, RunningMachine};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};

#[derive(Default)]
pub struct LibvirtRuntime;

fn todo(op: &str) -> ApiError {
    ApiError::not_implemented(format!(
        "libvirt runtime: {op} is not implemented yet (no libvirt binding linked); use the mock runtime in dev"
    ))
}

#[async_trait]
impl MachineRuntime for LibvirtRuntime {
    fn name(&self) -> &'static str {
        "libvirt"
    }
    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        Err(todo(&format!("start of {} ({})", req.name, req.id)))
    }
    async fn stop(&self, id: &Id) -> ApiResult<()> {
        Err(todo(&format!("stop of {id}")))
    }
    async fn delete(&self, id: &Id) -> ApiResult<()> {
        Err(todo(&format!("delete of {id}")))
    }
    async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
        Err(todo("list"))
    }
}
