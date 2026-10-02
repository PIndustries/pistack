// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! youki (OCI) container runtime. Stub: no bundle generation or exec yet.

use super::{MachineRequest, MachineRuntime, RunningMachine};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};

#[derive(Default)]
pub struct YoukiRuntime;

fn todo(op: &str) -> ApiError {
    ApiError::not_implemented(format!(
        "youki runtime: {op} is not implemented yet (OCI bundle generation pending); use the mock runtime in dev"
    ))
}

#[async_trait]
impl MachineRuntime for YoukiRuntime {
    fn name(&self) -> &'static str {
        "youki"
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
