// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gate::quota::{self, BUCKETS};
use pistack_store::Store;
use pistack_types::Id;

use super::count::count;

/// Record the count again after a bucket was deleted.
pub(crate) fn released(store: &Store, project: Option<&Id>) {
    let Some(project) = project else {
        return;
    };
    let _admission = quota::admission();
    match count(store, project) {
        Ok(n) => quota::mirror_usage(store, project, &[(BUCKETS, n)]),
        Err(e) => tracing::warn!(%project, error = %e, "silo: usage was not recorded"),
    }
}
