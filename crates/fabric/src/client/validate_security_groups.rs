// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use pistack_types::{ApiResult, Id, Meta};
impl Client {
    pub(in crate::client) fn validate_security_groups(
        &self,
        meta: &Meta,
        groups: &[Id],
    ) -> ApiResult<()> {
        for g in groups {
            let sg = self.security_groups().get(g).map_err(store_err)?;
            check_same_project(meta, &sg.meta, "security group")?;
        }
        Ok(())
    }
}
