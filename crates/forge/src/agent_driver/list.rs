// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
use crate::driver::MachineRuntime;
use pistack_types::ApiResult;
// The body is reached through the `HostDriver` impl in `start.rs`, which
// calls these by name; in this module alone they look unused.
#[allow(dead_code)]
impl RpcDriver {
    /// reports what it runs in every heartbeat, and the [`Registrar`] folds
    /// those reports into machine status instead.
    async fn list(&self) -> ApiResult<Vec<MachineRuntime>> {
        Ok(Vec::new())
    }
}
