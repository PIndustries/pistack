// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn update_floating_ip(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut FloatingIp) -> ApiResult<()>,
    ) -> ApiResult<FloatingIp> {
        let cur = self.floating_ips().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.network != cur.spec.network || next.spec.address != cur.spec.address {
            return Err(ApiError::bad_request(
                "floating ip network and address are immutable",
            ));
        }
        if next.spec.port != cur.spec.port {
            if let Some(p) = &next.spec.port {
                self.validate_fip_target(&next.meta, id, p)?;
            }
        }
        self.floating_ips()
            .update(next, expected)
            .map_err(store_err)
    }
}
