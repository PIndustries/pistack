// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::default_sg::DEFAULT_SG;
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_store::StoreError;
use pistack_types::{kind, ApiResult, Meta};
impl Client {
    /// Create the cloud-wide template "default" group if it is missing.
    /// Allow all egress; allow ingress from members of the same group.
    pub fn ensure_default_template(&self) -> ApiResult<SecurityGroup> {
        match self.security_groups().get_by_name(None, DEFAULT_SG) {
            Ok(sg) => return Ok(sg),
            Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(store_err(e)),
        }
        let mut meta = Meta::new(kind::SECGROUP, DEFAULT_SG, None);
        meta.description = "Template for each project's default security group".into();
        let me = meta.id.clone();
        let spec = SecurityGroupSpec {
            rules: default_rules(&me),
        };
        match self.security_groups().create(SecurityGroup {
            meta,
            spec,
            status: SecurityGroupStatus::default(),
        }) {
            Ok(sg) => Ok(sg),
            // Lost a race with another writer; theirs wins.
            Err(StoreError::Conflict(_)) => self
                .security_groups()
                .get_by_name(None, DEFAULT_SG)
                .map_err(store_err),
            Err(e) => Err(store_err(e)),
        }
    }
}
