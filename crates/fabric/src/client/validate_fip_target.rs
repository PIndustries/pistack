// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use pistack_types::{ApiError, ApiResult, Id, Meta};
impl Client {
    pub(in crate::client) fn validate_fip_target(
        &self,
        meta: &Meta,
        fip: &Id,
        port: &Id,
    ) -> ApiResult<()> {
        let p = self.ports().get(port).map_err(store_err)?;
        check_same_project(meta, &p.meta, "port")?;
        if p.is_router_port() {
            return Err(ApiError::bad_request(format!(
                "port {port} belongs to a router"
            )));
        }
        if p.spec.fixed_ips.is_empty() {
            return Err(ApiError::bad_request(format!(
                "port {port} has no fixed ip"
            )));
        }
        for other in self.floating_ips().list(None).map_err(store_err)? {
            if other.meta.id != *fip && other.spec.port.as_ref() == Some(port) {
                return Err(ApiError::conflict(format!(
                    "port {port} already has floating ip {}",
                    other.meta.id
                )));
            }
        }
        Ok(())
    }
}
