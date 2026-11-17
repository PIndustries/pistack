// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::ApiResult;

use super::applied::Applied;
use super::delete_owned::delete_owned;
use super::delete_rank::delete_rank;

/// Delete managed ids the document no longer names, dependents first.
pub(crate) fn delete_orphans(
    store: &Store,
    fabric: &fabric::Client,
    relay: &relay::Client,
    beacon: &beacon::Client,
    orphans: &[String],
    out: &mut Applied,
) -> ApiResult<()> {
    let mut ordered = orphans.to_vec();
    ordered.sort_by_key(|id| (delete_rank(id), id.clone()));
    for id in ordered {
        match delete_owned(store, fabric, relay, beacon, &id)? {
            true => out.deleted.push(id),
            false => out.left.push(id),
        }
    }
    Ok(())
}
