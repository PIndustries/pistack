// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The listener pass: read it, then program it at the provider.

use crate::client::Client;
use pistack_types::Id;

impl Client {
    pub async fn reconcile_listener(&self, id: &Id) {
        let l = match self.listeners().try_get(id) {
            Ok(Some(l)) => l,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(listener = %id, error = %e, "relay: read failed");
                return;
            }
        };
        self.program_listener(&l, true).await;
    }
}
