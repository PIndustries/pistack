// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    /// Delete a user port. Floating IPs pointing at it are disassociated.
    /// Router ports are refused; they go with their router.
    pub fn delete_port(&self, id: &Id) -> ApiResult<Port> {
        let port = self.ports().get(id).map_err(store_err)?;
        if port.is_router_port() {
            return Err(ApiError::conflict(format!(
                "port {id} belongs to a router; detach it there"
            )));
        }
        self.delete_port_unchecked(&port)
    }
}
