// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::denied::denied;
use super::limited::limited;
use super::mk::mk;
use crate::quota::SECRETS;
use pistack_store::Store;
use pistack_types::kind;

#[cfg(test)]
#[test]
fn gate_project_quota_overrides_the_default_and_records_usage() {
    let c = limited(3, 10);
    let store = c.store().clone();
    let mut spec = gate::ProjectSpec::default();
    spec.quotas.insert(SECRETS.into(), 5);
    let p = store
        .collection::<gate::Project>()
        .create(gate::Project {
            meta: pistack_types::Meta::new(kind::PROJECT, "roomy", None),
            spec,
            status: Default::default(),
        })
        .unwrap()
        .meta
        .id;
    let mut last = None;
    for i in 0..5 {
        last = Some(c.create_secret(mk(&format!("s{i}"), &p), b"x").unwrap());
    }
    assert!(denied(c.create_secret(mk("s5", &p), b"x"))
        .message
        .contains("roomy"));
    let usage = |store: &Store| {
        store
            .collection::<gate::Project>()
            .get(&p)
            .unwrap()
            .status
            .usage
            .get(SECRETS)
            .copied()
    };
    assert_eq!(usage(&store), Some(5));
    c.delete_secret(&last.unwrap().meta.id).unwrap();
    assert_eq!(usage(&store), Some(4));

    // A disabled project refuses creates.
    store
        .collection::<gate::Project>()
        .modify(&p, |x| {
            x.spec.enabled = false;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        denied(c.create_secret(mk("off", &p), b"x")).code,
        pistack_types::error::ErrorCode::Forbidden
    );
}
