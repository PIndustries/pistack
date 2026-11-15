// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use pistack_types::{ApiResult, Id};
use std::collections::BTreeSet;
impl Client {
    pub(in crate::client) fn macs_on_network(&self, network: &Id) -> ApiResult<BTreeSet<String>> {
        Ok(self
            .ports_on_network(network)?
            .into_iter()
            .map(|p| p.spec.mac)
            .collect())
    }
}
