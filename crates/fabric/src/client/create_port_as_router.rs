// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Meta};
impl Client {
    /// Router ports live in the router's project; `create_port` already
    /// allows a project port on a cloud-wide (external) network.
    pub(in crate::client) fn create_port_as_router(
        &self,
        meta: Meta,
        spec: PortSpec,
    ) -> ApiResult<Port> {
        self.create_port(meta, spec)
    }
}
