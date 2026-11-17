// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The two lookups every member handler runs first: the pool must be in the
//! caller's scope, and the member must belong to it.

use super::body::ScopeQuery;
use super::scope::scope;
use super::visible::visible;
use crate::client::Client;
use crate::types::{Member, Pool};
use pistack_types::{ApiError, ApiResult, Caller};

pub(super) fn pool_in_scope(
    c: &Client,
    caller: &Caller,
    sq: &ScopeQuery,
    id: &str,
) -> ApiResult<Pool> {
    let project = scope(caller, sq)?;
    visible(caller, c.pools().resolve(project.as_ref(), id)?)
}

pub(super) fn member_of_pool(c: &Client, pool: &Pool, member: &str) -> ApiResult<Member> {
    let m = c.members().resolve(pool.meta.project.as_ref(), member)?;
    if m.spec.pool != pool.meta.id {
        return Err(ApiError::not_found(format!(
            "member {member} not found in pool {}",
            pool.meta.id
        )));
    }
    Ok(m)
}
