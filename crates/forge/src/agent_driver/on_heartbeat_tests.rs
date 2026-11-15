// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use super::test_support::*;
use crate::driver::DriverRegistry;
use crate::types::{Host, Machine, PowerState};
use chrono::Utc;
use pistack_rpc::proto::{Heartbeat, MachineReport};
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{Phase, Resources};
use std::sync::Arc;

#[tokio::test]
async fn registrar_creates_host_and_applies_reports() {
    let store = Store::in_memory().unwrap();
    let drivers = Arc::new(DriverRegistry::new());
    let queue = CommandQueue::default();
    let gauge = gauge::Client::new(store.clone(), Arc::new(gauge::SampleRing::memory()));
    let reg = Registrar::with_gauge(store.clone(), drivers.clone(), queue.clone(), gauge.clone());

    let host = reg.on_hello(&hello("h1")).unwrap();
    assert_eq!(host.spec.node_name, "h1");
    assert_eq!(host.spec.drivers, vec!["kvm", "mock"]);
    assert_eq!(host.meta.labels["rack"], "r1");
    assert_eq!(host.status.phase, Phase::Ready);
    assert!(drivers.get(&host.meta.id).is_some());
    assert!(!drivers.is_local(&host.meta.id));

    // Second hello updates in place rather than duplicating.
    let mut h2 = hello("h1");
    h2.resources.vcpus = 16;
    let again = reg.on_hello(&h2).unwrap();
    assert_eq!(again.meta.id, host.meta.id);
    assert_eq!(again.spec.capacity.vcpus, 16);
    assert_eq!(store.collection::<Host>().list(None).unwrap().len(), 1);

    // A Ready machine on this host picks up the reported address/power.
    let mut m = machine("web");
    m.status.host = Some(host.meta.id.clone());
    m.status.phase = Phase::Ready;
    m.status.power = PowerState::On;
    m.status.addresses = vec!["10.0.0.1".into()];
    let m = store.collection::<Machine>().create(m).unwrap();
    reg.on_heartbeat(&Heartbeat {
        node_name: "h1".into(),
        resources_free: Resources::default(),
        machines: vec![MachineReport {
            id: m.meta.id.clone(),
            phase: Phase::Ready,
            message: String::new(),
            resources: None,
            addresses: vec!["10.0.0.7".into()],
            power: Some(pistack_rpc::proto::PowerState::Off),
            probes: Vec::new(),
        }],
        at: Utc::now(),
        ovn: None,
        usage: None,
    })
    .unwrap();
    let m = store.collection::<Machine>().get(&m.meta.id).unwrap();
    assert_eq!(m.status.addresses, vec!["10.0.0.7"]);
    assert_eq!(m.status.power, PowerState::Off);
    let h = store.collection::<Host>().get(&host.meta.id).unwrap();
    assert!(h.status.last_seen.unwrap() >= host.status.last_seen.unwrap());
    let text = gauge.metrics_text();
    assert!(
        text.lines().any(|l| {
            l.contains("pistack_host_memory_mib")
                && l.contains("host=\"h1\"")
                && (l.contains(" 16384") || l.contains(" 16384.0"))
        }),
        "{text}"
    );
    assert!(
        text.lines().any(|l| {
            l.contains("pistack_host_vcpus")
                && l.contains("host=\"h1\"")
                && (l.ends_with(" 16") || l.ends_with(" 16.0"))
        }),
        "{text}"
    );

    // Heartbeat from an unknown node is ignored, not an error.
    reg.on_heartbeat(&Heartbeat {
        node_name: "ghost".into(),
        resources_free: Resources::default(),
        machines: vec![],
        at: Utc::now(),
        ovn: None,
        usage: None,
    })
    .unwrap();
    assert_eq!(store.collection::<Host>().list(None).unwrap().len(), 1);
    let text = gauge.metrics_text();
    assert!(
        !text.lines().any(|l| l.contains("host=\"ghost\"")),
        "{text}"
    );
}
#[tokio::test]
async fn a_heartbeat_keeps_the_ovn_report_and_fabric_can_read_it() {
    let store = Store::in_memory().unwrap();
    let drivers = Arc::new(DriverRegistry::new());
    let queue = CommandQueue::default();
    let reg = Registrar::new(store.clone(), drivers, queue.clone());
    let host = reg.on_hello(&hello("h1")).unwrap();
    assert_eq!(host.status.ovn, None);
    let col = store.collection::<Host>();
    let beat = |ovn: Option<pistack_rpc::OvnReport>| Heartbeat {
        node_name: "h1".into(),
        resources_free: Resources::default(),
        machines: vec![],
        at: Utc::now(),
        ovn,
        usage: None,
    };

    let down = pistack_rpc::OvnReport {
        chassis: "h1".into(),
        ready: false,
        message: "ovn-controller did not answer; is it running?".into(),
        bridge_mappings: "physnet1:br-ex".into(),
    };
    reg.on_heartbeat(&beat(Some(down.clone()))).unwrap();
    let got = col.get(&host.meta.id).unwrap();
    assert_eq!(got.status.ovn.as_ref(), Some(&down));
    let cond = got
        .status
        .conditions
        .iter()
        .find(|c| c.kind == "OvnReady")
        .unwrap();
    assert!(!cond.status);
    assert_eq!(cond.reason, "ChassisNotReady");
    assert!(cond.message.contains("ovn-controller"));
    assert!(!crate::ovn::chassis_ready(&got));
    assert!(fabric::backend::ovn::gateway_candidates(&store)
        .unwrap()
        .is_empty());

    let up = pistack_rpc::OvnReport {
        ready: true,
        message: String::new(),
        ..down
    };
    reg.on_heartbeat(&beat(Some(up))).unwrap();
    let got = col.get(&host.meta.id).unwrap();
    assert!(crate::ovn::chassis_ready(&got));
    let cond = got
        .status
        .conditions
        .iter()
        .find(|c| c.kind == "OvnReady")
        .unwrap();
    assert!(cond.status);
    assert_eq!(cond.reason, "ChassisConnected");
    // The record forge writes is the one fabric picks gateways from.
    assert_eq!(
        fabric::backend::ovn::gateway_candidates(&store).unwrap(),
        vec!["h1".to_string()]
    );

    // An agent that stops reporting OVN clears it.
    reg.on_heartbeat(&beat(None)).unwrap();
    let got = col.get(&host.meta.id).unwrap();
    assert_eq!(got.status.ovn, None);
    assert!(got.status.conditions.iter().all(|c| c.kind != "OvnReady"));
    assert!(fabric::backend::ovn::gateway_candidates(&store)
        .unwrap()
        .is_empty());
}
