// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The pool write paths: a pool must name a balancer in this project.

use super::pool::validate_pool;
use super::project::check_same_project;
use super::store_err::store_err;
use crate::types::{kinds, Pool, PoolSpec};
use pistack_types::{ApiError, ApiResult, Id, Meta};

use super::Client;
use crate::types::PoolStatus;
impl Client {
    pub fn create_pool(&self, mut meta: Meta, spec: PoolSpec) -> ApiResult<Pool> {
        meta.id = Id::new(kinds::POOL);
        let lb = self.balancers().get(&spec.balancer).map_err(store_err)?;
        check_same_project(&meta, &lb.meta, "balancer")?;
        validate_pool(&spec)?;
        self.pools()
            .create(Pool {
                meta,
                spec,
                status: PoolStatus::default(),
            })
            .map_err(store_err)
    }

    pub fn update_pool(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Pool) -> ApiResult<()>,
    ) -> ApiResult<Pool> {
        let cur = self.pools().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.balancer != cur.spec.balancer {
            return Err(ApiError::bad_request("pool balancer is immutable"));
        }
        if next.spec.protocol != cur.spec.protocol && !self.listeners_using_pool(id)?.is_empty() {
            return Err(ApiError::conflict(
                "pool protocol cannot change while a listener uses it",
            ));
        }
        validate_pool(&next.spec)?;
        self.pools().update(next, expected).map_err(store_err)
    }

    /// Delete a pool and its members. Refused while a listener points at it.
    pub fn delete_pool(&self, id: &Id) -> ApiResult<Pool> {
        let users = self.listeners_using_pool(id)?;
        if let Some(l) = users.first() {
            return Err(ApiError::conflict(format!(
                "pool {id} is used by listener {}",
                l.meta.id
            )));
        }
        for m in self.members_of(id)? {
            self.members().delete(&m.meta.id).map_err(store_err)?;
        }
        self.pools().delete(id).map_err(store_err)
    }

    // ---- members -----------------------------------------------------------------
}
