// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    /// Create a port on `network` (name or id, looked up in `project` then
    /// cloud-wide) plugged into `device`, and realise it on the backend
    /// before returning so the caller gets a binding. The reconciler's own
    /// pass sees the status already current and skips it.
    pub async fn create_port_for_device(
        &self,
        project: &Id,
        network: &str,
        device: Id,
    ) -> ApiResult<Port> {
        self.create_port_for_device_with(project, network, device, &[])
            .await
    }
}
