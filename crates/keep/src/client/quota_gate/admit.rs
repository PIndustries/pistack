// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::access_key::ACCESS_KEYS;
use crate::client::Client;
use crate::error::Result;
use crate::quota::{self, CERTIFICATES};

impl Client {
    /// Refuse one more `resource` in `project` at its limit. Call with
    /// [`quota::admission`] held when a create follows.
    pub(crate) fn admit(&self, project: Option<&Id>, resource: &str) -> Result<()> {
        let Some(p) = project else {
            return Ok(());
        };
        let default = match resource {
            CERTIFICATES => self.limits.max_certificates_per_project,
            ACCESS_KEYS => self.limits.max_access_keys_per_project,
            _ => self.limits.max_secrets_per_project,
        };
        quota::admit(&self.store, p, resource, self.count(p, resource)?, default)
    }
}
