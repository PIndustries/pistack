// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::client::Client;
use crate::quota;

impl Client {
    /// Mirror the current count into the project's usage.
    pub(crate) fn note_usage(&self, project: Option<&Id>, resource: &str) {
        let Some(p) = project else {
            return;
        };
        match self.count(p, resource) {
            Ok(n) => quota::record(&self.store, p, resource, n),
            Err(e) => {
                tracing::warn!(project = %p, resource, error = %e, "keep: usage was not counted")
            }
        }
    }
}
