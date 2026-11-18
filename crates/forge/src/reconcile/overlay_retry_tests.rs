// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::{overlay_due, overlay_failed, OverlaySent, OVERLAY_RETRY};
use std::time::Instant;

#[test]
fn a_plan_goes_again_on_change_new_hello_or_old_failure() {
    let hello = Some(Utc::now());
    assert!(overlay_due(None, "a", hello));
    let sent = OverlaySent {
        body: "a".into(),
        hello,
        failed: None,
    };
    // Same plan, same agent, no failure: nothing to do.
    assert!(!overlay_due(Some(&sent), "a", hello));
    // The plan changed.
    assert!(overlay_due(Some(&sent), "b", hello));
    // The agent restarted and said hello again.
    let later = Some(Utc::now() + chrono::Duration::seconds(5));
    assert!(overlay_due(Some(&sent), "a", later));
    // A fresh failure waits; an old one goes again.
    let fresh = OverlaySent {
        failed: Some(Instant::now()),
        ..sent.clone()
    };
    assert!(!overlay_due(Some(&fresh), "a", hello));
    if let Some(old) = Instant::now().checked_sub(OVERLAY_RETRY) {
        let old = OverlaySent {
            failed: Some(old),
            ..sent
        };
        assert!(overlay_due(Some(&old), "a", hello));
    }
}

#[tokio::test]
async fn taking_the_lead_forgets_what_was_sent() {
    let store = pistack_store::Store::in_memory().unwrap();
    let drivers = Arc::new(crate::driver::DriverRegistry::new());
    let r = super::Reconciler::new(store, drivers, crate::scheduler::Strategy::default());
    r.overlay_sent.lock().insert(
        "sim2".to_owned(),
        OverlaySent {
            body: "a".into(),
            hello: None,
            failed: None,
        },
    );
    let active = pistack_types::CancellationToken::new();
    active.cancel();
    r.run_loop(active).await;
    assert!(r.overlay_sent.lock().is_empty());
}

#[test]
fn a_failure_on_an_older_plan_does_not_mark_the_newer_one() {
    let map = Arc::new(parking_lot::Mutex::new(BTreeMap::new()));
    map.lock().insert(
        "sim2".to_owned(),
        OverlaySent {
            body: "new".into(),
            hello: None,
            failed: None,
        },
    );
    overlay_failed(&map, "sim2", "old");
    assert!(map.lock()["sim2"].failed.is_none());
    overlay_failed(&map, "sim2", "new");
    assert!(map.lock()["sim2"].failed.is_some());
}
