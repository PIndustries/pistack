// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gate::quota::{self, Demand};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiResult, Id};

use super::count::count;

/// Run `create`, which stores one more `resource` in `project`, unless the
/// project is at its limit. `create` must not come back here: the lock is
/// held while it runs.
pub(crate) fn admitted<T>(
    store: &Store,
    project: Option<&Id>,
    resource: &str,
    create: impl FnOnce() -> ApiResult<T>,
) -> ApiResult<T> {
    let Some(project) = project else {
        return create();
    };
    let _admission = quota::admission();
    let used = count(store, project, resource)?;
    match quota::admit(store, project, &[Demand::new(resource, used, 1)]) {
        Err(e) if e.code == ErrorCode::NotFound => {}
        other => other?,
    }
    let out = create()?;
    quota::mirror_usage(store, project, &[(resource, used + 1)]);
    Ok(out)
}
