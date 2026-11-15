// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_types::Resources;
use serde_json::json;

use crate::proto::{AgentEvent, AgentHello, CommandKind, Heartbeat, MAX_LONG_POLL_SECS};
use crate::queue::test_support::*;

#[tokio::test]
async fn hello_and_heartbeat_update_node_info() {
    let q = CommandQueue::new(cfg(60));
    let ack = q.hello(&AgentHello {
        node_name: "n1".into(),
        capabilities: vec!["kvm".into()],
        resources: Resources {
            vcpus: 8,
            memory_mib: 16384,
            disk_gib: 100,
            gpus: 0,
        },
        version: "26.11.12.1".into(),
        labels: Default::default(),
        underlay_ip: Some("10.66.66.5".into()),
    });
    assert_eq!(ack.node_name, "n1");
    assert_eq!(ack.heartbeat_interval_secs, 5);
    assert_eq!(ack.max_long_poll_secs, MAX_LONG_POLL_SECS);

    let _t = q
        .enqueue("n1", CommandKind::StartMachine, json!(null))
        .unwrap();
    let hb = q.heartbeat(&Heartbeat {
        node_name: "n1".into(),
        resources_free: Resources {
            vcpus: 6,
            memory_mib: 8192,
            disk_gib: 50,
            gpus: 0,
        },
        machines: vec![],
        at: Utc::now(),
        ovn: None,
        usage: None,
    });
    assert!(hb.known);
    assert_eq!(hb.commands_waiting, 1);

    let info = q.node("n1").unwrap();
    assert_eq!(info.capabilities, vec!["kvm"]);
    assert_eq!(info.resources_total.vcpus, 8);
    assert_eq!(info.resources_free.vcpus, 6);
    assert_eq!(info.underlay_ip.as_deref(), Some("10.66.66.5"));
    assert!(info.last_hello.is_some() && info.last_heartbeat.is_some());
    assert_eq!(info.queued, 1);
}

#[tokio::test]
async fn hello_and_heartbeat_are_broadcast() {
    let q = CommandQueue::new(cfg(60));
    let mut rx = q.subscribe_agent_events();
    let hello = AgentHello {
        node_name: "n1".into(),
        capabilities: vec!["mock".into()],
        resources: Resources::default(),
        version: String::new(),
        labels: Default::default(),
        underlay_ip: None,
    };
    q.hello(&hello);
    let hb = Heartbeat {
        node_name: "n1".into(),
        resources_free: Resources::default(),
        machines: vec![],
        at: Utc::now(),
        ovn: None,
        usage: None,
    };
    q.heartbeat(&hb);
    assert_eq!(rx.recv().await.unwrap(), AgentEvent::Hello(hello));
    let got = rx.recv().await.unwrap();
    assert_eq!(got.node_name(), "n1");
    assert_eq!(got, AgentEvent::Heartbeat(hb));
}
