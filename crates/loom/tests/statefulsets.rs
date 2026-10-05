// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! StatefulSet controller: stable ordinals, ordered scale, volume claims.

use loom::controllers::{statefulset::StatefulSetController, Controller};
use loom::{
    Container, Pod, PodManagementPolicy, PodPhase, PodSpec, PodTemplate, StatefulSet,
    StatefulSetSpec, VolumeClaimTemplate, VolumeSource, OWNER_LABEL, POD_NAME_LABEL,
    POD_ORDINAL_LABEL, POD_TEMPLATE_HASH,
};
use pistack_store::Store;
use pistack_types::{kind, Phase, Resource, Spec};

fn template() -> PodTemplate {
    PodTemplate {
        labels: [("app".into(), "db".into())].into(),
        annotations: Default::default(),
        spec: PodSpec {
            containers: vec![Container {
                name: "db".into(),
                image: "postgres:16".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    }
}

fn mark_running(store: &Store, pod: &Pod) {
    store
        .collection::<Pod>()
        .modify(&pod.meta.id, |p| {
            p.status.phase = PodPhase::Running;
            p.status.pod_ip = Some("10.0.0.1".into());
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn ordered_scale_up_and_down() {
    let store = Store::in_memory().unwrap();
    let sets = store.collection::<StatefulSet>();
    let pods = store.collection::<Pod>();
    let ctl = StatefulSetController::new(store.clone());

    let sts = sets
        .create(Resource::new(
            "db",
            None,
            StatefulSetSpec {
                replicas: 3,
                selector: Default::default(),
                template: template(),
                service_name: "db".into(),
                volume_claim_templates: vec![],
                pod_management_policy: PodManagementPolicy::OrderedReady,
                update_strategy: Default::default(),
            },
        ))
        .unwrap();

    // First pass: only ordinal 0.
    ctl.reconcile_once().await.unwrap();
    let list = pods.list(None).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].meta.name, "db-0");
    assert_eq!(list[0].meta.labels.get(POD_ORDINAL_LABEL).unwrap(), "0");
    assert_eq!(list[0].meta.labels.get(POD_NAME_LABEL).unwrap(), "db-0");
    assert_eq!(
        list[0].meta.labels.get(OWNER_LABEL).unwrap(),
        &sts.meta.id.to_string()
    );
    assert!(list[0].meta.labels.contains_key(POD_TEMPLATE_HASH));
    assert_eq!(
        list[0]
            .meta
            .annotations
            .get("loom.pistack.io/service-name")
            .unwrap(),
        "db"
    );

    // Not Running yet: still only one pod.
    ctl.reconcile_once().await.unwrap();
    assert_eq!(pods.list(None).unwrap().len(), 1);

    mark_running(&store, &pods.list(None).unwrap()[0]);
    ctl.reconcile_once().await.unwrap();
    assert_eq!(pods.list(None).unwrap().len(), 2);
    assert!(pods
        .list(None)
        .unwrap()
        .iter()
        .any(|p| p.meta.name == "db-1"));

    for p in pods.list(None).unwrap() {
        mark_running(&store, &p);
    }
    ctl.reconcile_once().await.unwrap();
    let list = pods.list(None).unwrap();
    assert_eq!(list.len(), 3);
    let mut names: Vec<_> = list.iter().map(|p| p.meta.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["db-0", "db-1", "db-2"]);

    for p in pods.list(None).unwrap() {
        mark_running(&store, &p);
    }
    ctl.reconcile_once().await.unwrap();
    let sts = sets.get(&sts.meta.id).unwrap();
    assert_eq!(sts.status.replicas, 3);
    assert_eq!(sts.status.ready, 3);

    // Scale down: deletes highest ordinal first.
    sets.modify(&sts.meta.id, |s| {
        s.spec.replicas = 1;
        Ok(())
    })
    .unwrap();
    ctl.reconcile_once().await.unwrap();
    let live: Vec<_> = pods
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|p| !p.meta.annotations.contains_key(loom::DELETE_ANNOTATION))
        .map(|p| p.meta.name)
        .collect();
    // OrderedReady: only one delete per pass.
    assert!(live.contains(&"db-0".into()));
    assert_eq!(live.len(), 2, "one ordinal deleted per pass: {live:?}");

    ctl.reconcile_once().await.unwrap();
    let live: Vec<_> = pods
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|p| !p.meta.annotations.contains_key(loom::DELETE_ANNOTATION))
        .map(|p| p.meta.name)
        .collect();
    assert_eq!(live, vec!["db-0".to_string()]);
}

#[tokio::test]
async fn parallel_creates_all_ordinals() {
    let store = Store::in_memory().unwrap();
    let sets = store.collection::<StatefulSet>();
    let pods = store.collection::<Pod>();
    let ctl = StatefulSetController::new(store.clone());

    sets.create(Resource::new(
        "web",
        None,
        StatefulSetSpec {
            replicas: 3,
            selector: Default::default(),
            template: template(),
            service_name: String::new(),
            volume_claim_templates: vec![],
            pod_management_policy: PodManagementPolicy::Parallel,
            update_strategy: Default::default(),
        },
    ))
    .unwrap();

    ctl.reconcile_once().await.unwrap();
    let mut names: Vec<_> = pods
        .list(None)
        .unwrap()
        .into_iter()
        .map(|p| p.meta.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["web-0", "web-1", "web-2"]);
}

#[tokio::test]
async fn volume_claim_templates_create_volumes() {
    let store = Store::in_memory().unwrap();
    let sets = store.collection::<StatefulSet>();
    let pods = store.collection::<Pod>();
    let ctl = StatefulSetController::new(store.clone());

    sets.create(Resource::new(
        "pg",
        None,
        StatefulSetSpec {
            replicas: 1,
            selector: Default::default(),
            template: template(),
            service_name: String::new(),
            volume_claim_templates: vec![VolumeClaimTemplate {
                name: "data".into(),
                size_gib: 10,
                volume_type: "standard".into(),
            }],
            pod_management_policy: PodManagementPolicy::Parallel,
            update_strategy: Default::default(),
        },
    ))
    .unwrap();

    ctl.reconcile_once().await.unwrap();

    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    struct VolSpec {
        size_gib: u64,
        #[serde(default)]
        volume_type: String,
    }
    impl Spec for VolSpec {
        const KIND: &'static str = kind::VOLUME;
    }
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct VolStatus {
        #[serde(default)]
        phase: Phase,
    }
    type Vol = Resource<VolSpec, VolStatus>;

    let vols = store.collection::<Vol>().list(None).unwrap();
    assert_eq!(vols.len(), 1);
    assert_eq!(vols[0].meta.name, "pg-data-0");
    assert_eq!(vols[0].spec.size_gib, 10);

    let pod = &pods.list(None).unwrap()[0];
    assert_eq!(pod.meta.name, "pg-0");
    let data = pod
        .spec
        .volumes
        .iter()
        .find(|v| v.name == "data")
        .expect("data volume");
    match &data.source {
        VolumeSource::Volume(id) => assert_eq!(id, &vols[0].meta.id),
        other => panic!("expected Volume, got {other:?}"),
    }
}
