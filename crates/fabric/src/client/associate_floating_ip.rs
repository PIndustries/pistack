// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    /// Point the floating IP at `port`, or detach it with `None`.
    pub fn associate_floating_ip(&self, id: &Id, port: Option<Id>) -> ApiResult<FloatingIp> {
        let fip = self.floating_ips().get(id).map_err(store_err)?;
        if let Some(p) = &port {
            self.validate_fip_target(&fip.meta, id, p)?;
        }
        self.floating_ips()
            .modify(id, |f| {
                f.spec.port = port.clone();
                Ok(())
            })
            .map_err(store_err)
    }
}
