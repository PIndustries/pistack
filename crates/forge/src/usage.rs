// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Machine quota. No project, or no `machines` limit, means unlimited.
//! A disabled project rejects a create. Usage is counted when the row is
//! stored and released when the delete is accepted, once.

use gate::{check_quota, record_usage};
use pistack_store::Store;
use pistack_types::{ApiResult, Id};

const MACHINES: &str = "machines";

/// Refuse the create when the project cannot take another machine.
pub fn admit_create(store: &Store, project: Option<&Id>) -> ApiResult<()> {
    let Some(project) = project else {
        return Ok(());
    };
    check_quota(store, project, MACHINES, 1)
}

/// Count one machine after its row is stored. A bookkeeping failure is logged
/// and does not remove the row.
pub fn note_create(store: &Store, project: Option<&Id>) {
    adjust(store, project, 1);
}

/// Release one machine after the delete annotation is written.
pub fn note_delete(store: &Store, project: Option<&Id>) {
    adjust(store, project, -1);
}

fn adjust(store: &Store, project: Option<&Id>, delta: i64) {
    let Some(project) = project else {
        return;
    };
    if let Err(e) = record_usage(store, project, MACHINES, delta) {
        tracing::warn!(%project, delta, error = %e, "forge: machine usage was not recorded");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gate::{Project, ProjectSpec, ProjectStatus};
    use pistack_types::{kind, Meta};
    use std::collections::BTreeMap;

    fn project(limit: u64) -> Project {
        let mut spec = ProjectSpec::default();
        spec.quotas.insert(MACHINES.into(), limit);
        Project {
            meta: Meta::new(kind::PROJECT, "tight", None),
            spec,
            status: ProjectStatus {
                usage: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn a_full_project_refuses_and_delete_releases_once() {
        let store = Store::in_memory().unwrap();
        let p = store.collection::<Project>().create(project(0)).unwrap();
        let id = p.meta.id.clone();
        assert!(admit_create(&store, Some(&id)).is_err());

        let open = store
            .collection::<Project>()
            .modify(&id, |p| {
                p.spec.quotas.insert(MACHINES.into(), 1);
                Ok(())
            })
            .unwrap();
        assert!(admit_create(&store, Some(&open.meta.id)).is_ok());
        note_create(&store, Some(&id));
        assert!(admit_create(&store, Some(&id)).is_err());
        note_delete(&store, Some(&id));
        assert!(admit_create(&store, Some(&id)).is_ok());
        assert!(admit_create(&store, None).is_ok());
    }
}
