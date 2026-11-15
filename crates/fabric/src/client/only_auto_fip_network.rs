// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    /// Refuse to mark a second network for automatic floating IPs: which
    /// pool a machine draws from must not depend on the order of two ids.
    pub(in crate::client) fn only_auto_fip_network(&self, id: &Id) -> ApiResult<()> {
        let other = self
            .networks()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .find(|n| n.spec.auto_floating_ip && &n.meta.id != id);
        match other {
            Some(n) => Err(ApiError::conflict(format!(
                "network '{}' ({}) already gives out automatic floating ips; turn it off there first",
                n.meta.name, n.meta.id
            ))),
            None => Ok(()),
        }
    }
}
