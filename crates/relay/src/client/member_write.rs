// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The member write paths, and the member set the data planes are allowed
//! to dial.

use super::member::validate_member;
use super::pool::initial_status;
use super::store_err::store_err;
use crate::types::{kinds, Member, MemberSpec, OperatingStatus, Pool};
use pistack_types::{ApiError, ApiResult, Id, Meta};

use super::project::check_same_project;
use super::Client;
use crate::types::MemberStatus;
impl Client {
    pub fn create_member(&self, mut meta: Meta, spec: MemberSpec) -> ApiResult<Member> {
        meta.id = Id::new(kinds::MEMBER);
        let pool = self.pools().get(&spec.pool).map_err(store_err)?;
        check_same_project(&meta, &pool.meta, "pool")?;
        validate_member(&spec)?;
        for other in self.members_of(&spec.pool)? {
            if other.spec.address == spec.address && other.spec.port == spec.port {
                return Err(ApiError::conflict(format!(
                    "{}:{} is already member {}",
                    spec.address, spec.port, other.meta.id
                )));
            }
        }
        let status = MemberStatus {
            operating_status: initial_status(&pool, &spec),
            ..MemberStatus::default()
        };
        self.members()
            .create(Member { meta, spec, status })
            .map_err(store_err)
    }

    pub fn update_member(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Member) -> ApiResult<()>,
    ) -> ApiResult<Member> {
        let cur = self.members().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.pool != cur.spec.pool {
            return Err(ApiError::bad_request("member pool is immutable"));
        }
        validate_member(&next.spec)?;
        if next.spec.address != cur.spec.address || next.spec.port != cur.spec.port {
            for other in self.members_of(&cur.spec.pool)? {
                if other.meta.id != *id
                    && other.spec.address == next.spec.address
                    && other.spec.port == next.spec.port
                {
                    return Err(ApiError::conflict(format!(
                        "{}:{} is already member {}",
                        next.spec.address, next.spec.port, other.meta.id
                    )));
                }
            }
        }
        if !next.spec.admin_up && cur.spec.admin_up {
            next.status.operating_status = OperatingStatus::Draining;
        }
        self.members().update(next, expected).map_err(store_err)
    }

    pub fn delete_member(&self, id: &Id) -> ApiResult<Member> {
        let m = self.members().delete(id).map_err(store_err)?;
        self.touch_pool(&m.spec.pool);
        Ok(m)
    }

    /// Members of a pool that should take traffic right now. Everything the
    /// data planes dial starts here, so no algorithm can reach a member that
    /// is administratively down, `Offline`, `Draining` or weighted zero.
    pub fn active_members(&self, pool: &Pool) -> ApiResult<Vec<Member>> {
        Ok(crate::picker::eligible(&self.members_of(&pool.meta.id)?)
            .into_iter()
            .cloned()
            .collect())
    }
}
