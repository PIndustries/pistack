// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    pub fn ports_of_device(&self, device: &Id) -> ApiResult<Vec<Port>> {
        Ok(self
            .ports()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|p| p.spec.device.as_ref() == Some(device))
            .collect())
    }
}
