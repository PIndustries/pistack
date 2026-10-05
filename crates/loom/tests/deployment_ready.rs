// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Deployment rolling updates wait for new pods to be Ready, not just
//! Running. Drives the Deployment and ReplicaSet controllers by hand and
//! writes pod status the way the pod controller does.

use loom::controllers::{
    deployment::DeploymentController, replicaset::ReplicaSetController, Controller,
};
use loom::{
    set_condition, Container, Deployment, DeploymentSpec, DeploymentStrategy, IntOrPercent, Pod,
    PodPhase, PodSpec, PodTemplate, ReplicaSet, Selector, POD_READY,
};
use pistack_store::Store;
use pistack_types::Resource;

fn template(image: &str) -> PodTemplate {
    PodTemplate {
        labels: [("app".into(), "web".into())].into(),
        annotations: Default::default(),
        spec: PodSpec {
            containers: vec![Container {
                name: "web".into(),
                image: image.into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    }
}

struct Rig {
    store: Store,
    deploy: DeploymentController,
    rs: ReplicaSetController,
}

impl Rig {
    fn new() -> Self {
        let store = Store::in_memory().unwrap();
        Self {
            deploy: DeploymentController::new(store.clone()),
            rs: ReplicaSetController::new(store.clone()),
            store,
        }
    }

    /// A few rounds of both controllers, so status and spec settle.
    async fn settle(&self) {
        for _ in 0..3 {
            self.deploy.reconcile_once().await.unwrap();
            self.rs.reconcile_once().await.unwrap();
        }
    }

    fn live(&self) -> Vec<Pod> {
        self.store
            .collection::<Pod>()
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|p| !loom::delete_requested(p))
            .collect()
    }

    /// Live pods of each image: (old, new).
    fn counts(&self) -> (usize, usize) {
        let live = self.live();
        let n = |img: &str| {
            live.iter()
                .filter(|p| p.spec.containers[0].image == img)
                .count()
        };
        (n("web:1"), n("web:2"))
    }

    /// spec.replicas of the set running each image: (old, new).
    fn set_sizes(&self) -> (u32, u32) {
        let sets = self.store.collection::<ReplicaSet>().list(None).unwrap();
        let n = |img: &str| {
            sets.iter()
                .find(|rs| rs.spec.template.spec.containers[0].image == img)
                .map(|rs| rs.spec.replicas)
                .unwrap_or(0)
        };
        (n("web:1"), n("web:2"))
    }

    fn set_status_ready(&self, img: &str) -> u32 {
        self.store
            .collection::<ReplicaSet>()
            .list(None)
            .unwrap()
            .into_iter()
            .find(|rs| rs.spec.template.spec.containers[0].image == img)
            .map(|rs| rs.status.ready)
            .unwrap_or(0)
    }

    /// Mark every live pod of `img` Running, Ready or not.
    fn run(&self, img: &str, ready: bool) {
        let pods = self.store.collection::<Pod>();
        for p in self.live() {
            if p.spec.containers[0].image != img {
                continue;
            }
            pods.modify(&p.meta.id, |p| {
                p.status.phase = PodPhase::Running;
                p.status.pod_ip = Some("10.0.0.9".into());
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
    }

    /// A two-replica deployment on `web:1`, every pod Running and Ready,
    /// then switched to `web:2`.
    async fn rolling(&self, max_surge: u32, max_unavailable: u32) -> Deployment {
        let d = self
            .store
            .collection::<Deployment>()
            .create(Resource::new(
                "web",
                None,
                DeploymentSpec {
                    replicas: 2,
                    selector: Selector {
                        match_labels: [("app".into(), "web".into())].into(),
                        match_expressions: vec![],
                    },
                    template: template("web:1"),
                    strategy: DeploymentStrategy::RollingUpdate {
                        max_surge: IntOrPercent::Int(max_surge),
                        max_unavailable: IntOrPercent::Int(max_unavailable),
                    },
                    revision_history_limit: 3,
                    paused: false,
                },
            ))
            .unwrap();
        self.settle().await;
        self.run("web:1", true);
        self.settle().await;
        assert_eq!(self.counts(), (2, 0));
        let d = self
            .store
            .collection::<Deployment>()
            .modify(&d.meta.id, |d| {
                d.spec.template = template("web:2");
                Ok(())
            })
            .unwrap();
        self.settle().await;
        d
    }
}

#[tokio::test]
async fn surge_holds_until_the_new_pod_is_ready() {
    let rig = Rig::new();
    let d = rig.rolling(1, 0).await;

    // One surge pod, old pods untouched while it starts.
    assert_eq!(rig.set_sizes(), (2, 1));
    assert_eq!(rig.counts(), (2, 1));

    // Running, but its readiness probe has not passed: still held.
    rig.run("web:2", false);
    rig.settle().await;
    assert_eq!(rig.set_sizes(), (2, 1), "Running alone must not advance");
    assert_eq!(rig.counts(), (2, 1));
    assert_eq!(rig.set_status_ready("web:2"), 0);
    let status = rig
        .store
        .collection::<Deployment>()
        .get(&d.meta.id)
        .unwrap()
        .status;
    assert_eq!(status.available, 2, "only the old pods are available");

    // Ready: one old pod goes, the next surge pod comes.
    rig.run("web:2", true);
    rig.settle().await;
    assert_eq!(rig.set_sizes(), (1, 2));
    assert_eq!(rig.counts(), (1, 2));

    // The second new pod holds the last old one until it is ready too.
    rig.run("web:2", false);
    rig.settle().await;
    assert_eq!(rig.set_sizes(), (1, 2));
    rig.run("web:2", true);
    rig.settle().await;
    assert_eq!(rig.set_sizes(), (0, 2));
    assert_eq!(rig.counts(), (0, 2));
    let status = rig
        .store
        .collection::<Deployment>()
        .get(&d.meta.id)
        .unwrap()
        .status;
    assert_eq!((status.updated, status.ready, status.available), (2, 2, 2));
}

#[tokio::test]
async fn max_unavailable_counts_only_ready_pods() {
    let rig = Rig::new();
    rig.rolling(0, 1).await;

    // No surge: one old pod may go, then nothing more until a new one is ready.
    assert_eq!(rig.set_sizes(), (1, 1));
    assert_eq!(rig.counts(), (1, 1));
    rig.run("web:2", false);
    rig.settle().await;
    assert_eq!(rig.set_sizes(), (1, 1), "unready new pod frees no old pod");

    rig.run("web:2", true);
    rig.settle().await;
    assert_eq!(rig.set_sizes(), (0, 2));
    rig.run("web:2", true);
    rig.settle().await;
    assert_eq!(rig.counts(), (0, 2));
}
