// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The DaemonSet controller: one pod per eligible host.
//!
//! Eligible hosts are forge hosts in `Ready` that advertise a container
//! driver (`youki` or `mock`) and whose labels satisfy the pod template's
//! `node_selector`. The controller stamps `pistack.io/hostname` on each
//! host so a pod can pin to that host through `node_selector`, and records
//! the host id on the pod as `loom.pistack.io/target-host`.
//!
//! A host that joins gets a pod on the next pass; a host that leaves (or
//! is no longer eligible) has its pod deleted. A template change recreates
//! mismatched pods the same way Deployment's `Recreate` path does.

use super::{delete_quietly, owned_by, patch_status, request_delete, Controller};
use crate::selector::template_hash;
use crate::types::{
    delete_requested, set_condition, DaemonSet, Pod, PodPhase, PodStatus, PodTemplate,
    HOSTNAME_LABEL, OWNER_LABEL, POD_TEMPLATE_HASH, TARGET_HOST_ANNOTATION,
};
use forge::{Host, MachineKind};
use pistack_store::{Collection, Store};
use pistack_types::{kind, Phase, Resource};
use std::collections::BTreeSet;

pub struct DaemonSetController {
    store: Store,
}

impl DaemonSetController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    fn sets(&self) -> Collection<DaemonSet> {
        self.store.collection::<DaemonSet>()
    }
    fn pods(&self) -> Collection<Pod> {
        self.store.collection::<Pod>()
    }
    fn hosts(&self) -> Collection<Host> {
        self.store.collection::<Host>()
    }

    fn teardown(&self, ds: &DaemonSet, pods: &[Pod]) -> anyhow::Result<()> {
        let mut remaining = 0;
        for p in owned_by(pods, &ds.meta.id) {
            remaining += 1;
            if !delete_requested(p) {
                request_delete(&self.pods(), &p.meta.id)?;
            }
        }
        if remaining == 0 {
            delete_quietly(&self.sets(), &ds.meta.id)?;
            tracing::info!(ds = %ds.meta.id, name = %ds.meta.name, "loom: daemonset deleted");
        }
        Ok(())
    }

    fn converge(&self, ds: &DaemonSet, pods: &[Pod], hosts: &[Host]) -> anyhow::Result<()> {
        let hash = template_hash(&ds.spec.template);

        // Stamp hostname labels so node_selector can pin, then re-list.
        for h in hosts {
            if h.status.phase == Phase::Ready && !h.spec.node_name.is_empty() {
                self.ensure_hostname_label(h)?;
            }
        }
        let hosts = self.hosts().list(None)?;
        let eligible = eligible_hosts(&hosts, &ds.spec.template)?;
        let eligible_ids: BTreeSet<String> =
            eligible.iter().map(|h| h.meta.id.to_string()).collect();

        let owned: Vec<&Pod> = owned_by(pods, &ds.meta.id)
            .filter(|p| !delete_requested(p))
            .collect();

        // Delete pods for departed/ineligible hosts and pods with a stale template.
        let mut deleting: BTreeSet<String> = BTreeSet::new();
        for p in &owned {
            let target = p.meta.annotations.get(TARGET_HOST_ANNOTATION).cloned();
            let gone = target
                .as_ref()
                .map(|id| !eligible_ids.contains(id))
                .unwrap_or(true);
            let stale = p.meta.labels.get(POD_TEMPLATE_HASH) != Some(&hash);
            if gone || stale {
                request_delete(&self.pods(), &p.meta.id)?;
                if let Some(id) = target {
                    deleting.insert(id);
                }
                tracing::info!(
                    ds = %ds.meta.id,
                    pod = %p.meta.id,
                    gone,
                    stale,
                    "loom: daemonset pod delete requested"
                );
            }
        }

        // Create one pod per eligible host that has none (and is not mid-delete).
        for h in &eligible {
            let hid = h.meta.id.to_string();
            if deleting.contains(&hid) {
                continue;
            }
            let has = owned.iter().any(|p| {
                p.meta.annotations.get(TARGET_HOST_ANNOTATION) == Some(&hid)
                    && p.meta.labels.get(POD_TEMPLATE_HASH) == Some(&hash)
            });
            if has {
                continue;
            }
            let pod = self.build_pod(ds, h, &hash)?;
            let pod = self.pods().create(pod)?;
            tracing::info!(
                ds = %ds.meta.id,
                pod = %pod.meta.id,
                name = %pod.meta.name,
                host = %h.meta.id,
                "loom: daemonset pod created"
            );
        }

        let pods = self.pods().list(None)?;
        let live: Vec<&Pod> = owned_by(&pods, &ds.meta.id)
            .filter(|p| !delete_requested(p))
            .collect();
        let desired = eligible.len() as u32;
        let current = live.len() as u32;
        let ready = live
            .iter()
            .filter(|p| p.status.phase == PodPhase::Running)
            .count() as u32;
        patch_status(&self.sets(), &ds.meta.id, |s| {
            s.desired = desired;
            s.current = current;
            s.ready = ready;
            let ok = current == desired && ready == desired;
            set_condition(
                &mut s.conditions,
                "Ready",
                ok,
                if ok { "AllPodsReady" } else { "Progressing" },
                &format!("{ready}/{desired} ready"),
            );
        })?;
        Ok(())
    }

    fn ensure_hostname_label(&self, host: &Host) -> anyhow::Result<()> {
        let want = host.spec.node_name.clone();
        if want.is_empty() {
            return Ok(());
        }
        if host.meta.labels.get(HOSTNAME_LABEL) == Some(&want) {
            return Ok(());
        }
        self.hosts().modify(&host.meta.id, |h| {
            h.meta.labels.insert(HOSTNAME_LABEL.into(), want.clone());
            Ok(())
        })?;
        Ok(())
    }

    fn build_pod(&self, ds: &DaemonSet, host: &Host, hash: &str) -> anyhow::Result<Pod> {
        let suffix = sanitize_node(&host.spec.node_name);
        let name = truncate_name(&format!("{}-{suffix}", ds.meta.name), 63);
        let mut template: PodTemplate = ds.spec.template.clone();
        template
            .labels
            .insert(POD_TEMPLATE_HASH.into(), hash.to_owned());
        template
            .labels
            .insert(OWNER_LABEL.into(), ds.meta.id.to_string());

        // Pin to this host: template node_selector plus hostname label.
        template
            .spec
            .node_selector
            .insert(HOSTNAME_LABEL.into(), host.spec.node_name.clone());

        let mut pod: Pod = Resource::new(name, ds.meta.project.clone(), template.spec);
        pod.meta.labels = template.labels;
        pod.meta.annotations = template.annotations;
        pod.meta
            .annotations
            .insert(TARGET_HOST_ANNOTATION.into(), host.meta.id.to_string());
        pod.status = PodStatus::default();
        Ok(pod)
    }
}

/// Hosts that can run a DaemonSet pod for `template`.
fn eligible_hosts<'a>(hosts: &'a [Host], template: &PodTemplate) -> anyhow::Result<Vec<&'a Host>> {
    let accepted = MachineKind::Container.accepted_drivers();
    let mut out = Vec::new();
    for h in hosts {
        if h.status.phase != Phase::Ready {
            continue;
        }
        if h.spec.node_name.is_empty() {
            continue;
        }
        if !h
            .spec
            .drivers
            .iter()
            .any(|d| accepted.contains(&d.as_str()))
        {
            continue;
        }
        // Match the user-supplied node_selector against host labels,
        // treating a missing hostname label as equal to node_name.
        let mut labels = h.meta.labels.clone();
        labels
            .entry(HOSTNAME_LABEL.into())
            .or_insert_with(|| h.spec.node_name.clone());
        if !template
            .spec
            .node_selector
            .iter()
            .all(|(k, v)| labels.get(k) == Some(v))
        {
            continue;
        }
        out.push(h);
    }
    out.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    Ok(out)
}

fn sanitize_node(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() {
        "node".into()
    } else {
        s
    }
}

fn truncate_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    name.chars()
        .take(max)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

#[async_trait::async_trait]
impl Controller for DaemonSetController {
    fn name(&self) -> &'static str {
        "daemonset"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[
            crate::types::kinds::POD,
            crate::types::kinds::DAEMON_SET,
            kind::HOST,
        ]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let sets = self.sets().list(None)?;
        let pods = self.pods().list(None)?;
        let hosts = self.hosts().list(None)?;
        for ds in &sets {
            let res = if delete_requested(ds) {
                self.teardown(ds, &pods)
            } else {
                self.converge(ds, &pods, &hosts)
            };
            if let Err(e) = res {
                pistack_types::tally::reconcile_failed("loom");
                tracing::warn!(ds = %ds.meta.id, error = %e, "loom: daemonset reconcile failed");
            }
        }
        Ok(())
    }
}
