// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::machine_key::machine_key;
use super::rpc_driver::RpcDriver;
use pistack_rpc::proto::{CommandKind, MachineRef};
use pistack_types::{ApiResult, Id};
// The body is reached through the `HostDriver` impl in `start.rs`, which
// calls these by name; in this module alone they look unused.
#[allow(dead_code)]
impl RpcDriver {
    async fn stop(&self, id: &Id) -> ApiResult<()> {
        self.call(
            Some(&machine_key(id)),
            CommandKind::StopMachine,
            &MachineRef { id: id.clone() },
        )
        .await
        .map(|_| ())
    }
}
