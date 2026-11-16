// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn update_network(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Network) -> ApiResult<()>,
    ) -> ApiResult<Network> {
        let cur = self.networks().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        validate_network(&next.spec)?;
        if next.spec.vpc != cur.spec.vpc {
            return Err(ApiError::bad_request(
                "a network belongs to the vpc it was made in",
            ));
        }
        if next.spec.kind != cur.spec.kind {
            return Err(ApiError::bad_request("network kind is immutable"));
        }
        if next.spec.external && !cur.spec.external {
            self.check_vpc_membership(&cur.meta.project, &next.spec)?;
        }
        if next.spec.auto_floating_ip && !cur.spec.auto_floating_ip {
            self.only_auto_fip_network(id)?;
        }
        self.networks().update(next, expected).map_err(store_err)
    }
}
