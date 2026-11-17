// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Whether this client's reconcile paths must not program a balancer, which
//! is true only for the ones loom labels as its own.

use crate::types::{Balancer, MANAGED_LABEL};

use super::Client;
impl Client {
    /// True when this client's reconcile paths must not program `lb`.
    pub(crate) fn leaves(&self, lb: &Balancer) -> bool {
        self.leave_loom && lb.meta.labels.get(MANAGED_LABEL).map(String::as_str) == Some("loom")
    }
}
