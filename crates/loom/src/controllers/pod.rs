// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The pod controller: a pod becomes a forge `Machine` of kind `Container`.
//!
//! ```text
//! Pod Pending ──create──▶ Machine{kind: Container}  ──forge──▶ Ready
//!     ▲                                                          │
//!     └── mirror: Ready/On → Running (pod_ip, host); Error → Failed ┘
//! ```
//!
//! # The machine → anvil contract
//!
//! The machine's `spec.user_data` is the JSON of a [`PodSandbox`]: the pod's
//! identity plus its full container spec. `anvil`'s youki runtime
//! deserialises it to build the OCI bundle(s): one sandbox (network
//! namespace, the machine's `addresses`) and one container per entry in
//! `containers`, in order, sharing the sandbox namespaces. `spec.image` is
//! the first container's image for display and image pre-pull only; the
//! runtime takes each container's image from the sandbox document.
//!
//! What the runtime reports back through the ordinary `Machine.status`:
//! * `phase = Ready, power = On` while the containers run;
//! * when a container's main process exits, `power = Off` plus a condition
//!   `ContainerExited{status: true, reason: "<exit code>", message: "<name>"}`
//!   (`reason` is the decimal exit code of the first container to exit);
//! * `phase = Error` when the sandbox or a container cannot be started.
//!
//! The controller maps that to the pod: `Running`, `Succeeded` (exit 0),
//! `Failed`, and restarts per `restart_policy` by bumping the machine's
//! `spec.restarts` (the same knob `POST /machines/{id}/reboot` uses) with an
//! exponential backoff capped at five minutes.

use super::{delete_quietly, patch_status, request_delete, Controller};
use crate::types::{
    set_condition, ContainerState, ContainerStatus, Pod, PodPhase, PodStatus, RestartPolicy,
    MACHINE_POD_LABEL,
};
use chrono::{Duration, Utc};
use forge::{Machine, MachineKind, MachineSpec, PowerState, Size};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{Id, Phase, Resources};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The document a pod's machine carries in `spec.user_data`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodSandbox {
    /// Always `"loom.pistack.io/v1"`.
    pub api_version: String,
    /// Always `"PodSandbox"`.
    pub kind: String,
    pub pod: PodIdentity,
    pub restart_policy: RestartPolicy,
    pub containers: Vec<crate::types::Container>,
    pub volumes: Vec<crate::types::PodVolume>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service_account: String,
    pub networks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodIdentity {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<Id>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

pub const SANDBOX_API_VERSION: &str = "loom.pistack.io/v1";
pub const SANDBOX_KIND: &str = "PodSandbox";
/// Machine condition anvil sets when a container exits; `reason` is the
/// decimal exit code.
pub const EXITED_CONDITION: &str = "ContainerExited";
/// Network attached when the pod names none.
pub const DEFAULT_NETWORK: &str = "default";

impl PodSandbox {
    pub fn from_pod(pod: &Pod) -> Self {
        Self {
            api_version: SANDBOX_API_VERSION.into(),
            kind: SANDBOX_KIND.into(),
            pod: PodIdentity {
                id: pod.meta.id.clone(),
                name: pod.meta.name.clone(),
                project: pod.meta.project.clone(),
                labels: pod.meta.labels.clone(),
                annotations: pod.meta.annotations.clone(),
            },
            restart_policy: pod.spec.restart_policy,
            containers: pod.spec.containers.clone(),
            volumes: pod.spec.volumes.clone(),
            service_account: pod.spec.service_account.clone(),
            networks: networks_of(pod),
        }
    }
}

fn networks_of(pod: &Pod) -> Vec<String> {
    if pod.spec.networks.is_empty() {
        vec![DEFAULT_NETWORK.into()]
    } else {
        pod.spec.networks.clone()
    }
}

/// Name of the auto-created size for a pod that fits no existing one.
pub fn exact_size_name(r: &Resources) -> String {
    format!("pod-{}x{}", r.vcpus.max(1), r.memory_mib.max(1))
}

pub struct PodController {
    store: Store,
}

impl PodController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    fn pods(&self) -> Collection<Pod> {
        self.store.collection::<Pod>()
    }
    fn machines(&self) -> Collection<Machine> {
        self.store.collection::<Machine>()
    }
    fn sizes(&self) -> Collection<Size> {
        self.store.collection::<Size>()
    }

    /// Smallest existing size the pod fits, else create `pod-<v>x<m>`.
    fn size_for(&self, want: &Resources) -> Result<String, StoreError> {
        let mut sizes = self.sizes().list(None)?;
        sizes.sort_by_key(|s| {
            let r = s.spec.resources;
            (r.memory_mib, r.vcpus, r.disk_gib, r.gpus)
        });
        if let Some(s) = sizes.iter().find(|s| want.fits_in(&s.spec.resources)) {
            return Ok(s.meta.name.clone());
        }
        let name = exact_size_name(want);
        let exact = Resources {
            vcpus: want.vcpus.max(1),
            memory_mib: want.memory_mib.max(1),
            disk_gib: want.disk_gib,
            gpus: want.gpus,
        };
        match self.sizes().create(forge::new_size(name.clone(), exact)) {
            Ok(_) | Err(StoreError::Conflict(_)) => Ok(name),
            Err(e) => Err(e),
        }
    }

    fn machine_of<'a>(&self, pod: &Pod, machines: &'a [Machine]) -> Option<&'a Machine> {
        let pid = pod.meta.id.to_string();
        pod.status
            .machine
            .as_ref()
            .and_then(|mid| machines.iter().find(|m| &m.meta.id == mid))
            .or_else(|| {
                machines
                    .iter()
                    .find(|m| m.meta.labels.get(MACHINE_POD_LABEL) == Some(&pid))
            })
    }

    fn create_machine(&self, pod: &Pod) -> Result<Machine, StoreError> {
        let want = pod.spec.resources();
        let size = self.size_for(&want)?;
        let image = pod
            .spec
            .containers
            .first()
            .map(|c| c.image.clone())
            .unwrap_or_default();
        let sandbox = PodSandbox::from_pod(pod);
        let user_data = serde_json::to_string(&sandbox)?;
        let volumes = pod
            .spec
            .volumes
            .iter()
            .filter_map(|v| match &v.source {
                crate::types::VolumeSource::Volume(id) => Some(id.to_string()),
                _ => None,
            })
            .collect();
        let spec = MachineSpec {
            kind: MachineKind::Container,
            size,
            image,
            networks: networks_of(pod),
            volumes,
            user_data: Some(user_data),
            host_selector: pod.spec.node_selector.clone(),
            ..Default::default()
        };
        let mut m = forge::new_machine(
            format!("pod-{}", pod.meta.name),
            pod.meta.project.clone(),
            spec,
        );
        m.meta.labels = pod.meta.labels.clone();
        m.meta
            .labels
            .insert(MACHINE_POD_LABEL.into(), pod.meta.id.to_string());
        m.meta.description = format!("pod {}", pod.meta.name);
        match self.machines().create(m) {
            Ok(m) => Ok(m),
            Err(StoreError::Conflict(_)) => {
                // A previous pass created it but the status write was lost.
                let existing = self
                    .machines()
                    .get_by_name(pod.meta.project.as_ref(), &format!("pod-{}", pod.meta.name))?;
                if existing.meta.labels.get(MACHINE_POD_LABEL) == Some(&pod.meta.id.to_string()) {
                    Ok(existing)
                } else {
                    Err(StoreError::Conflict(format!(
                        "machine 'pod-{}' exists and belongs to another pod",
                        pod.meta.name
                    )))
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn teardown(&self, pod: &Pod, machines: &[Machine]) -> anyhow::Result<()> {
        if let Some(m) = self.machine_of(pod, machines) {
            request_delete(&self.machines(), &m.meta.id)?;
            patch_status(&self.pods(), &pod.meta.id, |s| {
                s.message = "deleting machine".into();
            })?;
            return Ok(());
        }
        delete_quietly(&self.pods(), &pod.meta.id)?;
        tracing::info!(pod = %pod.meta.id, name = %pod.meta.name, "loom: pod deleted");
        Ok(())
    }

    async fn converge(&self, pod: &Pod, machines: &[Machine]) -> anyhow::Result<()> {
        let Some(m) = self.machine_of(pod, machines) else {
            return self.without_machine(pod).await;
        };
        if forge::delete_requested(m) {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                s.message = "machine is being deleted".into();
            })?;
            return Ok(());
        }
        let host = m.status.host.clone();
        let addr = m.status.addresses.first().cloned();
        let mid = m.meta.id.clone();
        let exited = m
            .status
            .conditions
            .iter()
            .find(|c| c.kind == EXITED_CONDITION && c.status)
            .map(|c| (c.reason.parse::<i32>().unwrap_or(-1), c.message.clone()));
        let mphase = m.status.phase;
        let mpower = m.status.power;
        // A reboot we asked for that forge has not carried out yet: the
        // machine still looks exited, but the pod is on its way back up.
        let restart_in_flight = m.spec.restarts > m.status.restarts;
        let mmsg = m.status.message.clone();
        let names: Vec<(String, String)> = pod
            .spec
            .containers
            .iter()
            .map(|c| (c.name.clone(), c.image.clone()))
            .collect();

        let updated = patch_status(&self.pods(), &pod.meta.id, |s| {
            s.machine = Some(mid.clone());
            s.host = host.clone();
            ensure_container_statuses(s, &names);
            set_condition(
                &mut s.conditions,
                "PodScheduled",
                host.is_some(),
                if host.is_some() {
                    "Scheduled"
                } else {
                    "Unschedulable"
                },
                if host.is_some() { "" } else { &mmsg },
            );
            match mphase {
                Phase::Ready if restart_in_flight => {
                    s.phase = PodPhase::Pending;
                    s.message = "restarting".into();
                }
                Phase::Ready => match (mpower, &exited) {
                    (PowerState::Off, Some((code, which))) => {
                        let ok = *code == 0;
                        s.phase = if ok {
                            PodPhase::Succeeded
                        } else {
                            PodPhase::Failed
                        };
                        s.message = format!("container {which} exited with code {code}");
                        for c in &mut s.container_statuses {
                            c.ready = false;
                            c.state = ContainerState::Terminated {
                                exit_code: *code,
                                reason: if ok { "Completed" } else { "Error" }.into(),
                                finished_at: Utc::now(),
                            };
                        }
                        set_condition(&mut s.conditions, "Ready", false, "Terminated", "");
                    }
                    (PowerState::Off, None) => {
                        s.phase = PodPhase::Unknown;
                        s.message = "machine is powered off".into();
                        set_condition(&mut s.conditions, "Ready", false, "PoweredOff", "");
                    }
                    _ => {
                        s.phase = PodPhase::Running;
                        s.pod_ip = addr.clone();
                        s.message.clear();
                        let started = s.start_time.unwrap_or_else(Utc::now);
                        s.start_time = Some(started);
                        for c in &mut s.container_statuses {
                            c.ready = true;
                            if !matches!(c.state, ContainerState::Running { .. }) {
                                c.state = ContainerState::Running {
                                    started_at: Utc::now(),
                                };
                            }
                        }
                        set_condition(&mut s.conditions, "Ready", true, "Running", "");
                    }
                },
                Phase::Error => {
                    s.phase = PodPhase::Failed;
                    s.message = mmsg.clone();
                    for c in &mut s.container_statuses {
                        c.ready = false;
                        c.state = ContainerState::Waiting {
                            reason: "StartFailed".into(),
                        };
                    }
                    set_condition(&mut s.conditions, "Ready", false, "MachineError", &mmsg);
                }
                Phase::Pending | Phase::Creating | Phase::Updating => {
                    if !s.phase.is_terminal() || mphase != Phase::Updating {
                        s.phase = PodPhase::Pending;
                    }
                    s.message = mmsg.clone();
                    for c in &mut s.container_statuses {
                        c.ready = false;
                        c.state = ContainerState::Waiting {
                            reason: "ContainerCreating".into(),
                        };
                    }
                    set_condition(&mut s.conditions, "Ready", false, "Starting", "");
                }
                Phase::Deleting => {
                    s.message = "machine is being deleted".into();
                }
            }
        })?;

        // Restart policy: on a terminal pod whose machine still exists, ask
        // forge for a reboot once the backoff has elapsed.
        let restart = matches!(
            (updated.spec.restart_policy, updated.status.phase),
            (
                RestartPolicy::Always,
                PodPhase::Succeeded | PodPhase::Failed
            ) | (RestartPolicy::OnFailure, PodPhase::Failed)
        );
        if restart && !restart_in_flight && matches!(mphase, Phase::Ready | Phase::Error) {
            self.maybe_restart(&updated, &mid).await?;
        }
        Ok(())
    }

    async fn maybe_restart(&self, pod: &Pod, machine: &Id) -> anyhow::Result<()> {
        let restarts = pod
            .status
            .container_statuses
            .iter()
            .map(|c| c.restart_count)
            .max()
            .unwrap_or(0);
        let backoff = Duration::seconds((10i64 << restarts.min(5)).min(300));
        let since = pod
            .status
            .conditions
            .iter()
            .find(|c| c.kind == "Ready")
            .map(|c| c.last_transition)
            .unwrap_or_else(Utc::now);
        if Utc::now() - since < backoff {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                set_condition(
                    &mut s.conditions,
                    "BackOff",
                    true,
                    "Waiting",
                    &format!("restart {} after {}s", restarts + 1, backoff.num_seconds()),
                );
            })?;
            return Ok(());
        }
        match self.machines().modify(machine, |m| {
            m.spec.restarts += 1;
            m.spec.power = forge::DesiredPower::On;
            Ok(())
        }) {
            Ok(_) | Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        patch_status(&self.pods(), &pod.meta.id, |s| {
            s.phase = PodPhase::Pending;
            s.message = "restarting".into();
            for c in &mut s.container_statuses {
                c.restart_count += 1;
                c.ready = false;
                c.state = ContainerState::Waiting {
                    reason: "Restarting".into(),
                };
            }
            set_condition(&mut s.conditions, "Ready", false, "Restarting", "");
            set_condition(&mut s.conditions, "BackOff", false, "Restarted", "");
        })?;
        tracing::info!(pod = %pod.meta.id, restarts = restarts + 1, "loom: restarting pod");
        Ok(())
    }

    async fn without_machine(&self, pod: &Pod) -> anyhow::Result<()> {
        let vanished = pod.status.machine.is_some();
        let recreate = match pod.spec.restart_policy {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => pod.status.phase != PodPhase::Succeeded,
            RestartPolicy::Never => !vanished && !pod.status.phase.is_terminal(),
        };
        if vanished && !recreate {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                if !s.phase.is_terminal() {
                    s.phase = PodPhase::Failed;
                    s.message = "machine disappeared".into();
                }
                s.pod_ip = None;
                s.host = None;
                for c in &mut s.container_statuses {
                    c.ready = false;
                }
                set_condition(&mut s.conditions, "Ready", false, "MachineGone", "");
            })?;
            return Ok(());
        }
        if !recreate {
            return Ok(());
        }
        let m = self.create_machine(pod)?;
        let mid = m.meta.id.clone();
        let names: Vec<(String, String)> = pod
            .spec
            .containers
            .iter()
            .map(|c| (c.name.clone(), c.image.clone()))
            .collect();
        patch_status(&self.pods(), &pod.meta.id, |s| {
            s.machine = Some(mid.clone());
            s.phase = PodPhase::Pending;
            s.pod_ip = None;
            s.host = None;
            s.message.clear();
            ensure_container_statuses(s, &names);
            for c in &mut s.container_statuses {
                if vanished {
                    c.restart_count += 1;
                }
                c.ready = false;
                c.state = ContainerState::Waiting {
                    reason: "ContainerCreating".into(),
                };
            }
            set_condition(&mut s.conditions, "PodScheduled", false, "Pending", "");
            set_condition(&mut s.conditions, "Ready", false, "Starting", "");
        })?;
        tracing::info!(pod = %pod.meta.id, machine = %mid, "loom: machine created for pod");
        Ok(())
    }
}

fn ensure_container_statuses(s: &mut PodStatus, names: &[(String, String)]) {
    if s.container_statuses.len() != names.len()
        || s.container_statuses
            .iter()
            .zip(names)
            .any(|(c, (n, _))| &c.name != n)
    {
        s.container_statuses = names
            .iter()
            .map(|(n, i)| ContainerStatus {
                name: n.clone(),
                image: i.clone(),
                ..Default::default()
            })
            .collect();
    }
}

#[async_trait::async_trait]
impl Controller for PodController {
    fn name(&self) -> &'static str {
        "pod"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[crate::types::kinds::POD, pistack_types::kind::MACHINE]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let pods = self.pods().list(None)?;
        let machines = self.machines().list(None)?;
        for pod in &pods {
            let res = if crate::types::delete_requested(pod) {
                self.teardown(pod, &machines).await
            } else {
                self.converge(pod, &machines).await
            };
            if let Err(e) = res {
                tracing::warn!(pod = %pod.meta.id, error = %e, "loom: pod reconcile failed");
            }
        }
        Ok(())
    }
}
