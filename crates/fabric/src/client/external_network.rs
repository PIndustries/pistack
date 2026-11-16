// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub(crate) fn external_network(&self, id: &Id) -> ApiResult<Network> {
        let net = self.networks().get(id).map_err(store_err)?;
        if !net.spec.external {
            return Err(ApiError::bad_request(format!(
                "network {id} is not external"
            )));
        }
        Ok(net)
    }
}
