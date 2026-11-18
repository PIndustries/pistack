// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult, Caller};

use super::guarded::Guarded;

impl Guarded for NetworkSpec {
    /// Turning automatic floating IPs on makes machines in every project
    /// reachable from outside, so it is an admin's call. Anyone who may
    /// edit the network can turn it off.
    fn guard(&self, caller: &Caller, cur: Option<&Self>) -> ApiResult<()> {
        let was_on = cur.is_some_and(|c| c.auto_floating_ip);
        if self.auto_floating_ip && !was_on && !caller.is_admin() {
            return Err(ApiError::forbidden(
                "only an admin may turn on auto_floating_ip",
            ));
        }
        Ok(())
    }
}
