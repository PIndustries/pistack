// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, released};
use pistack_types::{ApiResult, Id};
impl Client {
    pub fn delete_floating_ip(&self, id: &Id) -> ApiResult<FloatingIp> {
        let fip = self.floating_ips().delete(id).map_err(store_err)?;
        released(&self.store, fip.meta.project.as_ref(), usage::FLOATING_IPS);
        Ok(fip)
    }
}
