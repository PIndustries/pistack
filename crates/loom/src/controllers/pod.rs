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
    set_condition, ContainerState, ContainerStatus, EnvVarSource, Pod, PodPhase, PodStatus,
    RestartPolicy, VolumeSource, MACHINE_POD_LABEL,
};
use chrono::{Duration, Utc};
use forge::{Machine, MachineKind, MachineSpec, PowerState, Size};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{Id, Phase, Resources};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
    /// ConfigMap file bodies keyed by name and by id. Empty from
    /// [`PodSandbox::from_pod`]; filled when the machine is created.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config_maps: BTreeMap<String, ProjectedFiles>,
    /// Secret file bodies keyed by name and by id. Same lifetime as
    /// [`Self::config_maps`]. Payloads are not part of the pod spec.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, ProjectedFiles>,
}

/// File bodies for one ConfigMap or Secret. `binary_data` values are base64.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectedFiles {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub data: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binary_data: BTreeMap<String, String>,
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
            config_maps: BTreeMap::new(),
            secrets: BTreeMap::new(),
        }
    }
}

/// Copy ConfigMap and Secret bodies the pod references into the sandbox
/// document the agent receives. A missing object is left out; the runtime
/// then starts without that value.
fn fill_projections(store: &Store, sandbox: &mut PodSandbox) {
    let project = sandbox.pod.project.clone();
    let (config_maps, secrets) = referenced_objects(sandbox);
    for name in config_maps {
        let Some(cm) = find_config_map(store, project.as_ref(), &name) else {
            tracing::warn!(config_map = %name, "loom: config map is not available for the sandbox");
            continue;
        };
        remember(
            &mut sandbox.config_maps,
            &cm.meta.name,
            &cm.meta.id.to_string(),
            ProjectedFiles {
                data: cm.spec.data.clone(),
                binary_data: cm.spec.binary_data.clone(),
            },
        );
    }
    for name in secrets {
        let Some(id) = find_secret_id(store, project.as_ref(), &name) else {
            tracing::warn!(secret = %name, "loom: secret is not available for the sandbox");
            continue;
        };
        let bytes = match keep::read_payload(store, &id) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(secret = %name, error = %e, "loom: secret payload is not available for the sandbox");
                continue;
            }
        };
        let label = store
            .collection::<keep::Secret>()
            .get(&id)
            .map(|s| s.meta.name)
            .unwrap_or_default();
        remember(
            &mut sandbox.secrets,
            &label,
            &id.to_string(),
            project_secret(&bytes),
        );
    }
}

fn referenced_objects(sandbox: &PodSandbox) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut config_maps = BTreeSet::new();
    let mut secrets = BTreeSet::new();
    for container in &sandbox.containers {
        for env in &container.env {
            match &env.value_from {
                Some(EnvVarSource::ConfigMap { name, .. }) => {
                    config_maps.insert(name.clone());
                }
                Some(EnvVarSource::Secret { name, .. }) => {
                    secrets.insert(name.clone());
                }
                None => {}
            }
        }
    }
    for volume in &sandbox.volumes {
        match &volume.source {
            VolumeSource::ConfigMap(name) => {
                config_maps.insert(name.clone());
            }
            VolumeSource::Secret(name) => {
                secrets.insert(name.clone());
            }
            VolumeSource::EmptyDir | VolumeSource::Volume(_) => {}
        }
    }
    (config_maps, secrets)
}

fn remember(
    map: &mut BTreeMap<String, ProjectedFiles>,
    name: &str,
    id: &str,
    files: ProjectedFiles,
) {
    if !name.is_empty() {
        map.insert(name.to_owned(), files.clone());
    }
    if !id.is_empty() && id != name {
        map.insert(id.to_owned(), files);
    }
}

fn find_config_map(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
) -> Option<crate::types::ConfigMap> {
    let col = store.collection::<crate::types::ConfigMap>();
    if let Some(id) = Id::parse(crate::types::kinds::CONFIG_MAP, name_or_id) {
        if let Ok(cm) = col.get(&id) {
            return Some(cm);
        }
    }
    col.get_by_name(project, name_or_id).ok()
}

fn find_secret_id(store: &Store, project: Option<&Id>, name_or_id: &str) -> Option<Id> {
    if let Some(id) = Id::parse(pistack_types::kind::SECRET, name_or_id) {
        return Some(id);
    }
    store
        .collection::<keep::Secret>()
        .get_by_name(project, name_or_id)
        .ok()
        .map(|s| s.meta.id)
}

/// A JSON object of strings becomes one key per field. Any other payload is
/// a single `data` entry (text, or base64 when the bytes are not text).
fn project_secret(bytes: &[u8]) -> ProjectedFiles {
    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(text) {
            if !map.is_empty() {
                return ProjectedFiles {
                    data: map,
                    binary_data: BTreeMap::new(),
                };
            }
        }
        let mut data = BTreeMap::new();
        data.insert("data".into(), text.to_owned());
        return ProjectedFiles {
            data,
            binary_data: BTreeMap::new(),
        };
    }
    use base64::{engine::general_purpose::STANDARD, Engine};
    let mut binary_data = BTreeMap::new();
    binary_data.insert("data".into(), STANDARD.encode(bytes));
    ProjectedFiles {
        data: BTreeMap::new(),
        binary_data,
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
        let mut sandbox = PodSandbox::from_pod(pod);
        fill_projections(&self.store, &mut sandbox);
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

#[cfg(test)]
mod tests {
    use super::{fill_projections, project_secret, PodSandbox};
    use crate::types::{
        ConfigMap, ConfigMapSpec, Container, EnvVar, EnvVarSource, Pod, PodSpec, PodVolume,
        VolumeSource,
    };
    use base64::{engine::general_purpose::STANDARD, Engine};
    use pistack_store::Store;
    use pistack_types::{kind, Id};
    use std::collections::BTreeMap;

    #[test]
    fn project_secret_splits_a_json_object_and_keeps_other_payloads() {
        let json = project_secret(br#"{"password":"s3cret-value"}"#);
        assert_eq!(
            json.data.get("password").map(String::as_str),
            Some("s3cret-value")
        );
        assert!(json.binary_data.is_empty());

        let text = project_secret(b"plain-token");
        assert_eq!(
            text.data.get("data").map(String::as_str),
            Some("plain-token")
        );

        let empty = project_secret(b"{}");
        assert_eq!(empty.data.get("data").map(String::as_str), Some("{}"));

        let odd = project_secret(br#"{"n":1}"#);
        assert_eq!(odd.data.get("data").map(String::as_str), Some(r#"{"n":1}"#));

        let raw = [0xff, 0xfe];
        let binary = project_secret(&raw);
        assert!(binary.data.is_empty());
        assert_eq!(
            binary.binary_data.get("data").map(String::as_str),
            Some(STANDARD.encode(raw).as_str())
        );
    }

    #[test]
    fn sandbox_carries_projected_files_the_debug_view_omits() {
        let store = Store::in_memory().unwrap();
        let project = Id::new(kind::PROJECT);
        let keep = keep::Client::open(&store).unwrap();
        let secret = keep
            .create_secret(
                keep::Secret::new("db", Some(project.clone()), keep::SecretSpec::default()),
                br#"{"password":"s3cret-value"}"#,
            )
            .unwrap();
        let plain = keep
            .create_secret(
                keep::Secret::new("token", Some(project.clone()), keep::SecretSpec::default()),
                b"plain-token",
            )
            .unwrap();
        let mut data = BTreeMap::new();
        data.insert("MODE".into(), "prod".into());
        let cm = store
            .collection::<ConfigMap>()
            .create(ConfigMap::new(
                "app",
                Some(project.clone()),
                ConfigMapSpec {
                    data,
                    ..ConfigMapSpec::default()
                },
            ))
            .unwrap();
        let pod = Pod::new(
            "web",
            Some(project),
            PodSpec {
                containers: vec![Container {
                    name: "web".into(),
                    image: "img".into(),
                    env: vec![
                        EnvVar {
                            name: "MODE".into(),
                            value_from: Some(EnvVarSource::ConfigMap {
                                name: "app".into(),
                                key: "MODE".into(),
                            }),
                            ..EnvVar::default()
                        },
                        EnvVar {
                            name: "PASSWORD".into(),
                            value_from: Some(EnvVarSource::Secret {
                                name: secret.meta.id.to_string(),
                                key: "password".into(),
                            }),
                            ..EnvVar::default()
                        },
                    ],
                    ..Container::default()
                }],
                volumes: vec![
                    PodVolume {
                        name: "cfg".into(),
                        source: VolumeSource::ConfigMap(cm.meta.id.to_string()),
                    },
                    PodVolume {
                        name: "sec".into(),
                        source: VolumeSource::Secret(secret.meta.name.clone()),
                    },
                    PodVolume {
                        name: "tok".into(),
                        source: VolumeSource::Secret(plain.meta.id.to_string()),
                    },
                ],
                ..PodSpec::default()
            },
        );
        let mut sandbox = PodSandbox::from_pod(&pod);
        let debug = serde_json::to_value(&sandbox).unwrap();
        assert!(debug.get("config_maps").is_none());
        assert!(debug.get("secrets").is_none());
        assert!(!debug.to_string().contains("s3cret-value"));
        fill_projections(&store, &mut sandbox);
        let by_name = sandbox.config_maps.get("app").unwrap();
        let by_id = sandbox.config_maps.get(&cm.meta.id.to_string()).unwrap();
        assert_eq!(by_name.data.get("MODE").map(String::as_str), Some("prod"));
        assert_eq!(by_id.data.get("MODE").map(String::as_str), Some("prod"));
        assert_eq!(
            sandbox
                .secrets
                .get("db")
                .unwrap()
                .data
                .get("password")
                .map(String::as_str),
            Some("s3cret-value")
        );
        assert_eq!(
            sandbox
                .secrets
                .get(&secret.meta.id.to_string())
                .unwrap()
                .data
                .get("password")
                .map(String::as_str),
            Some("s3cret-value")
        );
        assert_eq!(
            sandbox
                .secrets
                .get("token")
                .unwrap()
                .data
                .get("data")
                .map(String::as_str),
            Some("plain-token")
        );
        assert_eq!(
            sandbox
                .secrets
                .get(&plain.meta.id.to_string())
                .unwrap()
                .data
                .get("data")
                .map(String::as_str),
            Some("plain-token")
        );
        let machine = serde_json::to_string(&sandbox).unwrap();
        assert!(machine.contains("s3cret-value"));
        assert!(machine.contains("plain-token"));
    }
}
