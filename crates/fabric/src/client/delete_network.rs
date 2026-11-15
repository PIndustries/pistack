// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, released};
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn delete_network(&self, id: &Id) -> ApiResult<Network> {
        let net = self.networks().get(id).map_err(store_err)?;
        if !self.subnets_of_network(id)?.is_empty() {
            return Err(ApiError::conflict(format!(
                "network {id} still has subnets"
            )));
        }
        if !self.ports_on_network(id)?.is_empty() {
            return Err(ApiError::conflict(format!("network {id} still has ports")));
        }
        if self
            .routers()
            .list(None)
            .map_err(store_err)?
            .iter()
            .any(|r| r.spec.external_network.as_ref() == Some(id))
        {
            return Err(ApiError::conflict(format!(
                "network {id} is a router gateway"
            )));
        }
        let _ = net;
        let net = self.networks().delete(id).map_err(store_err)?;
        released(&self.store, net.meta.project.as_ref(), usage::NETWORKS);
        Ok(net)
    }
}
