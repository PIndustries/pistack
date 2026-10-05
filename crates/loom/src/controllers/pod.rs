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
//! * `phase = Error` when the sandbox or a container cannot be started;
//! * `status.probes` carries the host agent's liveness and readiness
//!   streaks for each container.
//! * a `Network` condition with status false means the sandbox is up and
//!   the recorded address is not on an interface. The pod stays `Running`,
//!   `pod_ip` stays empty, and Ready is false.
//!
//! The controller maps that to the pod: `Running`, `Succeeded` (exit 0),
//! `Failed`, and restarts per `restart_policy` by bumping the machine's
//! `spec.restarts` (the same knob `POST /machines/{id}/reboot` uses) with an
//! exponential backoff capped at five minutes. A container with no
//! readiness probe is ready once it is running. A readiness probe holds
//! the pod's Ready condition false until its success streak reaches
//! `success_threshold`. A liveness streak that reaches `failure_threshold`
//! restarts that container the same way, without the exit backoff, and
//! bumps only that container's restart count. `http_get`, `tcp_socket`, and
//! `exec` all use those thresholds. An exec probe the runtime cannot run is
//! reported unsupported and does not restart the container.
//!
//! A pod that names a slab volume stays Pending until every such volume is
//! attached on one node and the status records a host path or a device.
//! The machine is created only then, pinned to that node, with the path in
//! the sandbox document. If a volume attaches after the pod is already
//! running without it, the pod is marked not Ready and the machine restart
//! counter is bumped so the runtime creates the containers again.

use super::{delete_quietly, patch_status, request_delete, Controller};
use crate::types::{
    set_condition, ContainerState, ContainerStatus, EnvVarSource, Pod, PodPhase, PodStatus,
    ProbeHandlerKind, RestartPolicy, VolumeSource, MACHINE_POD_LABEL, POD_READY,
};
use chrono::{Duration, Utc};
use forge::{Machine, MachineKind, MachineSpec, PowerState, Size};
use pistack_rpc::proto::{ProbeReport, PROBE_LIVENESS, PROBE_READINESS};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{Id, Phase, Resource, Resources, Spec};
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
    /// Slab volume id to the absolute host path recorded at attach.
    /// Empty until the volume is attached. The youki runtime bind-mounts
    /// these paths when it creates the containers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub volume_paths: BTreeMap<String, String>,
    /// Slab volume id to a device name, when the volume has no host path
    /// (an RBD image is mapped to a device after the bundle exists).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub volume_devices: BTreeMap<String, String>,
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
/// Pod condition set from liveness probes. Absent when the pod has none.
const LIVENESS_CONDITION: &str = "Liveness";
/// Network attached when the pod names none and the project has a network
/// by this name. Otherwise the project's only non-external network is used
/// (see [`pick_networks`]).
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
            volume_paths: BTreeMap::new(),
            volume_devices: BTreeMap::new(),
        }
    }
}

/// Slab volume ids included in the sandbox the agent was asked to start.
const MOUNTED_VOLUMES: &str = "loom.pistack.io/mounted-volumes";

/// Wire view of a slab volume. Loom does not depend on the slab crate.
/// Unknown spec fields are ignored so a volume written by slab still reads.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
struct SlabVolSpec {
    #[serde(default)]
    size_gib: u64,
}

impl Spec for SlabVolSpec {
    const KIND: &'static str = pistack_types::kind::VOLUME;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
struct SlabCondition {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    status: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
struct SlabVolStatus {
    #[serde(default)]
    phase: Phase,
    #[serde(default)]
    attached_to: Option<Id>,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    backend_ref: Option<String>,
    /// Host path, when storage records it separately from `backend_ref`.
    #[serde(default)]
    host_path: Option<String>,
    /// Host id the volume is attached on, when storage records one.
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    conditions: Vec<SlabCondition>,
}

type SlabVolume = Resource<SlabVolSpec, SlabVolStatus>;

/// Paths and the node to pin, once every slab volume on the pod is attached.
struct SlabBind {
    paths: BTreeMap<String, String>,
    devices: BTreeMap<String, String>,
    node: Id,
    /// Sorted volume ids, comma-separated.
    mounted: String,
    names: BTreeMap<String, String>,
}

enum SlabHold {
    /// The pod names no slab volume.
    None,
    /// Do not create or start. The message names the volume.
    Wait(String),
    Ready(SlabBind),
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

/// Networks a new pod machine joins. The pod's own `networks` when it names
/// any; else `default` when the project can see a network by that name;
/// else the project's only non-external network. None or several is an
/// error that says which networks the pod could name.
pub fn pick_networks(store: &Store, pod: &Pod) -> Result<Vec<String>, String> {
    if !pod.spec.networks.is_empty() {
        return Ok(pod.spec.networks.clone());
    }
    let nets = store
        .collection::<fabric::Network>()
        .list(None)
        .map_err(|e| format!("networks are not available: {e}"))?;
    let visible: Vec<&fabric::Network> = nets
        .iter()
        .filter(|n| n.meta.project.is_none() || n.meta.project == pod.meta.project)
        .collect();
    if visible.iter().any(|n| n.meta.name == DEFAULT_NETWORK) {
        return Ok(vec![DEFAULT_NETWORK.into()]);
    }
    let mut usable: Vec<String> = visible
        .iter()
        .filter(|n| !n.spec.external)
        .map(|n| n.meta.name.clone())
        .collect();
    usable.sort();
    usable.dedup();
    match usable.len() {
        1 => Ok(usable),
        0 => Err(
            "pod names no network and its project has no network that is not external; \
             create one or name one in spec.networks"
                .into(),
        ),
        _ => Err(format!(
            "pod names no network and its project has several ({}); name one in spec.networks",
            usable.join(", ")
        )),
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

fn slab_refs(pod: &Pod) -> Vec<(&str, &Id)> {
    pod.spec
        .volumes
        .iter()
        .filter_map(|v| match &v.source {
            VolumeSource::Volume(id) => Some((v.name.as_str(), id)),
            _ => None,
        })
        .collect()
}

fn absolute_host_path(raw: &str) -> Option<&str> {
    if raw.starts_with('/') && !raw.contains('\0') && !raw.split('/').any(|part| part == "..") {
        Some(raw)
    } else {
        None
    }
}

fn host_id(raw: &str) -> Option<Id> {
    Id::parse(pistack_types::kind::HOST, raw)
}

fn machine_live(m: &Machine) -> bool {
    m.status.phase == Phase::Ready
        && m.status.power != PowerState::Off
        && m.spec.restarts <= m.status.restarts
}

fn mounted_set(m: &Machine) -> BTreeSet<String> {
    m.meta
        .annotations
        .get(MOUNTED_VOLUMES)
        .map(|raw| {
            raw.split(',')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

struct PublishedVol {
    id: String,
    name: String,
    path: Option<String>,
    device: Option<String>,
    node: Option<Id>,
    attached_here: bool,
}

fn publish_volume(
    vol: &SlabVolume,
    pod_name: &str,
    machine: Option<&Machine>,
    machines: &[Machine],
) -> Result<PublishedVol, String> {
    let name = if vol.meta.name.is_empty() {
        pod_name.to_string()
    } else {
        vol.meta.name.clone()
    };
    let wait = format!("waiting for volume {name}");
    if vol.status.phase != Phase::Ready {
        return Err(wait);
    }
    let attached = vol.status.attached_to.is_some()
        || vol
            .status
            .conditions
            .iter()
            .any(|c| c.kind == "Attached" && c.status);
    if !attached {
        return Err(wait);
    }
    let path = vol
        .status
        .host_path
        .as_deref()
        .and_then(absolute_host_path)
        .or_else(|| {
            vol.status
                .backend_ref
                .as_deref()
                .and_then(absolute_host_path)
        })
        .map(str::to_string);
    let device = vol
        .status
        .device
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    if path.is_none() && device.is_none() {
        return Err(wait);
    }
    let attached_here =
        machine.is_some_and(|m| vol.status.attached_to.as_ref() == Some(&m.meta.id));
    let node = vol.status.host.as_deref().and_then(host_id).or_else(|| {
        let mid = vol.status.attached_to.as_ref()?;
        machines
            .iter()
            .find(|m| &m.meta.id == mid)
            .and_then(|m| m.status.host.clone())
    });
    Ok(PublishedVol {
        id: vol.meta.id.to_string(),
        name,
        path,
        device,
        node,
        attached_here,
    })
}

fn decide_slab(
    pod: &Pod,
    machine: Option<&Machine>,
    machines: &[Machine],
    vols: &[SlabVolume],
) -> SlabHold {
    let named = slab_refs(pod);
    if named.is_empty() {
        return SlabHold::None;
    }
    let mut published = Vec::new();
    for (pod_name, id) in named {
        let Some(vol) = vols.iter().find(|v| &v.meta.id == id) else {
            return SlabHold::Wait(format!("waiting for volume {pod_name}"));
        };
        match publish_volume(vol, pod_name, machine, machines) {
            Ok(item) => published.push(item),
            Err(message) => return SlabHold::Wait(message),
        }
    }
    let placed = machine.and_then(|m| m.status.host.clone());
    let mut node = placed.clone();
    for item in &published {
        if let Some(host) = &placed {
            let here = item.attached_here || item.node.as_ref() == Some(host);
            if !here {
                return SlabHold::Wait(format!(
                    "volume {} is not attached on this node",
                    item.name
                ));
            }
            node = Some(host.clone());
        } else if let Some(found) = &item.node {
            match &node {
                None => node = Some(found.clone()),
                Some(have) if have == found => {}
                Some(_) => {
                    return SlabHold::Wait(format!(
                        "volume {} is not attached on this node",
                        item.name
                    ));
                }
            }
        } else {
            return SlabHold::Wait(format!("waiting for volume {}", item.name));
        }
    }
    let Some(node) = node else {
        return SlabHold::Wait(format!("waiting for volume {}", published[0].name));
    };
    let mut paths = BTreeMap::new();
    let mut devices = BTreeMap::new();
    let mut ids = Vec::new();
    let mut names = BTreeMap::new();
    for item in &published {
        ids.push(item.id.clone());
        names.insert(item.id.clone(), item.name.clone());
        if let Some(path) = &item.path {
            paths.insert(item.id.clone(), path.clone());
        } else if let Some(dev) = &item.device {
            devices.insert(item.id.clone(), dev.clone());
        }
    }
    ids.sort();
    SlabHold::Ready(SlabBind {
        paths,
        devices,
        node,
        mounted: ids.join(","),
        names,
    })
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
    fn slab_volumes(&self) -> Collection<SlabVolume> {
        self.store.collection::<SlabVolume>()
    }

    fn slab_hold(
        &self,
        pod: &Pod,
        machine: Option<&Machine>,
        machines: &[Machine],
    ) -> Result<SlabHold, StoreError> {
        if slab_refs(pod).is_empty() {
            return Ok(SlabHold::None);
        }
        let vols = self.slab_volumes().list(None)?;
        Ok(decide_slab(pod, machine, machines, &vols))
    }

    fn sandbox_json(
        &self,
        pod: &Pod,
        networks: &[String],
        bind: &SlabBind,
    ) -> Result<String, StoreError> {
        let mut sandbox = PodSandbox::from_pod(pod);
        sandbox.networks = networks.to_vec();
        fill_projections(&self.store, &mut sandbox);
        sandbox.volume_paths = bind.paths.clone();
        sandbox.volume_devices = bind.devices.clone();
        Ok(serde_json::to_string(&sandbox)?)
    }

    fn mark_waiting(&self, pod: &Pod, message: &str) -> Result<(), StoreError> {
        patch_status(&self.pods(), &pod.meta.id, |s| {
            if !s.phase.is_terminal() {
                s.phase = PodPhase::Pending;
            }
            s.message = message.to_string();
            for c in &mut s.container_statuses {
                c.ready = false;
                c.state = ContainerState::Waiting {
                    reason: "VolumeNotAttached".into(),
                };
            }
            set_condition(
                &mut s.conditions,
                POD_READY,
                false,
                "VolumeNotAttached",
                message,
            );
        })?;
        Ok(())
    }

    fn hold_for_volume(
        &self,
        pod: &Pod,
        machine: &Machine,
        message: &str,
    ) -> Result<(), StoreError> {
        if !machine_live(machine) {
            let current = machine
                .meta
                .annotations
                .get(forge::WAIT_VOLUME_ANNOTATION)
                .map(String::as_str);
            if current != Some(message) {
                match self.machines().modify(&machine.meta.id, |m| {
                    m.meta
                        .annotations
                        .insert(forge::WAIT_VOLUME_ANNOTATION.into(), message.to_string());
                    Ok(())
                }) {
                    Ok(_) | Err(StoreError::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.mark_waiting(pod, message)
    }

    /// Write the attached paths onto the machine. A volume that showed up
    /// after the containers were created bumps the restart counter.
    /// Returns true when the pod was marked not Ready for that restart.
    fn sync_mounted(
        &self,
        pod: &Pod,
        machine: &Machine,
        bind: &SlabBind,
    ) -> Result<bool, StoreError> {
        let mounted = mounted_set(machine);
        let missing: Vec<&str> = bind
            .mounted
            .split(',')
            .filter(|id| !id.is_empty() && !mounted.contains(*id))
            .collect();
        let networks = if machine.spec.networks.is_empty() {
            networks_of(pod)
        } else {
            machine.spec.networks.clone()
        };
        let user_data = self.sandbox_json(pod, &networks, bind)?;
        let node = bind.node.to_string();
        if machine_live(machine) && !missing.is_empty() {
            let id = missing[0];
            let name = bind.names.get(id).map(String::as_str).unwrap_or(id);
            let message = format!("volume {name} attached after the pod started");
            match self.machines().modify(&machine.meta.id, |m| {
                m.spec.user_data = Some(user_data.clone());
                m.spec.restarts += 1;
                m.spec.power = forge::DesiredPower::On;
                m.meta
                    .annotations
                    .insert(MOUNTED_VOLUMES.into(), bind.mounted.clone());
                m.meta
                    .annotations
                    .insert(forge::NODE_PIN_ANNOTATION.into(), node.clone());
                m.meta.annotations.remove(forge::WAIT_VOLUME_ANNOTATION);
                Ok(())
            }) {
                Ok(_) | Err(StoreError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
            patch_status(&self.pods(), &pod.meta.id, |s| {
                s.phase = PodPhase::Pending;
                s.message = message.clone();
                for c in &mut s.container_statuses {
                    c.ready = false;
                    c.state = ContainerState::Waiting {
                        reason: "VolumeAttached".into(),
                    };
                }
                set_condition(
                    &mut s.conditions,
                    POD_READY,
                    false,
                    "VolumeAttached",
                    &message,
                );
            })?;
            tracing::info!(pod = %pod.meta.id, %message, "loom: recreating pod for a late slab volume");
            return Ok(true);
        }
        let pin_now = machine
            .meta
            .annotations
            .get(forge::NODE_PIN_ANNOTATION)
            .map(String::as_str);
        let mounted_now = machine
            .meta
            .annotations
            .get(MOUNTED_VOLUMES)
            .map(String::as_str);
        let waiting = machine
            .meta
            .annotations
            .contains_key(forge::WAIT_VOLUME_ANNOTATION);
        let data_now = machine.spec.user_data.as_deref();
        if pin_now == Some(node.as_str())
            && mounted_now == Some(bind.mounted.as_str())
            && !waiting
            && data_now == Some(user_data.as_str())
        {
            return Ok(false);
        }
        match self.machines().modify(&machine.meta.id, |m| {
            m.spec.user_data = Some(user_data.clone());
            m.meta
                .annotations
                .insert(MOUNTED_VOLUMES.into(), bind.mounted.clone());
            m.meta
                .annotations
                .insert(forge::NODE_PIN_ANNOTATION.into(), node.clone());
            let old = m.meta.annotations.remove(forge::WAIT_VOLUME_ANNOTATION);
            if old.as_deref() == Some(m.status.message.as_str()) {
                m.status.message.clear();
            }
            Ok(())
        }) {
            Ok(_) | Err(StoreError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
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

    fn create_machine(
        &self,
        pod: &Pod,
        networks: Vec<String>,
        bind: Option<&SlabBind>,
    ) -> Result<Machine, StoreError> {
        let want = pod.spec.resources();
        let size = self.size_for(&want)?;
        let image = pod
            .spec
            .containers
            .first()
            .map(|c| c.image.clone())
            .unwrap_or_default();
        let mut sandbox = PodSandbox::from_pod(pod);
        sandbox.networks = networks.clone();
        fill_projections(&self.store, &mut sandbox);
        if let Some(bind) = bind {
            sandbox.volume_paths = bind.paths.clone();
            sandbox.volume_devices = bind.devices.clone();
        }
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
            networks,
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
        if let Some(bind) = bind {
            m.meta
                .annotations
                .insert(forge::NODE_PIN_ANNOTATION.into(), bind.node.to_string());
            m.meta
                .annotations
                .insert(MOUNTED_VOLUMES.into(), bind.mounted.clone());
        }
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
            return self.without_machine(pod, machines).await;
        };
        if forge::delete_requested(m) {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                s.message = "machine is being deleted".into();
            })?;
            return Ok(());
        }
        match self.slab_hold(pod, Some(m), machines)? {
            SlabHold::Wait(message) => {
                self.hold_for_volume(pod, m, &message)?;
                return Ok(());
            }
            SlabHold::Ready(bind) => {
                if self.sync_mounted(pod, m, &bind)? {
                    return Ok(());
                }
            }
            SlabHold::None => {}
        }
        let mid_now = m.meta.id.clone();
        let m = match self.machines().get(&mid_now) {
            Ok(m) => m,
            Err(StoreError::NotFound(_)) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
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
        let network_gap = m
            .status
            .conditions
            .iter()
            .find(|c| c.kind == "Network" && !c.status)
            .map(|c| {
                if c.message.is_empty() {
                    mmsg.clone()
                } else {
                    c.message.clone()
                }
            });
        let names: Vec<(String, String)> = pod
            .spec
            .containers
            .iter()
            .map(|c| (c.name.clone(), c.image.clone()))
            .collect();
        let reports = m.status.probes.clone();
        // Same branch as the running arm below: power is on (or not yet
        // known) and a restart we asked for has not landed.
        let check_liveness =
            matches!(mphase, Phase::Ready) && !restart_in_flight && mpower != PowerState::Off;

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
                        set_condition(&mut s.conditions, POD_READY, false, "Terminated", "");
                    }
                    (PowerState::Off, None) => {
                        s.phase = PodPhase::Unknown;
                        s.message = "machine is powered off".into();
                        set_condition(&mut s.conditions, POD_READY, false, "PoweredOff", "");
                    }
                    _ => {
                        s.phase = PodPhase::Running;
                        let started = s.start_time.unwrap_or_else(Utc::now);
                        s.start_time = Some(started);
                        if let Some(gap) = network_gap.as_deref() {
                            s.pod_ip = None;
                            s.message = gap.to_owned();
                            set_condition(&mut s.conditions, "Network", false, "Rootless", gap);
                            apply_readiness(pod, &reports, s);
                            set_condition(&mut s.conditions, POD_READY, false, "NoNetwork", gap);
                        } else {
                            s.pod_ip = addr.clone();
                            s.message.clear();
                            s.conditions.retain(|c| c.kind != "Network");
                            apply_readiness(pod, &reports, s);
                        }
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
                    set_condition(&mut s.conditions, POD_READY, false, "MachineError", &mmsg);
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
                    set_condition(&mut s.conditions, POD_READY, false, "Starting", "");
                }
                Phase::Deleting => {
                    s.message = "machine is being deleted".into();
                }
            }
        })?;

        if check_liveness {
            self.apply_liveness(&updated, &mid, &reports).await?;
        }

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
            .find(|c| c.kind == POD_READY)
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
            set_condition(&mut s.conditions, POD_READY, false, "Restarting", "");
            set_condition(&mut s.conditions, "BackOff", false, "Restarted", "");
        })?;
        tracing::info!(pod = %pod.meta.id, restarts = restarts + 1, "loom: restarting pod");
        Ok(())
    }

    async fn without_machine(&self, pod: &Pod, machines: &[Machine]) -> anyhow::Result<()> {
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
                set_condition(&mut s.conditions, POD_READY, false, "MachineGone", "");
            })?;
            return Ok(());
        }
        if !recreate {
            return Ok(());
        }
        let bind = match self.slab_hold(pod, None, machines)? {
            SlabHold::Wait(message) => {
                self.mark_waiting(pod, &message)?;
                return Ok(());
            }
            SlabHold::None => None,
            SlabHold::Ready(bind) => Some(bind),
        };
        let networks = match pick_networks(&self.store, pod) {
            Ok(networks) => networks,
            Err(message) => {
                if pod.status.message != message {
                    patch_status(&self.pods(), &pod.meta.id, |s| {
                        s.phase = PodPhase::Pending;
                        s.message = message.clone();
                        set_condition(
                            &mut s.conditions,
                            "PodScheduled",
                            false,
                            "NoNetwork",
                            &message,
                        );
                    })?;
                    tracing::warn!(pod = %pod.meta.id, %message, "loom: pod has no network");
                }
                return Ok(());
            }
        };
        let m = self.create_machine(pod, networks, bind.as_ref())?;
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
            set_condition(&mut s.conditions, POD_READY, false, "Starting", "");
        })?;
        tracing::info!(pod = %pod.meta.id, machine = %mid, "loom: machine created for pod");
        Ok(())
    }

    /// Restart containers whose liveness streak has reached the threshold.
    ///
    /// The whole sandbox restarts, because that is the machine restart
    /// knob, and only the containers that failed have their restart count
    /// bumped. `Never` records the failure and leaves the pod running.
    /// A probe the runtime reports unsupported does not count as a failure.
    async fn apply_liveness(
        &self,
        pod: &Pod,
        machine: &Id,
        reports: &[ProbeReport],
    ) -> anyhow::Result<()> {
        let mut failed = Vec::new();
        let mut saw_check = false;
        let mut unsupported_message: Option<String> = None;
        let mut missing = false;
        for container in &pod.spec.containers {
            let Some(probe) = &container.liveness_probe else {
                continue;
            };
            match probe.handler() {
                ProbeHandlerKind::Http | ProbeHandlerKind::Tcp | ProbeHandlerKind::Exec => {
                    let report = probe_report(reports, &container.name, PROBE_LIVENESS);
                    if report.is_some_and(|r| r.unsupported) {
                        if unsupported_message.is_none() {
                            unsupported_message = Some(unsupported_text(report));
                        }
                        continue;
                    }
                    saw_check = true;
                    let failures = report.map(|r| r.failures).unwrap_or(0);
                    if failures >= probe.failures_required() {
                        failed.push(container.name.clone());
                    }
                }
                ProbeHandlerKind::Missing => missing = true,
            }
        }
        if failed.is_empty() && !saw_check && unsupported_message.is_none() && !missing {
            return Ok(());
        }
        if !failed.is_empty() {
            let names = failed.join(", ");
            match pod.spec.restart_policy {
                RestartPolicy::Always | RestartPolicy::OnFailure => {
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
                        s.message = format!("restarting {names}");
                        for c in &mut s.container_statuses {
                            if failed.contains(&c.name) {
                                c.restart_count += 1;
                            }
                            c.ready = false;
                            c.state = ContainerState::Waiting {
                                reason: "Restarting".into(),
                            };
                        }
                        set_condition(&mut s.conditions, POD_READY, false, "Restarting", "");
                        set_condition(
                            &mut s.conditions,
                            LIVENESS_CONDITION,
                            false,
                            "ProbeFailed",
                            &names,
                        );
                    })?;
                    tracing::info!(pod = %pod.meta.id, containers = %names, "loom: liveness restart");
                }
                RestartPolicy::Never => {
                    patch_status(&self.pods(), &pod.meta.id, |s| {
                        set_condition(
                            &mut s.conditions,
                            LIVENESS_CONDITION,
                            false,
                            "ProbeFailed",
                            &names,
                        );
                    })?;
                }
            }
            return Ok(());
        }
        if saw_check {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                set_condition(&mut s.conditions, LIVENESS_CONDITION, true, "Ok", "");
            })?;
            return Ok(());
        }
        let message = unsupported_message.unwrap_or_else(|| "probe has no handler".to_owned());
        patch_status(&self.pods(), &pod.meta.id, |s| {
            set_condition(
                &mut s.conditions,
                LIVENESS_CONDITION,
                false,
                "Unsupported",
                &message,
            );
        })?;
        Ok(())
    }
}

fn unsupported_text(report: Option<&ProbeReport>) -> String {
    report
        .map(|r| r.message.trim())
        .filter(|message| !message.is_empty())
        .unwrap_or("probe is not supported")
        .to_owned()
}

fn probe_report<'a>(
    reports: &'a [ProbeReport],
    container: &str,
    kind: &str,
) -> Option<&'a ProbeReport> {
    reports
        .iter()
        .find(|r| r.container == container && r.kind == kind)
}

fn mark_running(state: &mut ContainerState) {
    if !matches!(state, ContainerState::Running { .. }) {
        *state = ContainerState::Running {
            started_at: Utc::now(),
        };
    }
}

fn hold_not_ready(cs: &mut ContainerStatus, message: &str, waiting: &mut Vec<String>) {
    cs.ready = false;
    cs.state = ContainerState::Waiting {
        reason: message.to_owned(),
    };
    waiting.push(format!("{}: {message}", cs.name));
}

/// Set each container's ready flag and the pod Ready condition from the
/// readiness streaks on the machine. A container with no readiness probe
/// is ready. Phase is left to the caller.
fn apply_readiness(pod: &Pod, reports: &[ProbeReport], status: &mut PodStatus) {
    let mut waiting = Vec::new();
    for cs in &mut status.container_statuses {
        let Some(spec) = pod.spec.containers.iter().find(|c| c.name == cs.name) else {
            continue;
        };
        let Some(probe) = spec.readiness_probe.as_ref() else {
            cs.ready = true;
            mark_running(&mut cs.state);
            continue;
        };
        match probe.handler() {
            ProbeHandlerKind::Http | ProbeHandlerKind::Tcp | ProbeHandlerKind::Exec => {
                let report = probe_report(reports, &cs.name, PROBE_READINESS);
                if report.is_some_and(|r| r.unsupported) {
                    let message = unsupported_text(report);
                    hold_not_ready(cs, &message, &mut waiting);
                    continue;
                }
                let successes = report.map(|r| r.successes).unwrap_or(0);
                if successes >= probe.successes_required() {
                    cs.ready = true;
                    mark_running(&mut cs.state);
                } else {
                    let message = report
                        .map(|r| r.message.as_str())
                        .filter(|m| !m.is_empty())
                        .unwrap_or("readiness probe has not passed");
                    hold_not_ready(cs, message, &mut waiting);
                }
            }
            ProbeHandlerKind::Missing => {
                hold_not_ready(cs, "probe has no handler", &mut waiting);
            }
        }
    }
    if status.container_statuses.iter().all(|c| c.ready) {
        set_condition(&mut status.conditions, POD_READY, true, "Running", "");
    } else {
        set_condition(
            &mut status.conditions,
            POD_READY,
            false,
            "ContainersNotReady",
            &waiting.join("; "),
        );
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
        &[
            crate::types::kinds::POD,
            pistack_types::kind::MACHINE,
            pistack_types::kind::VOLUME,
        ]
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
    use super::{fill_projections, project_secret, PodController, PodSandbox, LIVENESS_CONDITION};
    use crate::controllers::Controller;
    use crate::types::{
        ConfigMap, ConfigMapSpec, Container, EnvVar, EnvVarSource, ExecProbe, HttpGetProbe, Pod,
        PodPhase, PodSpec, PodVolume, Probe, RestartPolicy, TcpSocketProbe, VolumeSource,
        MACHINE_POD_LABEL, POD_READY,
    };
    use base64::{engine::general_purpose::STANDARD, Engine};
    use forge::{new_machine, Machine, MachineKind, MachineSpec, PowerState};
    use pistack_rpc::proto::{ProbeReport, PROBE_LIVENESS, PROBE_READINESS};
    use pistack_store::Store;
    use pistack_types::{kind, Id, Phase};
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

    fn http_probe(port: u16, success: u32, failure: u32) -> Probe {
        Probe {
            http_get: Some(HttpGetProbe {
                path: "/health".into(),
                port,
                host: None,
                scheme: None,
            }),
            success_threshold: success,
            failure_threshold: failure,
            ..Probe::default()
        }
    }

    fn tcp_probe(port: u16, success: u32) -> Probe {
        Probe {
            tcp_socket: Some(TcpSocketProbe { port }),
            success_threshold: success,
            ..Probe::default()
        }
    }

    fn exec_probe() -> Probe {
        exec_probe_with(1, 3)
    }

    fn exec_probe_with(success: u32, failure: u32) -> Probe {
        Probe {
            exec: Some(ExecProbe {
                command: vec!["/bin/true".into()],
            }),
            success_threshold: success,
            failure_threshold: failure,
            ..Probe::default()
        }
    }

    fn ctr(name: &str, liveness: Option<Probe>, readiness: Option<Probe>) -> Container {
        Container {
            name: name.into(),
            image: "img".into(),
            liveness_probe: liveness,
            readiness_probe: readiness,
            ..Container::default()
        }
    }

    fn streak(
        container: &str,
        kind: &str,
        successes: u32,
        failures: u32,
        message: &str,
    ) -> ProbeReport {
        ProbeReport {
            container: container.into(),
            kind: kind.into(),
            success: failures == 0 && successes > 0,
            successes,
            failures,
            message: message.into(),
            unsupported: false,
        }
    }

    fn unsupported_report(container: &str, kind: &str) -> ProbeReport {
        ProbeReport {
            container: container.into(),
            kind: kind.into(),
            success: false,
            successes: 0,
            failures: 0,
            message: "exec probes are not supported".into(),
            unsupported: true,
        }
    }

    struct Fixture {
        store: Store,
        pod: Id,
        machine: Id,
    }

    fn running_pod(
        policy: RestartPolicy,
        containers: Vec<Container>,
        probes: Vec<ProbeReport>,
        spec_restarts: u64,
        status_restarts: u64,
        counts: &[(&str, u32)],
    ) -> Fixture {
        let store = Store::in_memory().unwrap();
        let mut pod = Pod::new(
            "web",
            None,
            PodSpec {
                containers,
                restart_policy: policy,
                ..PodSpec::default()
            },
        );
        if !counts.is_empty() {
            pod.status.container_statuses = pod
                .spec
                .containers
                .iter()
                .map(|c| crate::types::ContainerStatus {
                    name: c.name.clone(),
                    image: c.image.clone(),
                    restart_count: counts
                        .iter()
                        .find(|(n, _)| *n == c.name)
                        .map(|(_, count)| *count)
                        .unwrap_or(0),
                    ..Default::default()
                })
                .collect();
        }
        let pod = store.collection::<Pod>().create(pod).unwrap();
        let mut machine = new_machine(
            "pod-web",
            None,
            MachineSpec {
                kind: MachineKind::Container,
                size: "small".into(),
                image: "img".into(),
                restarts: spec_restarts,
                ..MachineSpec::default()
            },
        );
        machine
            .meta
            .labels
            .insert(MACHINE_POD_LABEL.into(), pod.meta.id.to_string());
        machine.status.phase = Phase::Ready;
        machine.status.power = PowerState::On;
        machine.status.restarts = status_restarts;
        machine.status.host = Some(Id::new(kind::HOST));
        machine.status.addresses = vec!["10.1.0.8".into()];
        machine.status.probes = probes;
        let machine = store.collection::<Machine>().create(machine).unwrap();
        Fixture {
            store,
            pod: pod.meta.id,
            machine: machine.meta.id.clone(),
        }
    }

    async fn pass(store: &Store) {
        PodController::new(store.clone())
            .reconcile_once()
            .await
            .unwrap();
    }

    fn load(store: &Store, id: &Id) -> Pod {
        store.collection::<Pod>().get(id).unwrap()
    }

    fn reason<'a>(pod: &'a Pod, kind: &str) -> &'a str {
        pod.status
            .conditions
            .iter()
            .find(|c| c.kind == kind)
            .map(|c| c.reason.as_str())
            .unwrap()
    }

    #[tokio::test]
    async fn pod_without_probes_is_ready_once_running() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", None, None)],
            vec![],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert_eq!(pod.status.pod_ip.as_deref(), Some("10.1.0.8"));
        assert!(pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "Running");
        assert!(pod.status.container_statuses[0].ready);
        assert!(pod
            .status
            .conditions
            .iter()
            .all(|c| c.kind != LIVENESS_CONDITION));
    }

    #[tokio::test]
    async fn rootless_network_gap_hides_the_address_and_is_not_ready() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", None, None)],
            vec![],
            0,
            0,
            &[],
        );
        let message = "pod has no network: a rootless agent cannot attach the recorded address. Run the agent as root or set PISTACK_YOUKI_SUDO=1";
        fx.store
            .collection::<Machine>()
            .modify(&fx.machine, |m| {
                m.status.message = message.to_owned();
                m.status
                    .set_condition("Network", false, "Rootless", message);
                Ok(())
            })
            .unwrap();
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert!(pod.status.pod_ip.is_none());
        assert!(
            pod.status.message.contains("pod has no network"),
            "{}",
            pod.status.message
        );
        let network = pod
            .status
            .conditions
            .iter()
            .find(|c| c.kind == "Network")
            .unwrap();
        assert!(!network.status);
        assert_eq!(network.reason, "Rootless");
        assert!(!pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "NoNetwork");

        fx.store
            .collection::<Machine>()
            .modify(&fx.machine, |m| {
                m.status.message.clear();
                m.status.conditions.retain(|c| c.kind != "Network");
                Ok(())
            })
            .unwrap();
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert_eq!(pod.status.pod_ip.as_deref(), Some("10.1.0.8"));
        assert!(pod.status.message.is_empty());
        assert!(pod.status.conditions.iter().all(|c| c.kind != "Network"));
        assert!(pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "Running");
    }

    #[tokio::test]
    async fn readiness_holds_ready_until_the_success_streak() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", None, Some(http_probe(8080, 2, 3)))],
            vec![streak("web", PROBE_READINESS, 1, 0, "")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert!(!pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "ContainersNotReady");
        assert!(!pod.status.container_statuses[0].ready);
        let message = pod
            .status
            .conditions
            .iter()
            .find(|c| c.kind == POD_READY)
            .unwrap()
            .message
            .clone();
        assert!(message.contains("readiness probe has not passed"));

        fx.store
            .collection::<Machine>()
            .modify(&fx.machine, |m| {
                m.status.probes = vec![streak("web", PROBE_READINESS, 2, 0, "")];
                Ok(())
            })
            .unwrap();
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert!(pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "Running");
        assert!(pod.status.container_statuses[0].ready);
    }

    #[tokio::test]
    async fn tcp_readiness_passes_once_the_streak_is_met() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", None, Some(tcp_probe(80, 1)))],
            vec![streak("web", PROBE_READINESS, 1, 0, "")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert!(pod.status.is_ready());
        assert!(pod.status.container_statuses[0].ready);
    }

    #[tokio::test]
    async fn exec_readiness_follows_the_success_threshold() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", None, Some(exec_probe_with(2, 3)))],
            vec![streak("web", PROBE_READINESS, 1, 0, "")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert!(!pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "ContainersNotReady");
        assert!(!pod.status.container_statuses[0].ready);

        fx.store
            .collection::<Machine>()
            .modify(&fx.machine, |m| {
                m.status.probes = vec![streak("web", PROBE_READINESS, 2, 0, "")];
                Ok(())
            })
            .unwrap();
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert!(pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "Running");
        assert!(pod.status.container_statuses[0].ready);
    }

    #[tokio::test]
    async fn exec_readiness_stays_not_ready_when_unsupported() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", None, Some(exec_probe()))],
            vec![unsupported_report("web", PROBE_READINESS)],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert!(!pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "ContainersNotReady");
        let message = &pod
            .status
            .conditions
            .iter()
            .find(|c| c.kind == POD_READY)
            .unwrap()
            .message;
        assert!(message.contains("not supported"), "{message}");
        assert!(!pod.status.container_statuses[0].ready);
    }

    #[tokio::test]
    async fn liveness_failure_restarts_only_that_container() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![
                ctr("web", Some(http_probe(8080, 1, 3)), None),
                ctr("sidecar", None, None),
            ],
            vec![streak("web", PROBE_LIVENESS, 0, 3, "http status 500")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Pending);
        assert!(!pod.status.is_ready());
        assert_eq!(reason(&pod, POD_READY), "Restarting");
        assert_eq!(reason(&pod, LIVENESS_CONDITION), "ProbeFailed");
        assert!(pod.status.message.contains("web"));
        let web = pod
            .status
            .container_statuses
            .iter()
            .find(|c| c.name == "web")
            .unwrap();
        let side = pod
            .status
            .container_statuses
            .iter()
            .find(|c| c.name == "sidecar")
            .unwrap();
        assert_eq!(web.restart_count, 1);
        assert_eq!(side.restart_count, 0);
        assert!(!web.ready);
        assert!(!side.ready);
        let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
        assert_eq!(machine.spec.restarts, 1);
    }

    #[tokio::test]
    async fn liveness_below_threshold_does_not_restart() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", Some(http_probe(8080, 1, 3)), None)],
            vec![streak("web", PROBE_LIVENESS, 0, 2, "connect: refused")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert!(pod.status.is_ready());
        assert_eq!(pod.status.container_statuses[0].restart_count, 0);
        assert_eq!(reason(&pod, LIVENESS_CONDITION), "Ok");
        let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
        assert_eq!(machine.spec.restarts, 0);
    }

    #[tokio::test]
    async fn liveness_failure_restarts_on_failure_policy() {
        let fx = running_pod(
            RestartPolicy::OnFailure,
            vec![ctr("web", Some(http_probe(8080, 1, 1)), None)],
            vec![streak("web", PROBE_LIVENESS, 0, 1, "timed out")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Pending);
        assert_eq!(pod.status.container_statuses[0].restart_count, 1);
        let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
        assert_eq!(machine.spec.restarts, 1);
    }

    #[tokio::test]
    async fn liveness_failure_does_not_restart_when_policy_is_never() {
        let fx = running_pod(
            RestartPolicy::Never,
            vec![ctr("web", Some(http_probe(8080, 1, 3)), None)],
            vec![streak("web", PROBE_LIVENESS, 0, 3, "http status 500")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert_eq!(pod.status.container_statuses[0].restart_count, 0);
        assert!(pod.status.container_statuses[0].ready);
        assert_eq!(reason(&pod, LIVENESS_CONDITION), "ProbeFailed");
        let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
        assert_eq!(machine.spec.restarts, 0);
    }

    #[tokio::test]
    async fn exec_liveness_failure_restarts_at_the_threshold() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", Some(exec_probe_with(1, 3)), None)],
            vec![streak("web", PROBE_LIVENESS, 0, 3, "exit 1")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Pending);
        assert_eq!(pod.status.container_statuses[0].restart_count, 1);
        assert_eq!(reason(&pod, LIVENESS_CONDITION), "ProbeFailed");
        let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
        assert_eq!(machine.spec.restarts, 1);
    }

    #[tokio::test]
    async fn exec_liveness_below_threshold_does_not_restart() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", Some(exec_probe_with(1, 3)), None)],
            vec![streak("web", PROBE_LIVENESS, 0, 2, "timed out")],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert_eq!(pod.status.container_statuses[0].restart_count, 0);
        assert_eq!(reason(&pod, LIVENESS_CONDITION), "Ok");
        let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
        assert_eq!(machine.spec.restarts, 0);
    }

    #[tokio::test]
    async fn exec_liveness_is_unsupported_and_does_not_restart() {
        let mut report = unsupported_report("web", PROBE_LIVENESS);
        report.failures = 9;
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", Some(exec_probe()), None)],
            vec![report],
            0,
            0,
            &[],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Running);
        assert_eq!(pod.status.container_statuses[0].restart_count, 0);
        assert_eq!(reason(&pod, LIVENESS_CONDITION), "Unsupported");
        let message = pod
            .status
            .conditions
            .iter()
            .find(|c| c.kind == LIVENESS_CONDITION)
            .unwrap();
        assert!(message.message.contains("not supported"));
        let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
        assert_eq!(machine.spec.restarts, 0);
    }

    #[tokio::test]
    async fn liveness_restart_in_flight_is_not_counted_twice() {
        let fx = running_pod(
            RestartPolicy::Always,
            vec![ctr("web", Some(http_probe(8080, 1, 3)), None)],
            vec![streak("web", PROBE_LIVENESS, 0, 9, "http status 500")],
            1,
            0,
            &[("web", 1)],
        );
        pass(&fx.store).await;
        let pod = load(&fx.store, &fx.pod);
        assert_eq!(pod.status.phase, PodPhase::Pending);
        assert_eq!(pod.status.container_statuses[0].restart_count, 1);
        let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
        assert_eq!(machine.spec.restarts, 1);
        assert_eq!(machine.status.restarts, 0);
    }

    fn put_volume(
        store: &Store,
        name: &str,
        phase: Phase,
        attached: bool,
        device: Option<&str>,
        backend_ref: Option<&str>,
        host: Option<&Id>,
    ) -> Id {
        let vol = store
            .collection::<super::SlabVolume>()
            .create(pistack_types::Resource::new(
                name,
                None,
                super::SlabVolSpec { size_gib: 1 },
            ))
            .unwrap();
        store
            .collection::<super::SlabVolume>()
            .modify(&vol.meta.id, |v| {
                v.status.phase = phase;
                if attached {
                    v.status.attached_to = Some(Id::new(kind::MACHINE));
                    v.status.conditions.push(super::SlabCondition {
                        kind: "Attached".into(),
                        status: true,
                    });
                }
                v.status.device = device.map(str::to_string);
                v.status.backend_ref = backend_ref.map(str::to_string);
                v.status.host = host.map(|id| id.to_string());
                Ok(())
            })
            .unwrap();
        vol.meta.id
    }

    fn pod_with_volume(store: &Store, vol: Id) -> Id {
        let pod = Pod::new(
            "web",
            None,
            PodSpec {
                containers: vec![ctr("web", None, None)],
                volumes: vec![PodVolume {
                    name: "data".into(),
                    source: VolumeSource::Volume(vol),
                }],
                networks: vec!["default".into()],
                ..PodSpec::default()
            },
        );
        store.collection::<Pod>().create(pod).unwrap().meta.id
    }

    #[tokio::test]
    async fn pod_waits_until_the_slab_volume_is_attached() {
        let store = Store::in_memory().unwrap();
        let vol = put_volume(&store, "data", Phase::Pending, false, None, None, None);
        let pod = pod_with_volume(&store, vol.clone());
        pass(&store).await;
        let got = load(&store, &pod);
        assert_eq!(got.status.phase, PodPhase::Pending);
        assert!(
            got.status.message.contains("data"),
            "{}",
            got.status.message
        );
        assert!(
            got.status.message.contains("waiting for volume"),
            "{}",
            got.status.message
        );
        assert!(store.collection::<Machine>().list(None).unwrap().is_empty());

        store
            .collection::<super::SlabVolume>()
            .modify(&vol, |v| {
                v.status.phase = Phase::Ready;
                v.status.attached_to = Some(Id::new(kind::MACHINE));
                v.status.conditions.push(super::SlabCondition {
                    kind: "Attached".into(),
                    status: true,
                });
                Ok(())
            })
            .unwrap();
        pass(&store).await;
        let got = load(&store, &pod);
        assert!(
            got.status.message.contains("data"),
            "{}",
            got.status.message
        );
        assert!(store.collection::<Machine>().list(None).unwrap().is_empty());

        let host = Id::new(kind::HOST);
        let path = "/var/lib/pistack/volumes/data";
        store
            .collection::<super::SlabVolume>()
            .modify(&vol, |v| {
                v.status.backend_ref = Some(path.into());
                v.status.host = Some(host.to_string());
                Ok(())
            })
            .unwrap();
        pass(&store).await;
        let machines = store.collection::<Machine>().list(None).unwrap();
        assert_eq!(machines.len(), 1);
        let machine = &machines[0];
        let pin = host.to_string();
        assert_eq!(
            machine
                .meta
                .annotations
                .get(forge::NODE_PIN_ANNOTATION)
                .map(String::as_str),
            Some(pin.as_str())
        );
        assert!(machine
            .meta
            .annotations
            .get(super::MOUNTED_VOLUMES)
            .unwrap()
            .contains(&vol.to_string()));
        let sandbox: PodSandbox =
            serde_json::from_str(machine.spec.user_data.as_deref().unwrap()).unwrap();
        assert_eq!(
            sandbox
                .volume_paths
                .get(&vol.to_string())
                .map(String::as_str),
            Some(path)
        );
        assert!(!sandbox.volume_devices.contains_key(&vol.to_string()));
    }

    #[tokio::test]
    async fn device_only_slab_volume_starts_without_a_host_path() {
        let store = Store::in_memory().unwrap();
        let host = Id::new(kind::HOST);
        let vol = put_volume(
            &store,
            "data",
            Phase::Ready,
            true,
            Some("vdb"),
            Some("rbd-image"),
            Some(&host),
        );
        let pod = pod_with_volume(&store, vol.clone());
        pass(&store).await;
        let machines = store.collection::<Machine>().list(None).unwrap();
        assert_eq!(machines.len(), 1);
        let sandbox: PodSandbox =
            serde_json::from_str(machines[0].spec.user_data.as_deref().unwrap()).unwrap();
        assert_eq!(
            sandbox
                .volume_devices
                .get(&vol.to_string())
                .map(String::as_str),
            Some("vdb")
        );
        assert!(sandbox.volume_paths.is_empty());
        let got = load(&store, &pod);
        assert_eq!(got.status.machine.as_ref(), Some(&machines[0].meta.id));
    }

    #[tokio::test]
    async fn late_slab_attach_recreates_a_running_pod() {
        let store = Store::in_memory().unwrap();
        let host = Id::new(kind::HOST);
        let vol = put_volume(
            &store,
            "data",
            Phase::Ready,
            true,
            Some("vdb"),
            Some("/var/lib/pistack/volumes/data"),
            Some(&host),
        );
        let pod_id = pod_with_volume(&store, vol.clone());
        let mut machine = new_machine(
            "pod-web",
            None,
            MachineSpec {
                kind: MachineKind::Container,
                size: "small".into(),
                image: "img".into(),
                networks: vec!["default".into()],
                user_data: Some("{}".into()),
                ..MachineSpec::default()
            },
        );
        machine
            .meta
            .labels
            .insert(MACHINE_POD_LABEL.into(), pod_id.to_string());
        machine.status.phase = Phase::Ready;
        machine.status.power = PowerState::On;
        machine.status.host = Some(host);
        let machine = store.collection::<Machine>().create(machine).unwrap();
        pass(&store).await;
        let pod = load(&store, &pod_id);
        assert_eq!(pod.status.phase, PodPhase::Pending);
        assert!(
            pod.status.message.contains("data"),
            "{}",
            pod.status.message
        );
        assert!(!pod.status.is_ready());
        let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
        assert_eq!(machine.spec.restarts, 1);
        let sandbox: PodSandbox =
            serde_json::from_str(machine.spec.user_data.as_deref().unwrap()).unwrap();
        assert_eq!(
            sandbox
                .volume_paths
                .get(&vol.to_string())
                .map(String::as_str),
            Some("/var/lib/pistack/volumes/data")
        );
        pass(&store).await;
        let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
        assert_eq!(machine.spec.restarts, 1);
    }

    #[tokio::test]
    async fn slab_volume_on_another_node_is_not_accepted() {
        let store = Store::in_memory().unwrap();
        let here = Id::new(kind::HOST);
        let there = Id::new(kind::HOST);
        let vol = put_volume(
            &store,
            "data",
            Phase::Ready,
            true,
            None,
            Some("/var/lib/pistack/volumes/data"),
            Some(&there),
        );
        let pod_id = pod_with_volume(&store, vol);
        let mut machine = new_machine(
            "pod-web",
            None,
            MachineSpec {
                kind: MachineKind::Container,
                size: "small".into(),
                image: "img".into(),
                networks: vec!["default".into()],
                ..MachineSpec::default()
            },
        );
        machine
            .meta
            .labels
            .insert(MACHINE_POD_LABEL.into(), pod_id.to_string());
        machine.status.phase = Phase::Ready;
        machine.status.power = PowerState::On;
        machine.status.host = Some(here);
        let machine = store.collection::<Machine>().create(machine).unwrap();
        pass(&store).await;
        let pod = load(&store, &pod_id);
        assert_eq!(pod.status.phase, PodPhase::Pending);
        assert!(
            pod.status.message.contains("not attached on this node"),
            "{}",
            pod.status.message
        );
        assert!(
            pod.status.message.contains("data"),
            "{}",
            pod.status.message
        );
        let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
        assert_eq!(machine.spec.restarts, 0);
    }

    #[tokio::test]
    async fn existing_machine_waits_until_the_volume_has_a_path() {
        let store = Store::in_memory().unwrap();
        let vol = put_volume(&store, "data", Phase::Pending, false, None, None, None);
        let pod_id = pod_with_volume(&store, vol.clone());
        let mut machine = new_machine(
            "pod-web",
            None,
            MachineSpec {
                kind: MachineKind::Container,
                size: "small".into(),
                image: "img".into(),
                networks: vec!["default".into()],
                ..MachineSpec::default()
            },
        );
        machine
            .meta
            .labels
            .insert(MACHINE_POD_LABEL.into(), pod_id.to_string());
        let machine = store.collection::<Machine>().create(machine).unwrap();
        pass(&store).await;
        let pod = load(&store, &pod_id);
        assert!(
            pod.status.message.contains("waiting for volume data"),
            "{}",
            pod.status.message
        );
        let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
        assert_eq!(
            machine
                .meta
                .annotations
                .get(forge::WAIT_VOLUME_ANNOTATION)
                .map(String::as_str),
            Some("waiting for volume data")
        );
        assert_eq!(machine.spec.restarts, 0);

        let host = Id::new(kind::HOST);
        store
            .collection::<super::SlabVolume>()
            .modify(&vol, |v| {
                v.status.phase = Phase::Ready;
                v.status.attached_to = Some(machine.meta.id.clone());
                v.status.backend_ref = Some("/var/lib/pistack/volumes/data".into());
                v.status.host = Some(host.to_string());
                v.status.conditions.push(super::SlabCondition {
                    kind: "Attached".into(),
                    status: true,
                });
                Ok(())
            })
            .unwrap();
        pass(&store).await;
        let machine = store.collection::<Machine>().get(&machine.meta.id).unwrap();
        assert!(!machine
            .meta
            .annotations
            .contains_key(forge::WAIT_VOLUME_ANNOTATION));
        assert_eq!(machine.spec.restarts, 0);
        let sandbox: PodSandbox =
            serde_json::from_str(machine.spec.user_data.as_deref().unwrap()).unwrap();
        assert_eq!(
            sandbox
                .volume_paths
                .get(&vol.to_string())
                .map(String::as_str),
            Some("/var/lib/pistack/volumes/data")
        );
        let pin = host.to_string();
        assert_eq!(
            machine
                .meta
                .annotations
                .get(forge::NODE_PIN_ANNOTATION)
                .map(String::as_str),
            Some(pin.as_str())
        );
    }
}

#[cfg(test)]
mod network_tests {
    use super::{pick_networks, DEFAULT_NETWORK};
    use crate::types::{Pod, PodSpec};
    use pistack_store::Store;
    use pistack_types::{kind, Id, Meta};
    use std::sync::Arc;

    fn net(store: &Store, name: &str, project: Option<Id>, external: bool) {
        fabric::Client::new(store.clone(), Arc::new(fabric::MockBackend::new()))
            .create_network(
                Meta::new(kind::NETWORK, name, project),
                fabric::NetworkSpec {
                    external,
                    ..fabric::NetworkSpec::default()
                },
            )
            .unwrap();
    }

    fn pod(project: &Id, networks: &[&str]) -> Pod {
        Pod::new(
            "web",
            Some(project.clone()),
            PodSpec {
                networks: networks.iter().map(|n| n.to_string()).collect(),
                ..PodSpec::default()
            },
        )
    }

    #[test]
    fn named_networks_win() {
        let store = Store::in_memory().unwrap();
        let p = Id::new(kind::PROJECT);
        assert_eq!(
            pick_networks(&store, &pod(&p, &["labnet"])).unwrap(),
            vec!["labnet"]
        );
    }

    #[test]
    fn default_by_name_then_the_only_internal_one() {
        let store = Store::in_memory().unwrap();
        let p = Id::new(kind::PROJECT);
        net(&store, "labext", Some(p.clone()), true);
        net(&store, "labnet", Some(p.clone()), false);
        net(&store, "theirs", Some(Id::new(kind::PROJECT)), false);
        assert_eq!(
            pick_networks(&store, &pod(&p, &[])).unwrap(),
            vec!["labnet"]
        );
        net(&store, DEFAULT_NETWORK, None, false);
        assert_eq!(
            pick_networks(&store, &pod(&p, &[])).unwrap(),
            vec![DEFAULT_NETWORK]
        );
    }

    #[test]
    fn none_or_several_fail_clearly() {
        let store = Store::in_memory().unwrap();
        let p = Id::new(kind::PROJECT);
        net(&store, "labext", Some(p.clone()), true);
        let none = pick_networks(&store, &pod(&p, &[])).unwrap_err();
        assert!(none.contains("no network that is not external"), "{none}");
        net(&store, "labnet", Some(p.clone()), false);
        net(&store, "app", Some(p.clone()), false);
        let several = pick_networks(&store, &pod(&p, &[])).unwrap_err();
        assert!(several.contains("(app, labnet)"), "{several}");
        assert!(several.contains("spec.networks"), "{several}");
    }
}
