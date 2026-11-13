// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The StatefulSet controller: ordered, stable pod identities.
//!
//! Pods are named `<sts>-<ordinal>` and owned through `OWNER_LABEL`. Scale
//! up creates the next ordinal; scale down deletes the highest first.
//! `OrderedReady` waits for a pod to be Running and Ready
//! ([`super::pod_ready`], i.e. `PodStatus::is_ready`) before creating the
//! next ordinal; `Parallel` does every ordinal in one pass. A template
//! change under `RollingUpdate` replaces mismatched pods highest-first
//! (same delete/recreate path Deployment uses for `Recreate`); under
//! `OrderedReady` the next pod is replaced only once the one before it is
//! back and Ready. A pod with no readiness probe is ready once its
//! containers run.
//!
//! `volume_claim_templates` create one `slab` volume per ordinal
//! (`<sts>-<claim>-<ordinal>`) and attach it as `VolumeSource::Volume`.

use super::{delete_quietly, owned_by, patch_status, pod_ready, request_delete, Controller};
use crate::selector::template_hash;
use crate::types::{
    delete_requested, set_condition, Pod, PodManagementPolicy, PodStatus, PodTemplate, PodVolume,
    StatefulSet, StatefulSetUpdateStrategy, VolumeClaimTemplate, VolumeSource, OWNER_LABEL,
    POD_NAME_LABEL, POD_ORDINAL_LABEL, POD_TEMPLATE_HASH,
};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{kind, Id, Phase, Resource, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Wire shape of a slab volume, written directly so loom does not take a
/// slab dependency. Same kind and fields as `slab::Volume`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
struct BlockVolumeSpec {
    size_gib: u64,
    #[serde(default = "standard_type")]
    volume_type: String,
    #[serde(default)]
    source: BlockVolumeSource,
    #[serde(default)]
    bootable: bool,
    #[serde(default)]
    encrypted: bool,
}

fn standard_type() -> String {
    "standard".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
enum BlockVolumeSource {
    #[default]
    None,
}

impl Spec for BlockVolumeSpec {
    const KIND: &'static str = kind::VOLUME;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
struct BlockVolumeStatus {
    #[serde(default)]
    phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attached_to: Option<Id>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    message: String,
}

type BlockVolume = Resource<BlockVolumeSpec, BlockVolumeStatus>;

pub struct StatefulSetController {
    store: Store,
}

impl StatefulSetController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    fn sets(&self) -> Collection<StatefulSet> {
        self.store.collection::<StatefulSet>()
    }
    fn pods(&self) -> Collection<Pod> {
        self.store.collection::<Pod>()
    }
    fn volumes(&self) -> Collection<BlockVolume> {
        self.store.collection::<BlockVolume>()
    }

    fn teardown(
        &self,
        sts: &StatefulSet,
        pods: &[Pod],
        vols: &[BlockVolume],
    ) -> anyhow::Result<()> {
        let mut remaining = 0u32;
        let mut owned: Vec<&Pod> = owned_by(pods, &sts.meta.id).collect();
        owned.sort_by_key(|p| std::cmp::Reverse(ordinal_of(p).unwrap_or(0)));
        for p in owned {
            remaining += 1;
            if !delete_requested(p) {
                request_delete(&self.pods(), &p.meta.id)?;
            }
        }
        if remaining > 0 {
            return Ok(());
        }
        for v in owned_by(vols, &sts.meta.id) {
            delete_quietly(&self.volumes(), &v.meta.id)?;
        }
        delete_quietly(&self.sets(), &sts.meta.id)?;
        tracing::info!(sts = %sts.meta.id, name = %sts.meta.name, "loom: statefulset deleted");
        Ok(())
    }

    fn converge(
        &self,
        sts: &StatefulSet,
        pods: &[Pod],
        vols: &[BlockVolume],
    ) -> anyhow::Result<()> {
        let hash = template_hash(&sts.spec.template);
        let ordered = sts.spec.pod_management_policy == PodManagementPolicy::OrderedReady;
        let want = sts.spec.replicas;

        let mut by_ordinal: BTreeMap<u32, &Pod> = BTreeMap::new();
        for p in owned_by(pods, &sts.meta.id).filter(|p| !delete_requested(p)) {
            if let Some(o) = ordinal_of(p) {
                by_ordinal.insert(o, p);
            }
        }

        // Scale down: highest ordinal first.
        let surplus: Vec<u32> = by_ordinal
            .keys()
            .copied()
            .filter(|o| *o >= want)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        for o in surplus {
            if let Some(p) = by_ordinal.get(&o) {
                request_delete(&self.pods(), &p.meta.id)?;
                tracing::info!(sts = %sts.meta.id, pod = %p.meta.id, ordinal = o, "loom: statefulset scale down");
                if ordered {
                    return self.write_status(sts, &by_ordinal, &hash);
                }
            }
        }
        // Drop surplus from the map for the rest of this pass.
        by_ordinal.retain(|o, _| *o < want);

        // Rolling update: replace mismatched pods, highest first.
        if sts.spec.update_strategy == StatefulSetUpdateStrategy::RollingUpdate {
            let stale = |o: &u32| {
                by_ordinal
                    .get(o)
                    .is_some_and(|p| p.meta.labels.get(POD_TEMPLATE_HASH) != Some(&hash))
            };
            let replace: Vec<u32> = if ordered {
                // One pod down at a time: wait at the first ordinal that is
                // missing or not Ready (the scale-up below creates it or
                // waits for it), unless it is an old pod, which serves
                // nothing and is replaced first so a broken template can be
                // rolled forward.
                match (0..want).find(|o| by_ordinal.get(o).is_none_or(|p| !pod_ready(p))) {
                    None => by_ordinal
                        .keys()
                        .rev()
                        .copied()
                        .find(stale)
                        .into_iter()
                        .collect(),
                    Some(o) if stale(&o) => vec![o],
                    Some(_) => Vec::new(),
                }
            } else {
                by_ordinal.keys().rev().copied().filter(stale).collect()
            };
            for o in replace {
                if let Some(p) = by_ordinal.get(&o) {
                    request_delete(&self.pods(), &p.meta.id)?;
                    tracing::info!(sts = %sts.meta.id, pod = %p.meta.id, ordinal = o, "loom: statefulset rolling update");
                    if ordered {
                        return self.write_status(sts, &by_ordinal, &hash);
                    }
                }
            }
        }

        // Scale up / ensure each ordinal.
        for o in 0..want {
            match by_ordinal.get(&o) {
                Some(p) => {
                    if ordered && !pod_ready(p) {
                        break;
                    }
                }
                None => {
                    self.ensure_claims(sts, o, vols)?;
                    let pod = self.build_pod(sts, o, &hash)?;
                    let pod = self.pods().create(pod)?;
                    tracing::info!(sts = %sts.meta.id, pod = %pod.meta.id, name = %pod.meta.name, ordinal = o, "loom: statefulset pod created");
                    if ordered {
                        break;
                    }
                }
            }
        }

        // Refresh owned map for status from the store.
        let pods = self.pods().list(None)?;
        let mut live: BTreeMap<u32, &Pod> = BTreeMap::new();
        for p in owned_by(&pods, &sts.meta.id).filter(|p| !delete_requested(p)) {
            if let Some(o) = ordinal_of(p) {
                if o < want {
                    live.insert(o, p);
                }
            }
        }
        self.write_status(sts, &live, &hash)
    }

    fn write_status(
        &self,
        sts: &StatefulSet,
        by_ordinal: &BTreeMap<u32, &Pod>,
        hash: &str,
    ) -> anyhow::Result<()> {
        let replicas = by_ordinal.len() as u32;
        let ready = by_ordinal.values().filter(|p| pod_ready(p)).count() as u32;
        let current = by_ordinal
            .values()
            .filter(|p| p.meta.labels.get(POD_TEMPLATE_HASH).map(|v| v.as_str()) == Some(hash))
            .count() as u32;
        let target = sts.spec.replicas;
        patch_status(&self.sets(), &sts.meta.id, |s| {
            s.replicas = replicas;
            s.ready = ready;
            s.current = current;
            s.updated = current;
            let ok = replicas == target && ready == target && current == target;
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

    fn ensure_claims(
        &self,
        sts: &StatefulSet,
        ordinal: u32,
        _vols: &[BlockVolume],
    ) -> anyhow::Result<()> {
        for claim in &sts.spec.volume_claim_templates {
            let name = volume_name(&sts.meta.name, &claim.name, ordinal);
            if self
                .volumes()
                .get_by_name(sts.meta.project.as_ref(), &name)
                .is_ok()
            {
                continue;
            }
            self.create_volume(sts, claim, ordinal)?;
        }
        Ok(())
    }

    fn create_volume(
        &self,
        sts: &StatefulSet,
        claim: &VolumeClaimTemplate,
        ordinal: u32,
    ) -> Result<BlockVolume, StoreError> {
        let name = volume_name(&sts.meta.name, &claim.name, ordinal);
        let spec = BlockVolumeSpec {
            size_gib: claim.size_gib.max(1),
            volume_type: if claim.volume_type.is_empty() {
                standard_type()
            } else {
                claim.volume_type.clone()
            },
            ..Default::default()
        };
        let mut vol: BlockVolume = Resource::new(name, sts.meta.project.clone(), spec);
        vol.meta
            .labels
            .insert(OWNER_LABEL.into(), sts.meta.id.to_string());
        vol.meta
            .labels
            .insert(POD_ORDINAL_LABEL.into(), ordinal.to_string());
        vol.meta
            .labels
            .insert("loom.pistack.io/claim".into(), claim.name.clone());
        vol.status = BlockVolumeStatus {
            phase: Phase::Pending,
            ..Default::default()
        };
        match self.volumes().create(vol) {
            Ok(v) => {
                tracing::info!(sts = %sts.meta.id, volume = %v.meta.id, name = %v.meta.name, "loom: statefulset volume created");
                Ok(v)
            }
            Err(StoreError::Conflict(_)) => self.volumes().get_by_name(
                sts.meta.project.as_ref(),
                &volume_name(&sts.meta.name, &claim.name, ordinal),
            ),
            Err(e) => Err(e),
        }
    }

    fn build_pod(&self, sts: &StatefulSet, ordinal: u32, hash: &str) -> anyhow::Result<Pod> {
        let name = format!("{}-{ordinal}", sts.meta.name);
        let mut template: PodTemplate = sts.spec.template.clone();
        template
            .labels
            .insert(POD_TEMPLATE_HASH.into(), hash.to_owned());
        template.labels.insert(POD_NAME_LABEL.into(), name.clone());
        template
            .labels
            .insert(POD_ORDINAL_LABEL.into(), ordinal.to_string());
        template
            .labels
            .insert(OWNER_LABEL.into(), sts.meta.id.to_string());

        // Attach claim volumes by resolving each claim's volume id.
        for claim in &sts.spec.volume_claim_templates {
            let vname = volume_name(&sts.meta.name, &claim.name, ordinal);
            let vol = self
                .volumes()
                .get_by_name(sts.meta.project.as_ref(), &vname)?;
            if template.spec.volumes.iter().any(|v| v.name == claim.name) {
                // Replace EmptyDir/placeholder with the concrete volume id.
                for v in &mut template.spec.volumes {
                    if v.name == claim.name {
                        v.source = VolumeSource::Volume(vol.meta.id.clone());
                    }
                }
            } else {
                template.spec.volumes.push(PodVolume {
                    name: claim.name.clone(),
                    source: VolumeSource::Volume(vol.meta.id.clone()),
                });
            }
        }

        let mut pod: Pod = Resource::new(name.clone(), sts.meta.project.clone(), template.spec);
        pod.meta.labels = template.labels;
        pod.meta.annotations = template.annotations;
        if !sts.spec.service_name.is_empty() {
            pod.meta.annotations.insert(
                "loom.pistack.io/service-name".into(),
                sts.spec.service_name.clone(),
            );
        }
        pod.status = PodStatus::default();
        Ok(pod)
    }
}

fn volume_name(sts: &str, claim: &str, ordinal: u32) -> String {
    format!("{sts}-{claim}-{ordinal}")
}

fn ordinal_of(pod: &Pod) -> Option<u32> {
    pod.meta
        .labels
        .get(POD_ORDINAL_LABEL)
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            pod.meta
                .name
                .rsplit_once('-')
                .and_then(|(_, n)| n.parse().ok())
        })
}

#[async_trait::async_trait]
impl Controller for StatefulSetController {
    fn name(&self) -> &'static str {
        "statefulset"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[
            crate::types::kinds::POD,
            crate::types::kinds::STATEFUL_SET,
            kind::VOLUME,
        ]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let sets = self.sets().list(None)?;
        let pods = self.pods().list(None)?;
        let vols = self.volumes().list(None)?;
        for sts in &sets {
            let res = if delete_requested(sts) {
                self.teardown(sts, &pods, &vols)
            } else {
                self.converge(sts, &pods, &vols)
            };
            if let Err(e) = res {
                pistack_types::tally::reconcile_failed("loom");
                tracing::warn!(sts = %sts.meta.id, error = %e, "loom: statefulset reconcile failed");
            }
        }
        Ok(())
    }
}
