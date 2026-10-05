// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The Deployment controller: one ReplicaSet per pod-template hash.
//!
//! The ReplicaSet for the current template is `<deploy>-<hash>` and carries
//! `pod-template-hash=<hash>` on itself and on its pods. `RollingUpdate`
//! scales the new set up within `max_surge` and old sets down within
//! `max_unavailable`; `Recreate` scales every old set to zero first. Old
//! sets at zero replicas are kept for rollback up to
//! `revision_history_limit`, newest first.
//!
//! A rolling update only counts pods that are Running and Ready
//! ([`super::pod_ready`], i.e. `PodStatus::is_ready`) as available. New
//! pods still starting, or held back by a readiness probe, keep the old
//! pods in place: the surge stays used and `max_unavailable` is measured
//! against ready pods, so the roll moves on only as new pods turn ready.
//! A pod with no readiness probe is ready once its containers run.

use super::{
    delete_quietly, live_pods, owned_by, patch_status, pod_ready, request_delete, Controller,
};
use crate::selector::template_hash;
use crate::types::{
    delete_requested, set_condition, Deployment, DeploymentStrategy, Pod, ReplicaSet,
    ReplicaSetSpec, ReplicaSetStatus, OWNER_LABEL, POD_TEMPLATE_HASH, REVISION_ANNOTATION,
};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::Resource;

pub struct DeploymentController {
    store: Store,
}

/// Rollout revision recorded on a ReplicaSet (0 when absent).
pub fn revision_of(rs: &ReplicaSet) -> u64 {
    rs.meta
        .annotations
        .get(REVISION_ANNOTATION)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Owned ReplicaSets of a deployment that are not being deleted, oldest
/// first. Shared with the rollback handler.
pub fn owned_sets(sets: &[ReplicaSet], d: &Deployment) -> Vec<ReplicaSet> {
    let mut v: Vec<ReplicaSet> = owned_by(sets, &d.meta.id)
        .filter(|rs| !delete_requested(rs))
        .cloned()
        .collect();
    v.sort_by(|a, b| {
        revision_of(a)
            .cmp(&revision_of(b))
            .then(a.meta.created_at.cmp(&b.meta.created_at))
    });
    v
}

/// Pods of `rs` that are Running and Ready, read from the pods themselves
/// so a roll neither waits on nor runs ahead of a lagging ReplicaSet
/// status. Clamped to `spec.replicas`: pods a scale-down is about to remove
/// do not count.
fn ready_pods(rs: &ReplicaSet, pods: &[Pod]) -> u32 {
    let n = live_pods(pods, &rs.meta.id)
        .into_iter()
        .filter(|p| pod_ready(p))
        .count() as u32;
    n.min(rs.spec.replicas)
}

impl DeploymentController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    fn sets(&self) -> Collection<ReplicaSet> {
        self.store.collection::<ReplicaSet>()
    }
    fn deployments(&self) -> Collection<Deployment> {
        self.store.collection::<Deployment>()
    }
    fn pods(&self) -> Collection<Pod> {
        self.store.collection::<Pod>()
    }

    fn teardown(&self, d: &Deployment, sets: &[ReplicaSet]) -> anyhow::Result<()> {
        let mut remaining = 0;
        for rs in owned_by(sets, &d.meta.id) {
            remaining += 1;
            if !delete_requested(rs) {
                request_delete(&self.sets(), &rs.meta.id)?;
            }
        }
        if remaining == 0 {
            delete_quietly(&self.deployments(), &d.meta.id)?;
            tracing::info!(deploy = %d.meta.id, name = %d.meta.name, "loom: deployment deleted");
        }
        Ok(())
    }

    fn scale_rs(&self, rs: &ReplicaSet, replicas: u32) -> Result<ReplicaSet, StoreError> {
        if rs.spec.replicas == replicas {
            return Ok(rs.clone());
        }
        tracing::info!(rs = %rs.meta.id, from = rs.spec.replicas, to = replicas, "loom: scaling replicaset");
        self.sets().modify(&rs.meta.id, |x| {
            x.spec.replicas = replicas;
            Ok(())
        })
    }

    fn create_rs(
        &self,
        d: &Deployment,
        hash: &str,
        revision: u64,
    ) -> Result<ReplicaSet, StoreError> {
        let mut template = d.spec.template.clone();
        template
            .labels
            .insert(POD_TEMPLATE_HASH.into(), hash.to_owned());
        let spec = ReplicaSetSpec {
            replicas: 0,
            selector: d.spec.selector.clone().with_label(POD_TEMPLATE_HASH, hash),
            template,
        };
        let mut rs: ReplicaSet = Resource::new(
            format!("{}-{hash}", d.meta.name),
            d.meta.project.clone(),
            spec,
        );
        rs.meta.labels = d.spec.template.labels.clone();
        rs.meta
            .labels
            .insert(POD_TEMPLATE_HASH.into(), hash.to_owned());
        rs.meta
            .labels
            .insert(OWNER_LABEL.into(), d.meta.id.to_string());
        rs.meta
            .annotations
            .insert(REVISION_ANNOTATION.into(), revision.to_string());
        rs.status = ReplicaSetStatus::default();
        let rs = self.sets().create(rs)?;
        tracing::info!(deploy = %d.meta.id, rs = %rs.meta.id, revision, "loom: replicaset created");
        Ok(rs)
    }

    fn converge(
        &self,
        d: &Deployment,
        all_sets: &[ReplicaSet],
        pods: &[Pod],
    ) -> anyhow::Result<()> {
        let mut owned = owned_sets(all_sets, d);
        let hash = template_hash(&d.spec.template);
        let max_rev = owned.iter().map(revision_of).max().unwrap_or(0);

        let new = match owned
            .iter()
            .position(|rs| rs.meta.labels.get(POD_TEMPLATE_HASH) == Some(&hash))
        {
            Some(i) => {
                let rs = owned.remove(i);
                // A rollback re-activates an old set: it becomes the newest revision.
                if revision_of(&rs) < max_rev {
                    let rev = max_rev + 1;
                    self.sets().modify(&rs.meta.id, |x| {
                        x.meta
                            .annotations
                            .insert(REVISION_ANNOTATION.into(), rev.to_string());
                        Ok(())
                    })?
                } else {
                    rs
                }
            }
            None => self.create_rs(d, &hash, max_rev + 1)?,
        };
        let old: Vec<ReplicaSet> = owned; // everything but `new`, oldest first
        let replicas = d.spec.replicas;
        let mut new = new;

        if !d.spec.paused {
            match &d.spec.strategy {
                DeploymentStrategy::Recreate => {
                    let busy = old
                        .iter()
                        .filter(|rs| rs.spec.replicas > 0 || rs.status.replicas > 0)
                        .collect::<Vec<_>>();
                    if busy.is_empty() {
                        new = self.scale_rs(&new, replicas)?;
                    } else {
                        for rs in busy {
                            self.scale_rs(rs, 0)?;
                        }
                    }
                }
                DeploymentStrategy::RollingUpdate {
                    max_surge,
                    max_unavailable,
                } => {
                    let surge = max_surge.resolve(replicas, true);
                    let mut unavail = max_unavailable.resolve(replicas, false);
                    if surge == 0 && unavail == 0 {
                        unavail = 1;
                    }
                    // Scale the new set down directly when the deployment shrank.
                    if new.spec.replicas > replicas {
                        new = self.scale_rs(&new, replicas)?;
                    }
                    let total: u32 =
                        old.iter().map(|rs| rs.spec.replicas).sum::<u32>() + new.spec.replicas;
                    let max_total = replicas + surge;
                    if new.spec.replicas < replicas && total < max_total {
                        let target = (new.spec.replicas + (max_total - total)).min(replicas);
                        new = self.scale_rs(&new, target)?;
                    }
                    // Scale old sets down: unready pods are free to go, ready
                    // ones only while staying above min_available. Ready
                    // means Running and Ready, so a new pod held back by
                    // its readiness probe frees no old pod.
                    let min_available = replicas.saturating_sub(unavail);
                    let total_ready: u32 = old.iter().map(|rs| ready_pods(rs, pods)).sum::<u32>()
                        + ready_pods(&new, pods);
                    let mut can_down = total_ready.saturating_sub(min_available);
                    for rs in &old {
                        let ready = ready_pods(rs, pods);
                        let free = rs.spec.replicas - ready;
                        let ready_down = ready.min(can_down);
                        can_down -= ready_down;
                        let take = (free + ready_down).min(rs.spec.replicas);
                        if take > 0 {
                            self.scale_rs(rs, rs.spec.replicas - take)?;
                        }
                    }
                }
            }
        }

        // History: drop fully-scaled-down old sets beyond the limit.
        let mut idle: Vec<&ReplicaSet> = old
            .iter()
            .filter(|rs| rs.spec.replicas == 0 && rs.status.replicas == 0)
            .collect();
        idle.sort_by_key(|rs| std::cmp::Reverse(revision_of(rs)));
        for rs in idle.iter().skip(d.spec.revision_history_limit as usize) {
            request_delete(&self.sets(), &rs.meta.id)?;
            tracing::info!(deploy = %d.meta.id, rs = %rs.meta.id, "loom: pruning old replicaset");
        }

        // Status from the sets as last observed.
        let sum_status = |f: fn(&ReplicaSetStatus) -> u32| -> u32 {
            old.iter().map(|rs| f(&rs.status)).sum::<u32>() + f(&new.status)
        };
        let total_replicas = sum_status(|s| s.replicas);
        let ready = sum_status(|s| s.ready);
        let available = sum_status(|s| s.available);
        let updated = new.status.replicas;
        let revision = revision_of(&new);
        let min_available = match &d.spec.strategy {
            DeploymentStrategy::RollingUpdate {
                max_unavailable, ..
            } => replicas.saturating_sub(max_unavailable.resolve(replicas, false)),
            DeploymentStrategy::Recreate => 0,
        };
        let complete = updated == replicas && total_replicas == replicas && available == replicas;
        patch_status(&self.deployments(), &d.meta.id, |s| {
            s.replicas = total_replicas;
            s.updated = updated;
            s.ready = ready;
            s.available = available;
            s.unavailable = replicas.saturating_sub(available);
            s.revision = revision;
            set_condition(
                &mut s.conditions,
                "Available",
                available >= min_available,
                if available >= min_available {
                    "MinimumReplicasAvailable"
                } else {
                    "MinimumReplicasUnavailable"
                },
                &format!("{available}/{replicas} available"),
            );
            set_condition(
                &mut s.conditions,
                "Progressing",
                true,
                if d.spec.paused {
                    "DeploymentPaused"
                } else if complete {
                    "NewReplicaSetAvailable"
                } else {
                    "ReplicaSetUpdated"
                },
                &format!("revision {revision}: {updated}/{replicas} updated"),
            );
        })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Controller for DeploymentController {
    fn name(&self) -> &'static str {
        "deployment"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[
            crate::types::kinds::POD,
            crate::types::kinds::REPLICA_SET,
            crate::types::kinds::DEPLOYMENT,
        ]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let deployments = self.deployments().list(None)?;
        let sets = self.sets().list(None)?;
        let pods = self.pods().list(None)?;
        for d in &deployments {
            let res = if delete_requested(d) {
                self.teardown(d, &sets)
            } else {
                self.converge(d, &sets, &pods)
            };
            if let Err(e) = res {
                tracing::warn!(deploy = %d.meta.id, error = %e, "loom: deployment reconcile failed");
            }
        }
        Ok(())
    }
}
