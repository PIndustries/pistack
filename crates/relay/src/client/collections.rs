// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The store collections this client reads and writes, and the provider it
//! sends a balancer to.

use crate::types::{Balancer, Listener, Member, Pool};
use pistack_store::Collection;
use std::sync::Arc;

use super::Client;
use crate::provider::LbProvider;
use pistack_types::ApiResult;
impl Client {
    pub fn provider_for(&self, lb: &Balancer) -> ApiResult<Arc<dyn LbProvider>> {
        self.providers.for_balancer(lb)
    }

    pub fn balancers(&self) -> Collection<Balancer> {
        self.store.collection()
    }
    pub fn listeners(&self) -> Collection<Listener> {
        self.store.collection()
    }
    pub fn pools(&self) -> Collection<Pool> {
        self.store.collection()
    }
    pub fn members(&self) -> Collection<Member> {
        self.store.collection()
    }

    // ---- lookups ------------------------------------------------------------
}
