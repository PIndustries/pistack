// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::ipam;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn update_subnet(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Subnet) -> ApiResult<()>,
    ) -> ApiResult<Subnet> {
        let cur = self.subnets().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.network != cur.spec.network || next.spec.cidr != cur.spec.cidr {
            return Err(ApiError::bad_request(
                "subnet network and cidr are immutable",
            ));
        }
        ipam::validate_subnet(&next.spec)?;
        // Shrinking pools or moving the gateway must not strand an address.
        if next.spec.gateway != cur.spec.gateway
            || next.spec.allocation_pools != cur.spec.allocation_pools
        {
            let used = self.used_addresses(&cur)?;
            for ip in &used {
                if ipam::is_reserved(&next.spec, *ip) {
                    return Err(ApiError::conflict(format!(
                        "{ip} is in use and would become reserved"
                    )));
                }
            }
        }
        self.subnets().update(next, expected).map_err(store_err)
    }
}
