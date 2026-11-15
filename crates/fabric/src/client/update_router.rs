// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    /// Update routes and the external gateway. `interfaces` changes are
    /// ignored here; use `set_router_interfaces`.
    pub fn update_router(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Router) -> ApiResult<()>,
    ) -> ApiResult<Router> {
        let cur = self.routers().get(id).map_err(store_err)?;
        owned_by_vpc(&cur)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        next.spec.interfaces = cur.spec.interfaces.clone();
        validate_routes(&next.spec.routes)?;
        if next.spec.external_network != cur.spec.external_network {
            if let Some(old) = &cur.spec.external_network {
                for p in self.ports_of_device(id)? {
                    if &p.spec.network == old {
                        self.delete_port_unchecked(&p)?;
                    }
                }
            }
            if let Some(n) = &next.spec.external_network {
                let net = self.external_network(n)?;
                self.create_gateway_port(&cur, &net)?;
            }
        }
        self.routers().update(next, expected).map_err(store_err)
    }
}
