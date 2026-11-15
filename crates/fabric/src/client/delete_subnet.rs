// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, released};
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn delete_subnet(&self, id: &Id) -> ApiResult<Subnet> {
        let sub = self.subnets().get(id).map_err(store_err)?;
        let net = self.networks().get(&sub.spec.network).map_err(store_err)?;
        // The vpc's own interface leaves with the subnet; any other router on
        // the subnet is somebody else's and the subnet cannot go under it.
        let owned = self
            .vpc_of_network(&net.meta.id)
            .and_then(|v| self.vpc_router(&v.meta.id).ok().map(|r| r.meta.id));
        for p in self.ports_on_network(&sub.spec.network)? {
            if p.spec.device.as_ref() == owned.as_ref() {
                continue;
            }
            if p.spec.subnet.as_ref() == Some(id)
                || p.spec.fixed_ips.iter().any(|ip| sub.spec.cidr.contains(ip))
            {
                let what = if p.is_router_port() {
                    "a router interface"
                } else {
                    "ports"
                };
                return Err(ApiError::conflict(format!("subnet {id} still has {what}")));
            }
        }
        if let Some(v) = self.vpc_of_network(&net.meta.id) {
            // The interface port goes first: it holds an address in the
            // subnet, and the subnet cannot go with anything on it.
            self.vpc_detach(&v, id)?;
        }
        let sub = self.subnets().delete(id).map_err(store_err)?;
        released(&self.store, sub.meta.project.as_ref(), usage::SUBNETS);
        if let Some(v) = self.vpc_of_network(&net.meta.id) {
            self.count_vpc_subnets(&v.meta.id);
        }
        Ok(sub)
    }
}
