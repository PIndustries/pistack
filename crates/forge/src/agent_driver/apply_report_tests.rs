// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use super::test_support::*;
use crate::driver::DriverRegistry;
use crate::types::{Machine, PowerState, EVACUATED_FROM_ANNOTATION};
use chrono::Utc;
use pistack_rpc::proto::{Heartbeat, MachineReport, ProbeReport};
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{Phase, Resources};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn heartbeat_copies_probe_streaks_onto_machine_status() {
    let store = Store::in_memory().unwrap();
    let drivers = Arc::new(DriverRegistry::new());
    let queue = CommandQueue::default();
    let reg = Registrar::new(store.clone(), drivers, queue);
    let host = reg.on_hello(&hello("h1")).unwrap();

    let mut m = machine("web");
    m.status.host = Some(host.meta.id.clone());
    m.status.phase = Phase::Ready;
    m.status.power = PowerState::On;
    m.status.addresses = vec!["10.0.0.7".into()];
    let m = store.collection::<Machine>().create(m).unwrap();
    let report = ProbeReport {
        container: "web".into(),
        kind: pistack_rpc::PROBE_READINESS.into(),
        success: false,
        successes: 0,
        failures: 2,
        message: "http status 500".into(),
        unsupported: false,
    };
    reg.on_heartbeat(&Heartbeat {
        node_name: "h1".into(),
        resources_free: Resources::default(),
        machines: vec![MachineReport {
            id: m.meta.id.clone(),
            phase: Phase::Ready,
            message: String::new(),
            resources: None,
            addresses: vec!["10.0.0.7".into()],
            power: Some(pistack_rpc::proto::PowerState::On),
            probes: vec![report.clone()],
        }],
        at: Utc::now(),
        ovn: None,
        usage: None,
    })
    .unwrap();
    let stored = store.collection::<Machine>().get(&m.meta.id).unwrap();
    assert_eq!(stored.status.addresses, vec!["10.0.0.7"]);
    assert_eq!(stored.status.power, PowerState::On);
    assert_eq!(stored.status.probes, vec![report]);
}
#[tokio::test]
async fn a_machine_migrated_back_to_the_host_it_was_evacuated_off_is_kept() {
    let store = Store::in_memory().unwrap();
    let queue = CommandQueue::default();
    let reg = Registrar::new(
        store.clone(),
        Arc::new(DriverRegistry::new()),
        queue.clone(),
    );
    let old = reg.on_hello(&hello("old")).unwrap();
    let new = reg.on_hello(&hello("new")).unwrap();
    let col = store.collection::<Machine>();
    let mut m = placed("back", &new.meta.id, &[]);
    m.meta
        .annotations
        .insert(EVACUATED_FROM_ANNOTATION.into(), old.meta.id.to_string());
    m.status.phase = Phase::Updating;
    m.status.message = format!("migrating {}", old.meta.id);
    let m = col.create(m).unwrap();

    // The destination of a live migration reports the machine before the
    // store says it lives there. That is an arrival, not a stale copy.
    let beat = reports("old", &[&m.meta.id]);
    reg.on_heartbeat(&beat).unwrap();
    assert!(queue
        .next("old", Duration::from_millis(50), 8)
        .await
        .is_empty());
    let got = col.get(&m.meta.id).unwrap();
    assert_eq!(got.status.host.as_ref(), Some(&old.meta.id));
    assert_eq!(got.status.phase, Phase::Ready);

    // Once it lives there again the marker goes, so a later move off
    // this host is not mistaken for the old evacuation.
    reg.on_heartbeat(&beat).unwrap();
    let got = col.get(&m.meta.id).unwrap();
    assert!(!got.meta.annotations.contains_key(EVACUATED_FROM_ANNOTATION));
    assert!(queue
        .next("old", Duration::from_millis(50), 8)
        .await
        .is_empty());
}
