// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    /// Subnets of a network in creation order.
    pub fn subnets_of_network(&self, network: &Id) -> ApiResult<Vec<Subnet>> {
        let mut v: Vec<Subnet> = self
            .subnets()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|s| &s.spec.network == network)
            .collect();
        v.sort_by(|a, b| {
            a.meta
                .created_at
                .cmp(&b.meta.created_at)
                .then(a.meta.id.cmp(&b.meta.id))
        });
        Ok(v)
    }
}
