// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    pub fn update_security_group(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut SecurityGroup) -> ApiResult<()>,
    ) -> ApiResult<SecurityGroup> {
        let cur = self.security_groups().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        self.validate_rules(&next.meta, &next.spec.rules, Some(id))?;
        self.security_groups()
            .update(next, expected)
            .map_err(store_err)
    }
}
