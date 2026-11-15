// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Meta};
impl Client {
    /// Create a port. Fills in subnet, fixed IPs, MAC and the project's
    /// default security group when the spec leaves them empty.
    pub fn create_port(&self, meta: Meta, spec: PortSpec) -> ApiResult<Port> {
        self.port_row(meta, spec, None)
    }
}
