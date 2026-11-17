// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The member pass: settle its status and let its pool refresh through it.

use crate::client::Client;
use pistack_types::Id;
use pistack_types::Phase;

impl Client {
    /// A member change (spec, or the health checker flipping its status)
    /// re-programs the listeners that use its pool.
    pub async fn reconcile_member(&self, id: &Id) {
        let m = match self.members().try_get(id) {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(member = %id, error = %e, "relay: read failed");
                return;
            }
        };
        if m.status.observed_revision == m.meta.revision && m.status.phase == Phase::Ready {
            return;
        }
        let _ = self.members().modify(id, |x| {
            x.status.phase = Phase::Ready;
            x.status.observed_revision = x.meta.revision + 1;
            Ok(())
        });
        self.touch_pool(&m.spec.pool);
    }
}
