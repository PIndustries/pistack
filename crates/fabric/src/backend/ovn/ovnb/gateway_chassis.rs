// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::{gateway_candidates::gateway_candidates, ovn_backend::OvnBackend};
use pistack_types::ApiResult;

impl OvnBackend {
    /// Chassis for gateway ports: the configured list when there is one,
    /// else the hosts that report they can carry one.
    pub(in crate::backend::ovn) fn gateway_chassis(&self) -> ApiResult<Vec<String>> {
        if !self.gateway_chassis.is_empty() {
            return Ok(self.gateway_chassis.clone());
        }
        match self.attached.get() {
            Some(a) => gateway_candidates(&a.store),
            None => Ok(Vec::new()),
        }
    }
}
