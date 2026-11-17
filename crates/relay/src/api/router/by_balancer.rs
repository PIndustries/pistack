// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Narrowing a list to one balancer's children by name or id.

use super::super::body::ScopeQuery;
use crate::client::Client;
use pistack_types::{ApiResult, Id, Kind};

/// Listeners and pools both belong to a balancer, so both can be listed
/// through one of them by name or id.
pub(super) fn by_balancer<K: Kind>(
    c: &Client,
    project: Option<&Id>,
    sq: &ScopeQuery,
    items: &mut Vec<K>,
    balancer_of: fn(&K) -> &Id,
) -> ApiResult<()> {
    if let Some(b) = &sq.balancer {
        let lb = c.balancers().resolve(project, b)?;
        items.retain(|k| balancer_of(k) == &lb.meta.id);
    }
    Ok(())
}
