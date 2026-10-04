// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The controllers: one reconcile loop per resource kind.
//!
//! Every controller implements [`Controller`]: it names the store kinds
//! whose events wake it and provides one idempotent `reconcile_once` pass.
//! [`run`] drives it until shutdown, waking on those events and on a
//! [`TICK`] so nothing depends on an event being seen.
//!
//! Rules shared by every controller:
//! * `status` is written only through [`patch_status`], which re-reads the
//!   resource, applies the change and writes with the observed revision, so
//!   a concurrent handler write is never clobbered.
//! * Deletes are requested through `DELETE_ANNOTATION` ([`request_delete`])
//!   and carried out by the owning controller once children are gone.
//! * Ownership is a label (`OWNER_LABEL` = owner id), never a scan by name.

pub mod cronjob;
pub mod deployment;
pub mod ingress;
pub mod job;
pub mod pod;
pub mod replicaset;
pub mod service;

use crate::types::{delete_requested, PodTemplate, DELETE_ANNOTATION, OWNER_LABEL};
use crate::types::{Pod, PodStatus};
use async_trait::async_trait;
use chrono::Utc;
use pistack_store::{Collection, StoreError};
use pistack_types::{Id, Kind, ModuleContext, Resource, Spec};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// How often a pass runs even when nothing changed.
pub const TICK: Duration = Duration::from_secs(2);

#[async_trait]
pub trait Controller: Send + Sync + 'static {
    /// Short name for logs.
    fn name(&self) -> &'static str;
    /// Store kinds whose events wake this controller.
    fn watches(&self) -> &'static [&'static str];
    /// One idempotent pass over everything this controller owns.
    async fn reconcile_once(&self) -> anyhow::Result<()>;
}

/// Drive `c` until `ctx.shutdown` fires.
pub async fn run(c: impl Controller, ctx: ModuleContext) {
    let mut rx = ctx.events.subscribe();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let name = c.name();
    loop {
        tokio::select! {
            _ = ctx.shutdown.cancelled() => break,
            _ = tick.tick() => {}
            ev = rx.recv() => match ev {
                Ok(e) if c.watches().contains(&e.kind.as_str()) => {}
                Ok(_) => continue,
                Err(RecvError::Lagged(n)) => tracing::debug!(controller = name, skipped = n, "loom: event bus lagged"),
                Err(RecvError::Closed) => break,
            },
        }
        if let Err(e) = c.reconcile_once().await {
            tracing::warn!(controller = name, error = %e, "loom: reconcile pass failed");
        }
    }
    tracing::info!(controller = name, "loom: controller stopped");
}

/// Status types every loom resource uses.
pub trait Status:
    Serialize + DeserializeOwned + Clone + PartialEq + Default + Send + Sync + 'static
{
}
impl<T> Status for T where
    T: Serialize + DeserializeOwned + Clone + PartialEq + Default + Send + Sync + 'static
{
}

/// Apply `f` to the resource's status and write it back only if it changed,
/// guarded by the revision observed. Returns the stored resource.
pub fn patch_status<S: Spec, T: Status>(
    col: &Collection<Resource<S, T>>,
    id: &Id,
    f: impl Fn(&mut T),
) -> Result<Resource<S, T>, StoreError> {
    for _ in 0..8 {
        let cur = col.get(id)?;
        let mut next = cur.clone();
        f(&mut next.status);
        if next.status == cur.status {
            return Ok(cur);
        }
        match col.update(next, Some(cur.meta.revision)) {
            Ok(r) => return Ok(r),
            Err(StoreError::Revision { .. }) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(StoreError::Conflict(format!(
        "{} {id}: too many concurrent writers",
        S::KIND
    )))
}

/// Set the delete annotation if it is not there yet.
pub fn request_delete<K: Kind>(col: &Collection<K>, id: &Id) -> Result<(), StoreError> {
    match col.modify(id, |r| {
        r.meta_mut()
            .annotations
            .entry(DELETE_ANNOTATION.into())
            .or_insert_with(|| Utc::now().to_rfc3339());
        Ok(())
    }) {
        Ok(_) | Err(StoreError::NotFound(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Delete from the store, tolerating "already gone".
pub fn delete_quietly<K: Kind>(col: &Collection<K>, id: &Id) -> Result<(), StoreError> {
    match col.delete(id) {
        Ok(_) | Err(StoreError::NotFound(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Resources whose `OWNER_LABEL` is `owner`.
pub fn owned_by<'a, S, T>(
    items: &'a [Resource<S, T>],
    owner: &Id,
) -> impl Iterator<Item = &'a Resource<S, T>> + 'a {
    let owner = owner.to_string();
    items
        .iter()
        .filter(move |r| r.meta.labels.get(OWNER_LABEL) == Some(&owner))
}

/// Build a pod from a template with `owner` recorded. Name is
/// `<prefix>-<5 random>`.
pub fn pod_from_template(prefix: &str, project: Option<Id>, t: &PodTemplate, owner: &Id) -> Pod {
    let name = format!("{prefix}-{}", crate::selector::random_suffix());
    let mut pod: Pod = Resource::new(name, project, t.spec.clone());
    pod.meta.labels = t.labels.clone();
    pod.meta
        .labels
        .insert(OWNER_LABEL.into(), owner.to_string());
    pod.meta.annotations = t.annotations.clone();
    pod.status = PodStatus::default();
    pod
}

/// Pods that count towards a controller's replica count: owned, not
/// being deleted, not terminal.
pub fn live_pods<'a>(pods: &'a [Pod], owner: &Id) -> Vec<&'a Pod> {
    owned_by(pods, owner)
        .filter(|p| !delete_requested(p) && !p.status.phase.is_terminal())
        .collect()
}
