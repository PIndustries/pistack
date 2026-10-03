// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The ReplicaSet controller: keep exactly `spec.replicas` pods alive.
//!
//! Pods are owned through `OWNER_LABEL`. Orphan pods that match the
//! selector are adopted; surplus pods are deleted oldest first; missing ones
//! are created from the template as `<rs>-<5 random>`.

use super::{
    delete_quietly, live_pods, owned_by, patch_status, pod_from_template, request_delete,
    Controller,
};
use crate::types::{delete_requested, set_condition, Pod, PodPhase, ReplicaSet, OWNER_LABEL};
use pistack_store::{Collection, Store};

pub struct ReplicaSetController {
    store: Store,
}

impl ReplicaSetController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    fn pods(&self) -> Collection<Pod> {
        self.store.collection::<Pod>()
    }
    fn sets(&self) -> Collection<ReplicaSet> {
        self.store.collection::<ReplicaSet>()
    }

    fn teardown(&self, rs: &ReplicaSet, pods: &[Pod]) -> anyhow::Result<()> {
        let mut remaining = 0;
        for p in owned_by(pods, &rs.meta.id) {
            remaining += 1;
            if !delete_requested(p) {
                request_delete(&self.pods(), &p.meta.id)?;
            }
        }
        if remaining == 0 {
            delete_quietly(&self.sets(), &rs.meta.id)?;
            tracing::info!(rs = %rs.meta.id, name = %rs.meta.name, "loom: replicaset deleted");
        }
        Ok(())
    }

    fn adopt_orphans(&self, rs: &ReplicaSet, pods: &[Pod]) -> anyhow::Result<()> {
        for p in pods.iter().filter(|p| {
            p.meta.project == rs.meta.project
                && !p.meta.labels.contains_key(OWNER_LABEL)
                && !delete_requested(p)
                && rs.spec.selector.matches(&p.meta.labels)
        }) {
            let owner = rs.meta.id.to_string();
            self.pods().modify(&p.meta.id, |x| {
                x.meta.labels.insert(OWNER_LABEL.into(), owner.clone());
                Ok(())
            })?;
            tracing::info!(rs = %rs.meta.id, pod = %p.meta.id, "loom: adopted orphan pod");
        }
        Ok(())
    }

    fn converge(&self, rs: &ReplicaSet, pods: &[Pod]) -> anyhow::Result<()> {
        self.adopt_orphans(rs, pods)?;
        let live = live_pods(pods, &rs.meta.id);
        let want = rs.spec.replicas as usize;
        let have = live.len();

        if have < want {
            for _ in have..want {
                let pod = pod_from_template(
                    &rs.meta.name,
                    rs.meta.project.clone(),
                    &rs.spec.template,
                    &rs.meta.id,
                );
                let pod = self.pods().create(pod)?;
                tracing::info!(rs = %rs.meta.id, pod = %pod.meta.id, name = %pod.meta.name, "loom: pod created");
            }
        } else if have > want {
            // Oldest first; among equals prefer not-yet-running pods.
            let mut victims = live.clone();
            victims.sort_by(|a, b| {
                let ra = a.status.phase == PodPhase::Running;
                let rb = b.status.phase == PodPhase::Running;
                ra.cmp(&rb).then(a.meta.created_at.cmp(&b.meta.created_at))
            });
            for p in victims.iter().take(have - want) {
                request_delete(&self.pods(), &p.meta.id)?;
                tracing::info!(rs = %rs.meta.id, pod = %p.meta.id, "loom: pod delete requested (scale down)");
            }
        }

        let replicas = have as u32;
        let ready = live
            .iter()
            .filter(|p| p.status.phase == PodPhase::Running)
            .count() as u32;
        let target = rs.spec.replicas;
        patch_status(&self.sets(), &rs.meta.id, |s| {
            s.replicas = replicas;
            s.ready = ready;
            s.available = ready;
            let ok = replicas == target && ready == target;
            set_condition(
                &mut s.conditions,
                "Ready",
                ok,
                if ok {
                    "AllReplicasReady"
                } else {
                    "Progressing"
                },
                &format!("{ready}/{target} ready"),
            );
        })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Controller for ReplicaSetController {
    fn name(&self) -> &'static str {
        "replicaset"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[crate::types::kinds::POD, crate::types::kinds::REPLICA_SET]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let sets = self.sets().list(None)?;
        let pods = self.pods().list(None)?;
        for rs in &sets {
            let res = if delete_requested(rs) {
                self.teardown(rs, &pods)
            } else {
                self.converge(rs, &pods)
            };
            if let Err(e) = res {
                tracing::warn!(rs = %rs.meta.id, error = %e, "loom: replicaset reconcile failed");
            }
        }
        Ok(())
    }
}
