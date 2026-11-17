// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The first pass: every balancer, in dependency order.

use crate::client::Client;

impl Client {
    /// Everything, in dependency order.
    pub async fn reconcile_all(&self) {
        let lbs = self.balancers().list(None).unwrap_or_default();
        for lb in lbs {
            self.reconcile_balancer(&lb.meta.id).await;
        }
    }
}
