// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Meta};
use std::net::IpAddr;
impl Client {
    /// The most specific subnet that `owner` may use and that contains
    /// `addr`. Ties go to the lower id.
    pub(crate) fn subnet_containing(
        &self,
        owner: &Meta,
        addr: IpAddr,
    ) -> ApiResult<Option<Subnet>> {
        let mut subnets: Vec<Subnet> = self
            .subnets()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|s| {
                s.spec.cidr.contains(&addr) && check_same_project(owner, &s.meta, "subnet").is_ok()
            })
            .collect();
        subnets.sort_by(|a, b| {
            prefix_len(b.spec.cidr)
                .cmp(&prefix_len(a.spec.cidr))
                .then(a.meta.id.cmp(&b.meta.id))
        });
        Ok(subnets.into_iter().next())
    }
}
