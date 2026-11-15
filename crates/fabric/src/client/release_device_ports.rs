// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    /// Delete every port plugged into `device`. Floating IPs are detached.
    pub fn release_device_ports(&self, device: &Id) -> ApiResult<Vec<Port>> {
        let mut out = Vec::new();
        for p in self.ports_of_device(device)? {
            out.push(self.delete_port_unchecked(&p)?);
        }
        Ok(out)
    }
}
