// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Host, MachineSpec, ZONE_LABEL};
use pistack_types::{Phase, Resources};
use std::collections::BTreeMap;

use super::*;
use crate::types::{new_host, MachineKind};

fn res(vcpus: u32, memory_mib: u64) -> Resources {
    Resources {
        vcpus,
        memory_mib,
        disk_gib: 100,
        gpus: 0,
    }
}

fn host(name: &str, mem: u64, drivers: &[&str]) -> Host {
    new_host(
        name,
        name,
        res(8, mem),
        drivers.iter().map(|s| s.to_string()).collect(),
        BTreeMap::new(),
    )
}

fn spec(kind: MachineKind) -> MachineSpec {
    MachineSpec {
        kind,
        size: "m.2x4".into(),
        image: "img".into(),
        ..Default::default()
    }
}

const SIZE: Resources = Resources {
    vcpus: 2,
    memory_mib: 4096,
    disk_gib: 20,
    gpus: 0,
};

#[test]
fn filters_not_ready() {
    let mut a = host("a", 32768, &["kvm"]);
    a.status.phase = Phase::Error;
    let b = host("b", 8192, &["kvm"]);
    let got = place(
        &spec(MachineKind::Vm),
        &SIZE,
        &[a, b.clone()],
        &Allocations::new(),
        Strategy::Spread,
    )
    .unwrap();
    assert_eq!(got, b.meta.id);
}

#[test]
fn filters_disabled() {
    let mut a = host("a", 32768, &["kvm"]);
    a.spec.disabled = true;
    let b = host("b", 8192, &["kvm"]);
    let place_one = |hosts: &[Host]| {
        place(
            &spec(MachineKind::Vm),
            &SIZE,
            hosts,
            &Allocations::new(),
            Strategy::Spread,
        )
    };
    assert_eq!(place_one(&[a.clone(), b.clone()]).unwrap(), b.meta.id);
    let err = place_one(std::slice::from_ref(&a)).unwrap_err();
    assert!(err.message.contains("1 disabled"), "{}", err.message);
    assert_eq!(
        err.details.get("disabled").and_then(|v| v.as_u64()),
        Some(1)
    );
    a.spec.disabled = false;
    assert_eq!(place_one(&[a.clone(), b]).unwrap(), a.meta.id);
}

#[test]
fn filters_kind_by_driver() {
    let a = host("a", 32768, &["kvm"]);
    let b = host("b", 8192, &["youki"]);
    let got = place(
        &spec(MachineKind::Container),
        &SIZE,
        &[a.clone(), b.clone()],
        &Allocations::new(),
        Strategy::Spread,
    )
    .unwrap();
    assert_eq!(got, b.meta.id);
    let got = place(
        &spec(MachineKind::Vm),
        &SIZE,
        &[a.clone(), b],
        &Allocations::new(),
        Strategy::Spread,
    )
    .unwrap();
    assert_eq!(got, a.meta.id);
    let mock = host("m", 8192, &["mock"]);
    assert!(place(
        &spec(MachineKind::Metal),
        &SIZE,
        &[mock],
        &Allocations::new(),
        Strategy::Spread
    )
    .is_ok());
}

#[test]
fn filters_host_selector() {
    let mut a = host("a", 32768, &["kvm"]);
    a.meta.labels.insert("zone".into(), "a".into());
    let mut b = host("b", 8192, &["kvm"]);
    b.meta.labels.insert("zone".into(), "b".into());
    let mut s = spec(MachineKind::Vm);
    s.host_selector.insert("zone".into(), "b".into());
    let got = place(
        &s,
        &SIZE,
        &[a.clone(), b.clone()],
        &Allocations::new(),
        Strategy::Spread,
    )
    .unwrap();
    assert_eq!(got, b.meta.id);
    s.host_selector.insert("rack".into(), "9".into());
    let err = place(&s, &SIZE, &[a, b], &Allocations::new(), Strategy::Spread).unwrap_err();
    assert_eq!(
        err.details.get("selector").and_then(|v| v.as_u64()),
        Some(2)
    );
}

#[test]
fn a_zone_is_a_hard_selector_on_the_zone_label() {
    let mut a = host("a", 32768, &["kvm"]);
    a.meta.labels.insert(ZONE_LABEL.into(), "east".into());
    let mut b = host("b", 8192, &["kvm"]);
    b.meta.labels.insert(ZONE_LABEL.into(), "west".into());
    let unlabelled = host("c", 65536, &["kvm"]);
    let hosts = [a, b.clone(), unlabelled];
    let mut s = spec(MachineKind::Vm);
    s.zone = Some("west".into());
    let got = place(&s, &SIZE, &hosts, &Allocations::new(), Strategy::Spread).unwrap();
    assert_eq!(got, b.meta.id);
    s.zone = Some("north".into());
    let err = place(&s, &SIZE, &hosts, &Allocations::new(), Strategy::Spread).unwrap_err();
    assert_eq!(
        err.details.get("selector").and_then(|v| v.as_u64()),
        Some(3)
    );
}

#[test]
fn filters_resources_and_allocations() {
    let a = host("a", 4096, &["kvm"]);
    let b = host("b", 8192, &["kvm"]);
    let mut alloc = Allocations::new();
    // b already has one m.2x4 pending on it: 4096 left, same as a.
    alloc.add(&b.meta.id, "other", SIZE);
    let got = place(
        &spec(MachineKind::Vm),
        &SIZE,
        &[a.clone(), b.clone()],
        &alloc,
        Strategy::Spread,
    )
    .unwrap();
    // tie on memory (0 left either way) -> lowest id; a was created first (uuidv7)
    assert_eq!(got, a.meta.id);
    alloc.add(&a.meta.id, "x", SIZE);
    alloc.add(&b.meta.id, "y", SIZE);
    let err = place(
        &spec(MachineKind::Vm),
        &SIZE,
        &[a, b],
        &alloc,
        Strategy::Spread,
    )
    .unwrap_err();
    assert_eq!(
        err.details.get("resources").and_then(|v| v.as_u64()),
        Some(2)
    );
}

#[test]
fn spread_prefers_most_free_pack_prefers_least() {
    let a = host("a", 32768, &["kvm"]);
    let b = host("b", 8192, &["kvm"]);
    let hosts = [b.clone(), a.clone()];
    assert_eq!(
        place(
            &spec(MachineKind::Vm),
            &SIZE,
            &hosts,
            &Allocations::new(),
            Strategy::Spread
        )
        .unwrap(),
        a.meta.id
    );
    assert_eq!(
        place(
            &spec(MachineKind::Vm),
            &SIZE,
            &hosts,
            &Allocations::new(),
            Strategy::Pack
        )
        .unwrap(),
        b.meta.id
    );
}

#[test]
fn tie_break_is_lowest_id_regardless_of_order() {
    let a = host("a", 8192, &["kvm"]);
    let b = host("b", 8192, &["kvm"]);
    let c = host("c", 8192, &["kvm"]);
    let lowest = [&a, &b, &c]
        .iter()
        .map(|h| h.meta.id.clone())
        .min()
        .unwrap();
    for order in [[&a, &b, &c], [&c, &b, &a], [&b, &c, &a]] {
        let hosts: Vec<Host> = order.iter().map(|h| (*h).clone()).collect();
        assert_eq!(
            place(
                &spec(MachineKind::Vm),
                &SIZE,
                &hosts,
                &Allocations::new(),
                Strategy::Spread
            )
            .unwrap(),
            lowest
        );
    }
}

#[test]
fn affinity_outweighs_memory() {
    let a = host("a", 32768, &["kvm"]);
    let b = host("b", 8192, &["kvm"]);
    let mut alloc = Allocations::new();
    alloc.add(
        &b.meta.id,
        "db",
        Resources {
            vcpus: 1,
            memory_mib: 1024,
            disk_gib: 10,
            gpus: 0,
        },
    );
    let mut s = spec(MachineKind::Vm);
    s.affinity.push("db".into());
    assert_eq!(
        place(&s, &SIZE, &[a.clone(), b.clone()], &alloc, Strategy::Spread).unwrap(),
        b.meta.id
    );
    let mut s = spec(MachineKind::Vm);
    s.anti_affinity.push("db".into());
    let mut alloc2 = Allocations::new();
    alloc2.add(
        &a.meta.id,
        "db",
        Resources {
            vcpus: 1,
            memory_mib: 1024,
            disk_gib: 10,
            gpus: 0,
        },
    );
    assert_eq!(
        place(&s, &SIZE, &[a, b.clone()], &alloc2, Strategy::Spread).unwrap(),
        b.meta.id
    );
}

#[test]
fn no_hosts_is_unavailable() {
    let err = place(
        &spec(MachineKind::Vm),
        &SIZE,
        &[],
        &Allocations::new(),
        Strategy::Spread,
    )
    .unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Unavailable);
}

#[test]
fn place_still_accepts_hosts_without_underlay() {
    let h = host("a", 8192, &["kvm"]);
    assert!(h.spec.underlay_ip.is_none());
    assert!(place(
        &spec(MachineKind::Vm),
        &SIZE,
        &[h],
        &Allocations::new(),
        Strategy::Spread
    )
    .is_ok());
}

#[test]
fn place_on_segment_rejects_a_host_with_no_underlay() {
    let h = host("a", 32768, &["kvm"]);
    assert_eq!(h.status.phase, Phase::Ready);
    let err = place_on_segment(
        &spec(MachineKind::Vm),
        &SIZE,
        std::slice::from_ref(&h),
        &Allocations::new(),
        Strategy::Spread,
    )
    .unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Unavailable);
    assert!(err.message.contains("segment"), "{}", err.message);
    assert!(err.message.contains("1"), "{}", err.message);
    assert!(err.message.contains("underlay"), "{}", err.message);
    assert!(place(
        &spec(MachineKind::Vm),
        &SIZE,
        &[h],
        &Allocations::new(),
        Strategy::Spread
    )
    .is_ok());
}

#[test]
fn place_on_segment_accepts_a_private_underlay() {
    for ip in [
        "10.1.0.2",
        "172.16.0.1",
        "192.168.1.1",
        "100.64.0.1",
        "100.127.255.254",
    ] {
        let mut h = host("a", 32768, &["kvm"]);
        h.spec.underlay_ip = Some(ip.into());
        let got = place_on_segment(
            &spec(MachineKind::Vm),
            &SIZE,
            &[h.clone()],
            &Allocations::new(),
            Strategy::Spread,
        )
        .unwrap_or_else(|e| panic!("{ip}: {e}"));
        assert_eq!(got, h.meta.id, "{ip}");
    }
}

#[test]
fn place_on_segment_rejects_public_and_unspecified() {
    for ip in [
        "8.8.8.8",
        "0.0.0.0",
        "127.0.0.1",
        "255.255.255.255",
        "224.0.0.1",
        "100.128.0.1",
        "169.254.1.1",
    ] {
        let mut h = host("a", 32768, &["kvm"]);
        h.spec.underlay_ip = Some(ip.into());
        let err = place_on_segment(
            &spec(MachineKind::Vm),
            &SIZE,
            &[h],
            &Allocations::new(),
            Strategy::Spread,
        )
        .unwrap_err();
        assert!(err.message.contains("segment"), "{ip}: {}", err.message);
    }
}

fn chassis(ready: bool, message: &str) -> pistack_rpc::OvnReport {
    pistack_rpc::OvnReport {
        chassis: "a".into(),
        ready,
        message: message.into(),
        bridge_mappings: String::new(),
    }
}

#[test]
fn place_on_ovn_takes_only_a_connected_chassis_with_an_underlay() {
    let place_one = |hosts: &[Host]| {
        place_on_ovn(
            &spec(MachineKind::Vm),
            &SIZE,
            hosts,
            &Allocations::new(),
            Strategy::Spread,
        )
    };
    // No report at all, a report that says not ready, and a ready
    // chassis with no tunnel endpoint.
    let mut silent = host("silent", 32768, &["kvm"]);
    silent.spec.underlay_ip = Some("10.1.0.2".into());
    let mut down = host("down", 32768, &["kvm"]);
    down.spec.underlay_ip = Some("10.1.0.3".into());
    down.status
        .set_ovn(Some(&chassis(false, "ovn-controller did not answer")));
    let mut bare = host("bare", 32768, &["kvm"]);
    bare.status.set_ovn(Some(&chassis(true, "")));

    let err = place_one(&[silent.clone(), down.clone(), bare.clone()]).unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Unavailable);
    assert!(err.message.contains("OVN"), "{}", err.message);
    assert_eq!(err.details.get("hosts").and_then(|v| v.as_u64()), Some(3));
    assert_eq!(
        err.details.get("no_underlay").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        err.details.get("ovn_not_ready").and_then(|v| v.as_u64()),
        Some(2)
    );

    // The one ready host wins although it has the least memory.
    let mut ready = host("ready", 8192, &["kvm"]);
    ready.spec.underlay_ip = Some("10.1.0.4".into());
    ready.status.set_ovn(Some(&chassis(true, "")));
    let got = place_one(&[silent, down.clone(), bare, ready.clone()]).unwrap();
    assert_eq!(got, ready.meta.id);

    // A chassis that comes back is usable again.
    down.status.set_ovn(Some(&chassis(true, "")));
    assert_eq!(
        place_one(std::slice::from_ref(&down)).unwrap(),
        down.meta.id
    );

    // Hosts that qualify still go through the ordinary filters.
    let mut pods_only = host("pods", 32768, &["youki"]);
    pods_only.spec.underlay_ip = Some("10.1.0.5".into());
    pods_only.status.set_ovn(Some(&chassis(true, "")));
    let err = place_one(&[pods_only]).unwrap_err();
    assert!(!err.message.contains("OVN"), "{}", err.message);
    assert_eq!(
        err.details.get("no_driver").and_then(|v| v.as_u64()),
        Some(1)
    );
}

#[test]
fn place_on_segment_returns_place_errors_for_hosts_on_the_segment() {
    let mut h = host("a", 8192, &["youki"]);
    h.spec.underlay_ip = Some("10.1.0.2".into());
    let err = place_on_segment(
        &spec(MachineKind::Vm),
        &SIZE,
        &[h],
        &Allocations::new(),
        Strategy::Spread,
    )
    .unwrap_err();
    assert!(!err.message.contains("segment"), "{}", err.message);
    assert_eq!(
        err.details.get("no_driver").and_then(|v| v.as_u64()),
        Some(1)
    );
}
