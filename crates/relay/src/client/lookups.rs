// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Finding the resources that belong to another one: a balancer's listeners
//! and pools, a pool's members, and the listeners that use a pool.

use super::store_err::store_err;
use crate::types::{Listener, Member, Pool};
use pistack_types::{ApiResult, Id};

use super::Client;
impl Client {
    pub fn listeners_of(&self, balancer: &Id) -> ApiResult<Vec<Listener>> {
        Ok(self
            .listeners()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|l| &l.spec.balancer == balancer)
            .collect())
    }

    pub fn pools_of(&self, balancer: &Id) -> ApiResult<Vec<Pool>> {
        Ok(self
            .pools()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|p| &p.spec.balancer == balancer)
            .collect())
    }

    pub fn members_of(&self, pool: &Id) -> ApiResult<Vec<Member>> {
        let mut v: Vec<Member> = self
            .members()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|m| &m.spec.pool == pool)
            .collect();
        v.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
        Ok(v)
    }

    /// Listeners whose default pool or one of whose routes is `pool`.
    pub fn listeners_using_pool(&self, pool: &Id) -> ApiResult<Vec<Listener>> {
        Ok(self
            .listeners()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|l| {
                l.spec.default_pool.as_ref() == Some(pool)
                    || l.spec.routes.iter().any(|r| &r.pool == pool)
            })
            .collect())
    }
}
