// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, admitted};
use pistack_types::{kind, ApiResult, Id, Meta};
impl Client {
    pub fn create_security_group(
        &self,
        mut meta: Meta,
        spec: SecurityGroupSpec,
    ) -> ApiResult<SecurityGroup> {
        meta.id = Id::new(kind::SECGROUP);
        self.validate_rules(&meta, &spec.rules, Some(&meta.id))?;
        let project = meta.project.clone();
        admitted(
            &self.store,
            project.as_ref(),
            usage::SECURITY_GROUPS,
            || {
                self.security_groups()
                    .create(SecurityGroup {
                        meta,
                        spec,
                        status: SecurityGroupStatus::default(),
                    })
                    .map_err(store_err)
            },
        )
    }
}
