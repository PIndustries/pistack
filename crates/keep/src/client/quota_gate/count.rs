// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::access_key::{ACCESS_KEYS, ROLE_ACCESS_KEY};
use crate::client::helpers::managed_role;
use crate::client::Client;
use crate::error::Result;
use crate::quota::CERTIFICATES;
use crate::types::{Certificate, StoredSecret};

impl Client {
    /// Rows of `resource` in `project` that count against its quota. A
    /// certificate's key secret counts as part of the certificate; an
    /// access key counts under `access_keys`, not `secrets`.
    pub(crate) fn count(&self, project: &Id, resource: &str) -> Result<u64> {
        let n = if resource == CERTIFICATES {
            self.store
                .collection::<Certificate>()
                .list(Some(project))?
                .len()
        } else {
            let want = (resource == ACCESS_KEYS).then_some(ROLE_ACCESS_KEY);
            self.store
                .collection::<StoredSecret>()
                .list(Some(project))?
                .iter()
                .filter(|s| managed_role(&s.meta.labels) == want)
                .count()
        };
        Ok(n as u64)
    }
}
