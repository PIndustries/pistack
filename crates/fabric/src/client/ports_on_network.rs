// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    /// Ports of a network, optionally those plugged into `device`.
    pub fn ports_on_network(&self, network: &Id) -> ApiResult<Vec<Port>> {
        Ok(self
            .ports()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|p| &p.spec.network == network)
            .collect())
    }
}
