// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gate::quota::{self};
use pistack_store::Store;
use pistack_types::Id;

use super::count::count;

/// Record the count again after a row of `resource` was deleted.
pub(crate) fn released(store: &Store, project: Option<&Id>, resource: &str) {
    let Some(project) = project else {
        return;
    };
    let _admission = quota::admission();
    match count(store, project, resource) {
        Ok(n) => quota::mirror_usage(store, project, &[(resource, n)]),
        Err(e) => {
            tracing::warn!(%project, resource, error = %e, "fabric: usage was not recorded")
        }
    }
}
