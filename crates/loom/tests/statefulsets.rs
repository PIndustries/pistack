// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! StatefulSet controller: stable ordinals, ordered scale, volume claims.

use loom::controllers::{statefulset::StatefulSetController, Controller};
use loom::{
    set_condition, Container, Pod, PodManagementPolicy, PodPhase, PodSpec, PodTemplate,
    StatefulSet, StatefulSetSpec, VolumeClaimTemplate, VolumeSource, OWNER_LABEL, POD_NAME_LABEL,
    POD_ORDINAL_LABEL, POD_READY, POD_TEMPLATE_HASH,
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

/// Running and Ready: what the pod controller writes for a pod with no
/// readiness probe once its containers run.
fn mark_running(store: &Store, pod: &Pod) {
    set_running(store, pod, true);
}

/// Running but held back by a readiness probe that has not passed.
fn mark_running_unready(store: &Store, pod: &Pod) {
    set_running(store, pod, false);
}

fn set_running(store: &Store, pod: &Pod, ready: bool) {
    store
        .collection::<Pod>()
        .modify(&pod.meta.id, |p| {
            p.status.phase = PodPhase::Running;
            p.status.pod_ip = Some("10.0.0.1".into());
            let reason = if ready {
                "Running"
            } else {
                "ContainersNotReady"
            };
            set_condition(&mut p.status.conditions, POD_READY, ready, reason, "");
            Ok(())
        })
        .unwrap();
}

fn spec(replicas: u32, policy: PodManagementPolicy) -> StatefulSetSpec {
    StatefulSetSpec {
        replicas,
        selector: Default::default(),
        template: template(),
        service_name: String::new(),
        volume_claim_templates: vec![],
        pod_management_policy: policy,
        update_strategy: Default::default(),
    }
}

fn by_name(store: &Store, name: &str) -> Option<Pod> {
    store
        .collection::<Pod>()
        .list(None)
        .unwrap()
        .into_iter()
        .find(|p| p.meta.name == name)
}

/// Names of owned pods not marked for deletion, sorted.
fn live_names(store: &Store) -> Vec<String> {
    let mut v: Vec<String> = store
        .collection::<Pod>()
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|p| !loom::delete_requested(p))
        .map(|p| p.meta.name)
        .collect();
    v.sort();
    v
}

fn hash_of(p: &Pod) -> String {
    p.meta.labels.get(POD_TEMPLATE_HASH).unwrap().clone()
}

/// Finish deletes the way the pod controller would: drop pods marked for
/// deletion from the store.
fn reap(store: &Store) {
    let pods = store.collection::<Pod>();
    for p in pods.list(None).unwrap() {
        if loom::delete_requested(&p) {
            pods.delete(&p.meta.id).unwrap();
        }
    }
}

/// Bring every ordinal of an OrderedReady set up Running and Ready.
async fn bring_up(store: &Store, ctl: &StatefulSetController, n: usize) {
    for _ in 0..=n {
        ctl.reconcile_once().await.unwrap();
        for p in store.collection::<Pod>().list(None).unwrap() {
            mark_running(store, &p);
        }
    }
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(store).len(), n);
}

fn new_image(store: &Store, sts: &StatefulSet) {
    store
        .collection::<StatefulSet>()
        .modify(&sts.meta.id, |s| {
            s.spec.template.spec.containers[0].image = "postgres:17".into();
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

#[tokio::test]
async fn ordered_ready_waits_for_ready_not_just_running() {
    let store = Store::in_memory().unwrap();
    let sets = store.collection::<StatefulSet>();
    let ctl = StatefulSetController::new(store.clone());
    let sts = sets
        .create(Resource::new(
            "db",
            None,
            spec(3, PodManagementPolicy::OrderedReady),
        ))
        .unwrap();

    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0"]);

    // Running, but its readiness probe has not passed: db-1 waits.
    mark_running_unready(&store, &by_name(&store, "db-0").unwrap());
    for _ in 0..3 {
        ctl.reconcile_once().await.unwrap();
    }
    assert_eq!(live_names(&store), vec!["db-0"]);
    let s = sets.get(&sts.meta.id).unwrap();
    assert_eq!(s.status.ready, 0, "Running is not Ready");

    // Ready: the next ordinal is created.
    mark_running(&store, &by_name(&store, "db-0").unwrap());
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0", "db-1"]);
    assert_eq!(sets.get(&sts.meta.id).unwrap().status.ready, 1);
}

#[tokio::test]
async fn ordered_rolling_update_waits_for_each_replacement_to_be_ready() {
    let store = Store::in_memory().unwrap();
    let sets = store.collection::<StatefulSet>();
    let ctl = StatefulSetController::new(store.clone());
    let sts = sets
        .create(Resource::new(
            "db",
            None,
            spec(3, PodManagementPolicy::OrderedReady),
        ))
        .unwrap();
    bring_up(&store, &ctl, 3).await;
    let old_hash = hash_of(&by_name(&store, "db-0").unwrap());

    new_image(&store, &sts);
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0", "db-1"], "highest first");
    reap(&store);

    // db-2 comes back on the new template.
    ctl.reconcile_once().await.unwrap();
    let db2 = by_name(&store, "db-2").unwrap();
    assert_ne!(hash_of(&db2), old_hash);

    // Pending, then Running but unready: db-1 is left alone.
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0", "db-1", "db-2"]);
    mark_running_unready(&store, &db2);
    for _ in 0..3 {
        ctl.reconcile_once().await.unwrap();
    }
    assert_eq!(live_names(&store), vec!["db-0", "db-1", "db-2"]);

    // Ready: the roll moves on to db-1.
    mark_running(&store, &db2);
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0", "db-2"]);
    reap(&store);
    ctl.reconcile_once().await.unwrap();
    let db1 = by_name(&store, "db-1").unwrap();
    assert_ne!(hash_of(&db1), old_hash);
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0", "db-1", "db-2"]);

    // And on to db-0 once db-1 is ready, then done.
    mark_running(&store, &db1);
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-1", "db-2"]);
    reap(&store);
    ctl.reconcile_once().await.unwrap();
    mark_running(&store, &by_name(&store, "db-0").unwrap());
    ctl.reconcile_once().await.unwrap();
    let s = sets.get(&sts.meta.id).unwrap();
    assert_eq!(
        (s.status.replicas, s.status.ready, s.status.updated),
        (3, 3, 3)
    );
}

#[tokio::test]
async fn ordered_rolling_update_replaces_an_unready_old_pod_first() {
    let store = Store::in_memory().unwrap();
    let sets = store.collection::<StatefulSet>();
    let ctl = StatefulSetController::new(store.clone());
    let sts = sets
        .create(Resource::new(
            "db",
            None,
            spec(3, PodManagementPolicy::OrderedReady),
        ))
        .unwrap();
    bring_up(&store, &ctl, 3).await;

    // db-0 on the old template stops being ready: it serves nothing, so it
    // is replaced first rather than wedging the roll.
    mark_running_unready(&store, &by_name(&store, "db-0").unwrap());
    new_image(&store, &sts);
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-1", "db-2"]);
    reap(&store);
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0", "db-1", "db-2"]);
    // Then the usual order: the new db-0 must be ready before db-2 goes.
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0", "db-1", "db-2"]);
    mark_running(&store, &by_name(&store, "db-0").unwrap());
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["db-0", "db-1"]);
}

#[tokio::test]
async fn parallel_rolling_update_does_not_wait_for_ready() {
    let store = Store::in_memory().unwrap();
    let sets = store.collection::<StatefulSet>();
    let ctl = StatefulSetController::new(store.clone());
    let sts = sets
        .create(Resource::new(
            "web",
            None,
            spec(3, PodManagementPolicy::Parallel),
        ))
        .unwrap();
    ctl.reconcile_once().await.unwrap();
    for p in store.collection::<Pod>().list(None).unwrap() {
        mark_running_unready(&store, &p);
    }
    new_image(&store, &sts);
    ctl.reconcile_once().await.unwrap();
    assert!(live_names(&store).is_empty(), "every stale pod in one pass");
    reap(&store);
    ctl.reconcile_once().await.unwrap();
    assert_eq!(live_names(&store), vec!["web-0", "web-1", "web-2"]);
    assert_eq!(sets.get(&sts.meta.id).unwrap().status.ready, 0);
}
