// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{NODE_PIN_ANNOTATION, WAIT_VOLUME_ANNOTATION};
use pistack_types::{ApiError, Resources};

use super::{pinned_hosts, volume_wait, waits_for};

#[test]
fn a_start_that_is_ahead_of_fabric_waits_and_any_other_failure_does_not() {
    let unbound = ApiError::unavailable("machine mach-1 has a port that is not bound in OVN yet");
    assert_eq!(waits_for(&unbound), Some("PortBinding"));
    let segment = ApiError::unavailable("overlay network net-1 has no segment");
    assert_eq!(waits_for(&segment), Some("Segment"));
    assert_eq!(waits_for(&ApiError::unavailable("host is not ready")), None);
    assert_eq!(
        waits_for(&ApiError::internal("port is not bound in OVN yet")),
        None
    );
}

use crate::types::{new_host, new_machine, MachineSpec};

#[test]
fn wait_annotation_is_a_message_and_a_pin_keeps_one_host() {
    let mut machine = new_machine("pod-web", None, MachineSpec::default());
    assert!(volume_wait(&machine).is_none());
    machine.meta.annotations.insert(
        WAIT_VOLUME_ANNOTATION.into(),
        "waiting for volume data".into(),
    );
    assert_eq!(volume_wait(&machine), Some("waiting for volume data"));
    machine
        .meta
        .annotations
        .insert(WAIT_VOLUME_ANNOTATION.into(), String::new());
    assert!(volume_wait(&machine).is_none());

    let cap = Resources {
        vcpus: 1,
        memory_mib: 512,
        ..Resources::default()
    };
    let a = new_host("a", "a", cap, vec!["youki".into()], Default::default());
    let b = new_host("b", "b", cap, vec!["youki".into()], Default::default());
    let hosts = vec![a.clone(), b];
    assert!(pinned_hosts(&machine, &hosts).is_none());
    machine
        .meta
        .annotations
        .insert(NODE_PIN_ANNOTATION.into(), a.meta.id.to_string());
    let pinned = pinned_hosts(&machine, &hosts).unwrap();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].meta.id, a.meta.id);
}

#[test]
fn a_network_note_is_a_false_condition_and_clears() {
    let mut machine = new_machine("pod-web", None, MachineSpec::default());
    let message = "pod has no network: a rootless agent cannot attach the recorded address. Run the agent as root or set PISTACK_YOUKI_SUDO=1";
    super::note_network(&mut machine.status, message);
    assert_eq!(machine.status.message, message);
    let condition = machine
        .status
        .conditions
        .iter()
        .find(|c| c.kind == "Network")
        .unwrap();
    assert!(!condition.status);
    assert_eq!(condition.reason, "Rootless");
    assert_eq!(condition.message, message);
    super::note_network(&mut machine.status, "");
    assert!(machine
        .status
        .conditions
        .iter()
        .all(|c| c.kind != "Network"));
}
