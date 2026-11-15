// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::PodSandbox;
use crate::controllers::pod::probes::test_support::*;
use crate::controllers::pod::slab::mounting::test_support::*;
use crate::types::MACHINE_POD_LABEL;
use forge::{new_machine, Machine, MachineKind, MachineSpec};
use pistack_store::Store;
use pistack_types::{kind, Id, Phase};

#[tokio::test]
pub(in crate::controllers::pod) async fn existing_machine_waits_until_the_volume_has_a_path() {
    let store = Store::in_memory().unwrap();
    let vol = put_volume(&store, "data", Phase::Pending, false, None, None, None);
    let pod_id = pod_with_volume(&store, vol.clone());
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
    let machine = store.collection::<Machine>().create(machine).unwrap();
    pass(&store).await;
    let pod = load(&store, &pod_id);
    assert!(
        pod.status.message.contains("waiting for volume data"),
        "{}",
        pod.status.message
    );
    let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
    assert_eq!(
        machine
            .meta
            .annotations
            .get(forge::WAIT_VOLUME_ANNOTATION)
            .map(String::as_str),
        Some("waiting for volume data")
    );
    assert_eq!(machine.spec.restarts, 0);

    let host = Id::new(kind::HOST);
    store
        .collection::<crate::controllers::pod::slab::SlabVolume>()
        .modify(&vol, |v| {
            v.status.phase = Phase::Ready;
            v.status.attached_to = Some(machine.meta.id.clone());
            v.status.backend_ref = Some("/var/lib/pistack/volumes/data".into());
            v.status.host = Some(host.to_string());
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
    let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
    assert!(!machine
        .meta
        .annotations
        .contains_key(forge::WAIT_VOLUME_ANNOTATION));
    assert_eq!(machine.spec.restarts, 0);
    let sandbox: PodSandbox =
        serde_json::from_str(machine.spec.user_data.as_deref().unwrap()).unwrap();
    assert_eq!(
        sandbox
            .volume_paths
            .get(&vol.to_string())
            .map(String::as_str),
        Some("/var/lib/pistack/volumes/data")
    );
    let pin = host.to_string();
    assert_eq!(
        machine
            .meta
            .annotations
            .get(forge::NODE_PIN_ANNOTATION)
            .map(String::as_str),
        Some(pin.as_str())
    );
}
