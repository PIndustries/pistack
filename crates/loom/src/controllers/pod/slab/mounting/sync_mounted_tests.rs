// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::PodSandbox;
use crate::controllers::pod::probes::test_support::*;
use crate::controllers::pod::slab::mounting::test_support::*;
use crate::types::{PodPhase, MACHINE_POD_LABEL};
use forge::{new_machine, Machine, MachineKind, MachineSpec, PowerState};
use pistack_store::Store;
use pistack_types::{kind, Id, Phase};

#[tokio::test]
pub(in crate::controllers::pod) async fn pod_waits_until_the_slab_volume_is_attached() {
    let store = Store::in_memory().unwrap();
    let vol = put_volume(&store, "data", Phase::Pending, false, None, None, None);
    let pod = pod_with_volume(&store, vol.clone());
    pass(&store).await;
    let got = load(&store, &pod);
    assert_eq!(got.status.phase, PodPhase::Pending);
    assert!(
        got.status.message.contains("data"),
        "{}",
        got.status.message
    );
    assert!(
        got.status.message.contains("waiting for volume"),
        "{}",
        got.status.message
    );
    assert!(store.collection::<Machine>().list(None).unwrap().is_empty());

    store
        .collection::<crate::controllers::pod::slab::SlabVolume>()
        .modify(&vol, |v| {
            v.status.phase = Phase::Ready;
            v.status.attached_to = Some(Id::new(kind::MACHINE));
            v.status
                .conditions
                .push(crate::controllers::pod::slab::SlabCondition {
                    kind: "Attached".into(),
                    status: true,
                });
            Ok(())
        })
        .unwrap();
    pass(&store).await;
    let got = load(&store, &pod);
    assert!(
        got.status.message.contains("data"),
        "{}",
        got.status.message
    );
    assert!(store.collection::<Machine>().list(None).unwrap().is_empty());

    let host = Id::new(kind::HOST);
    let path = "/var/lib/pistack/volumes/data";
    store
        .collection::<crate::controllers::pod::slab::SlabVolume>()
        .modify(&vol, |v| {
            v.status.backend_ref = Some(path.into());
            v.status.host = Some(host.to_string());
            Ok(())
        })
        .unwrap();
    pass(&store).await;
    let machines = store.collection::<Machine>().list(None).unwrap();
    assert_eq!(machines.len(), 1);
    let machine = &machines[0];
    let pin = host.to_string();
    assert_eq!(
        machine
            .meta
            .annotations
            .get(forge::NODE_PIN_ANNOTATION)
            .map(String::as_str),
        Some(pin.as_str())
    );
    assert!(machine
        .meta
        .annotations
        .get(super::MOUNTED_VOLUMES)
        .unwrap()
        .contains(&vol.to_string()));
    let sandbox: PodSandbox =
        serde_json::from_str(machine.spec.user_data.as_deref().unwrap()).unwrap();
    assert_eq!(
        sandbox
            .volume_paths
            .get(&vol.to_string())
            .map(String::as_str),
        Some(path)
    );
    assert!(!sandbox.volume_devices.contains_key(&vol.to_string()));
}
#[tokio::test]
pub(in crate::controllers::pod) async fn device_only_slab_volume_starts_without_a_host_path() {
    let store = Store::in_memory().unwrap();
    let host = Id::new(kind::HOST);
    let vol = put_volume(
        &store,
        "data",
        Phase::Ready,
        true,
        Some("vdb"),
        Some("rbd-image"),
        Some(&host),
    );
    let pod = pod_with_volume(&store, vol.clone());
    pass(&store).await;
    let machines = store.collection::<Machine>().list(None).unwrap();
    assert_eq!(machines.len(), 1);
    let sandbox: PodSandbox =
        serde_json::from_str(machines[0].spec.user_data.as_deref().unwrap()).unwrap();
    assert_eq!(
        sandbox
            .volume_devices
            .get(&vol.to_string())
            .map(String::as_str),
        Some("vdb")
    );
    assert!(sandbox.volume_paths.is_empty());
    let got = load(&store, &pod);
    assert_eq!(got.status.machine.as_ref(), Some(&machines[0].meta.id));
}
#[tokio::test]
pub(in crate::controllers::pod) async fn late_slab_attach_recreates_a_running_pod() {
    let store = Store::in_memory().unwrap();
    let host = Id::new(kind::HOST);
    let vol = put_volume(
        &store,
        "data",
        Phase::Ready,
        true,
        Some("vdb"),
        Some("/var/lib/pistack/volumes/data"),
        Some(&host),
    );
    let pod_id = pod_with_volume(&store, vol.clone());
    let mut machine = new_machine(
        "pod-web",
        None,
        MachineSpec {
            kind: MachineKind::Container,
            size: "small".into(),
            image: "img".into(),
            networks: vec!["default".into()],
            user_data: Some("{}".into()),
            ..MachineSpec::default()
        },
    );
    machine
        .meta
        .labels
        .insert(MACHINE_POD_LABEL.into(), pod_id.to_string());
    machine.status.phase = Phase::Ready;
    machine.status.power = PowerState::On;
    machine.status.host = Some(host);
    let machine = store.collection::<Machine>().create(machine).unwrap();
    pass(&store).await;
    let pod = load(&store, &pod_id);
    assert_eq!(pod.status.phase, PodPhase::Pending);
    assert!(
        pod.status.message.contains("data"),
        "{}",
        pod.status.message
    );
    assert!(!pod.status.is_ready());
    let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
    assert_eq!(machine.spec.restarts, 1);
    let sandbox: PodSandbox =
        serde_json::from_str(machine.spec.user_data.as_deref().unwrap()).unwrap();
    assert_eq!(
        sandbox
            .volume_paths
            .get(&vol.to_string())
            .map(String::as_str),
        Some("/var/lib/pistack/volumes/data")
    );
    pass(&store).await;
    let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
    assert_eq!(machine.spec.restarts, 1);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn slab_volume_on_another_node_is_not_accepted() {
    let store = Store::in_memory().unwrap();
    let here = Id::new(kind::HOST);
    let there = Id::new(kind::HOST);
    let vol = put_volume(
        &store,
        "data",
        Phase::Ready,
        true,
        None,
        Some("/var/lib/pistack/volumes/data"),
        Some(&there),
    );
    let pod_id = pod_with_volume(&store, vol);
    let mut machine = new_machine(
        "pod-web",
        None,
        MachineSpec {
            kind: MachineKind::Container,
            size: "small".into(),
            image: "img".into(),
            networks: vec!["default".into()],
            ..MachineSpec::default()
        },
    );
    machine
        .meta
        .labels
        .insert(MACHINE_POD_LABEL.into(), pod_id.to_string());
    machine.status.phase = Phase::Ready;
    machine.status.power = PowerState::On;
    machine.status.host = Some(here);
    let machine = store.collection::<Machine>().create(machine).unwrap();
    pass(&store).await;
    let pod = load(&store, &pod_id);
    assert_eq!(pod.status.phase, PodPhase::Pending);
    assert!(
        pod.status.message.contains("not attached on this node"),
        "{}",
        pod.status.message
    );
    assert!(
        pod.status.message.contains("data"),
        "{}",
        pod.status.message
    );
    let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
    assert_eq!(machine.spec.restarts, 0);
}
