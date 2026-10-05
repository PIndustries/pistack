// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The resources `loom` owns.
//!
//! Every one is a plain `Resource<Spec, Status>` from `pistack-types` with
//! the spec type implementing [`Spec`], so the store's `Collection` API
//! works unchanged. The shapes follow Kubernetes field for field where that
//! costs nothing (metadata + spec + status, labels, selectors, conditions,
//! `pod-template-hash`) so a k8s-API facade can be bolted on later without
//! a translation layer.
//!
//! `spec` is the user's; `status` is written only by the controllers in
//! [`crate::controllers`].

use chrono::{DateTime, Utc};
use pistack_types::{Condition, Id, Resource, Resources, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- kinds and well-known keys ------------------------------------------------

/// Id prefixes / store tables.
pub mod kinds {
    pub const POD: &str = pistack_types::kind::POD;
    pub const REPLICA_SET: &str = "rs";
    pub const DEPLOYMENT: &str = "deploy";
    pub const STATEFUL_SET: &str = "sts";
    pub const DAEMON_SET: &str = "ds";
    pub const SERVICE: &str = "svc";
    pub const CONFIG_MAP: &str = "cm";
    pub const JOB: &str = "job";
    pub const CRON_JOB: &str = "cron";
    pub const INGRESS: &str = "ing";
    pub const AUTOSCALER: &str = "scaler";
}

/// Annotation a DELETE handler sets to ask a controller to tear the
/// resource down (the Kubernetes `deletionTimestamp` idea). Same key as
/// forge uses on machines so one convention covers the cloud.
pub const DELETE_ANNOTATION: &str = forge::DELETE_ANNOTATION;

/// Label on a pod naming the ReplicaSet/Job that owns it, and on a
/// ReplicaSet/Job naming the Deployment/CronJob that owns it. Value is the
/// owner's id. (Kubernetes uses `ownerReferences`; a label keeps it
/// selectable.)
pub const OWNER_LABEL: &str = "loom.pistack.io/owner";

/// Label the Deployment controller stamps on every ReplicaSet it creates and
/// on the pods under it. Same key as Kubernetes.
pub const POD_TEMPLATE_HASH: &str = "pod-template-hash";

/// Label on the forge `Machine` backing a pod. Value is the pod id.
pub const MACHINE_POD_LABEL: &str = "loom.pistack.io/pod";

/// Annotation the Deployment controller keeps on each ReplicaSet with its
/// monotonically increasing rollout revision (`1`, `2`, ...).
pub const REVISION_ANNOTATION: &str = "loom.pistack.io/revision";

/// Stable pod name for a StatefulSet ordinal (`web-0`).
pub const POD_NAME_LABEL: &str = "loom.pistack.io/pod-name";

/// Ordinal index of a StatefulSet pod (`0`, `1`, ...).
pub const POD_ORDINAL_LABEL: &str = "loom.pistack.io/pod-ordinal";

/// Host `node_name` stamped on forge hosts so a DaemonSet can pin one pod
/// per host through `pod.spec.node_selector`.
pub const HOSTNAME_LABEL: &str = "pistack.io/hostname";

/// Annotation on a DaemonSet pod naming the forge host id it belongs to.
pub const TARGET_HOST_ANNOTATION: &str = "loom.pistack.io/target-host";

// ---- selectors -----------------------------------------------------------------

/// A Kubernetes-shaped label selector: every `match_labels` pair must match
/// and every expression must hold.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Selector {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub match_labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_expressions: Vec<SelectorRequirement>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectorRequirement {
    pub key: String,
    pub operator: SelectorOperator,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum SelectorOperator {
    In,
    NotIn,
    Exists,
    DoesNotExist,
}

// ---- Pod -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EnvVar {
    pub name: String,
    #[serde(default)]
    pub value: String,
    /// Pull the value from a ConfigMap or Secret key (`{config_map|secret, name, key}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_from: Option<EnvVarSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvVarSource {
    ConfigMap { name: String, key: String },
    Secret { name: String, key: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ContainerPort {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub container_port: u16,
    #[serde(default)]
    pub protocol: Protocol,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "UPPERCASE")]
pub enum Protocol {
    #[default]
    Tcp,
    Udp,
    Sctp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VolumeMount {
    pub name: String,
    pub mount_path: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Container {
    pub name: String,
    pub image: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<ContainerPort>,
    /// Requested resources; the pod's size is the sum over its containers.
    #[serde(default)]
    pub resources: Resources,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_mounts: Vec<VolumeMount>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    /// Checked on the host that runs the pod. Failure past
    /// `failure_threshold` restarts the container when the restart policy
    /// allows it. An `exec` handler is stored and reported unsupported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness_probe: Option<Probe>,
    /// While this probe has not passed, the container is not ready and the
    /// pod's Ready condition stays false. Absent means the container is
    /// ready once it is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness_probe: Option<Probe>,
}

/// One check against a container. Exactly one of `http_get`, `tcp_socket`,
/// or `exec` is set. Zero `period_seconds`, `timeout_seconds`,
/// `success_threshold`, or `failure_threshold` means the default.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Probe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_get: Option<HttpGetProbe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_socket: Option<TcpSocketProbe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecProbe>,
    #[serde(default)]
    pub initial_delay_seconds: u32,
    #[serde(default = "default_period")]
    pub period_seconds: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    #[serde(default = "default_success")]
    pub success_threshold: u32,
    #[serde(default = "default_failure")]
    pub failure_threshold: u32,
}

fn default_period() -> u32 {
    10
}
fn default_timeout() -> u32 {
    1
}
fn default_success() -> u32 {
    1
}
fn default_failure() -> u32 {
    3
}

fn nonzero(value: u32, default: u32) -> u32 {
    if value == 0 {
        default
    } else {
        value
    }
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            http_get: None,
            tcp_socket: None,
            exec: None,
            initial_delay_seconds: 0,
            period_seconds: default_period(),
            timeout_seconds: default_timeout(),
            success_threshold: default_success(),
            failure_threshold: default_failure(),
        }
    }
}

impl Probe {
    pub fn period(&self) -> u32 {
        nonzero(self.period_seconds, default_period())
    }
    pub fn timeout(&self) -> u32 {
        nonzero(self.timeout_seconds, default_timeout())
    }
    pub fn successes_required(&self) -> u32 {
        nonzero(self.success_threshold, default_success())
    }
    pub fn failures_required(&self) -> u32 {
        nonzero(self.failure_threshold, default_failure())
    }
    pub fn handler(&self) -> ProbeHandlerKind {
        if self.http_get.is_some() {
            ProbeHandlerKind::Http
        } else if self.tcp_socket.is_some() {
            ProbeHandlerKind::Tcp
        } else if self.exec.is_some() {
            ProbeHandlerKind::Exec
        } else {
            ProbeHandlerKind::Missing
        }
    }
}

/// Which action a [`Probe`] will run. `Exec` is accepted on the spec and
/// reported unsupported until a later change can run a command in the
/// container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeHandlerKind {
    Http,
    Tcp,
    Exec,
    Missing,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpGetProbe {
    #[serde(default = "default_probe_path")]
    pub path: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<ProbeScheme>,
}

fn default_probe_path() -> String {
    "/".into()
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum ProbeScheme {
    #[default]
    #[serde(alias = "http")]
    Http,
    #[serde(alias = "https")]
    Https,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TcpSocketProbe {
    pub port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecProbe {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeSource {
    /// Scratch space that lives as long as the pod.
    EmptyDir,
    /// A `keep` secret, mounted as files.
    Secret(String),
    /// A ConfigMap (name or id), mounted as files.
    ConfigMap(String),
    /// A `slab` block volume, attached to the pod's machine.
    Volume(Id),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodVolume {
    pub name: String,
    pub source: VolumeSource,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum RestartPolicy {
    #[default]
    Always,
    OnFailure,
    Never,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PodSpec {
    pub containers: Vec<Container>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<PodVolume>,
    #[serde(default)]
    pub restart_policy: RestartPolicy,
    /// Every pair must match a host label (forge `host_selector`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_selector: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service_account: String,
    /// fabric networks to attach. Empty means `["default"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
}

impl Spec for PodSpec {
    const KIND: &'static str = kinds::POD;
}

impl PodSpec {
    /// Sum of the containers' requests: what the pod's machine must offer.
    pub fn resources(&self) -> Resources {
        self.containers
            .iter()
            .fold(Resources::default(), |acc, c| acc.add(&c.resources))
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum PodPhase {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
    Unknown,
}

impl PodPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, PodPhase::Succeeded | PodPhase::Failed)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    Waiting {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
    Running {
        started_at: DateTime<Utc>,
    },
    Terminated {
        exit_code: i32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
        finished_at: DateTime<Utc>,
    },
}

impl Default for ContainerState {
    fn default() -> Self {
        ContainerState::Waiting {
            reason: "Pending".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ContainerStatus {
    pub name: String,
    pub image: String,
    pub ready: bool,
    #[serde(default)]
    pub restart_count: u32,
    #[serde(default)]
    pub state: ContainerState,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PodStatus {
    #[serde(default)]
    pub phase: PodPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_ip: Option<String>,
    /// forge host id the backing machine runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Id>,
    /// The forge machine backing this pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<Id>,
    #[serde(default)]
    pub container_statuses: Vec<ContainerStatus>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,
}

pub type Pod = Resource<PodSpec, PodStatus>;

/// Condition kind [`PodStatus::is_ready`] reads.
pub const POD_READY: &str = "Ready";

impl PodStatus {
    /// Whether controllers should treat this pod as ready to serve.
    ///
    /// True when the [`POD_READY`] condition is present and true. A pod
    /// with no readiness probe reaches that once every container is
    /// running. A readiness probe holds the condition false until the
    /// probe has passed.
    pub fn is_ready(&self) -> bool {
        self.conditions
            .iter()
            .any(|c| c.kind == POD_READY && c.status)
    }
}

/// A pod template: the metadata a controller stamps on each pod plus its spec.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PodTemplate {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    pub spec: PodSpec,
}

// ---- ReplicaSet ------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicaSetSpec {
    #[serde(default = "one")]
    pub replicas: u32,
    #[serde(default)]
    pub selector: Selector,
    pub template: PodTemplate,
}

fn one() -> u32 {
    1
}

impl Spec for ReplicaSetSpec {
    const KIND: &'static str = kinds::REPLICA_SET;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ReplicaSetStatus {
    /// Pods owned and not being deleted.
    #[serde(default)]
    pub replicas: u32,
    #[serde(default)]
    pub ready: u32,
    #[serde(default)]
    pub available: u32,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

pub type ReplicaSet = Resource<ReplicaSetSpec, ReplicaSetStatus>;

// ---- Deployment ------------------------------------------------------------------

/// `3` or `"25%"`, as in Kubernetes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum IntOrPercent {
    Int(u32),
    Percent(String),
}

impl IntOrPercent {
    /// Resolve against `total`. Percent strings are `"<n>%"`; anything else
    /// is treated as 0.
    pub fn resolve(&self, total: u32, round_up: bool) -> u32 {
        match self {
            IntOrPercent::Int(n) => *n,
            IntOrPercent::Percent(s) => {
                let pct: u64 = s
                    .trim()
                    .strip_suffix('%')
                    .and_then(|n| n.trim().parse().ok())
                    .unwrap_or(0);
                let num = pct * total as u64;
                let v = if round_up {
                    num.div_ceil(100)
                } else {
                    num / 100
                };
                v.min(u32::MAX as u64) as u32
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum DeploymentStrategy {
    RollingUpdate {
        #[serde(default = "quarter")]
        max_surge: IntOrPercent,
        #[serde(default = "quarter")]
        max_unavailable: IntOrPercent,
    },
    Recreate,
}

fn quarter() -> IntOrPercent {
    IntOrPercent::Percent("25%".into())
}

impl Default for DeploymentStrategy {
    fn default() -> Self {
        DeploymentStrategy::RollingUpdate {
            max_surge: quarter(),
            max_unavailable: quarter(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeploymentSpec {
    #[serde(default = "one")]
    pub replicas: u32,
    /// Defaults to the template labels when empty.
    #[serde(default)]
    pub selector: Selector,
    pub template: PodTemplate,
    #[serde(default)]
    pub strategy: DeploymentStrategy,
    /// Old ReplicaSets to keep for rollback.
    #[serde(default = "three")]
    pub revision_history_limit: u32,
    #[serde(default)]
    pub paused: bool,
}

fn three() -> u32 {
    3
}

impl Spec for DeploymentSpec {
    const KIND: &'static str = kinds::DEPLOYMENT;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DeploymentStatus {
    #[serde(default)]
    pub replicas: u32,
    #[serde(default)]
    pub updated: u32,
    #[serde(default)]
    pub ready: u32,
    #[serde(default)]
    pub available: u32,
    #[serde(default)]
    pub unavailable: u32,
    /// Current rollout revision (see `REVISION_ANNOTATION`).
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

pub type Deployment = Resource<DeploymentSpec, DeploymentStatus>;

// ---- Service ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum ServiceKind {
    #[default]
    ClusterIp,
    NodePort,
    LoadBalancer,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ServicePort {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub protocol: Protocol,
    pub port: u16,
    /// Container port to send to; defaults to `port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
    /// `NodePort` and `LoadBalancer` only: ask for this node port
    /// (30000-32767) instead of the next free one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_port: Option<u16>,
}

/// Node ports a service may hold.
pub const NODE_PORT_RANGE: std::ops::RangeInclusive<u16> = 30000..=32767;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ServiceSpec {
    #[serde(default)]
    pub selector: BTreeMap<String, String>,
    #[serde(default)]
    pub ports: Vec<ServicePort>,
    #[serde(default)]
    pub kind: ServiceKind,
    /// Ask for a specific cluster IP instead of the next free one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_ip: Option<String>,
}

impl Spec for ServiceSpec {
    const KIND: &'static str = kinds::SERVICE;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub ip: String,
    pub pod: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Id>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ServiceStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_ip: Option<String>,
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    /// External addresses for `LoadBalancer` services, filled by the realizer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_balancer: Vec<String>,
    /// Node port held for each service port, keyed by `port`. Set for
    /// `NodePort` and `LoadBalancer` services: `spec` asks for one, or the
    /// controller takes the lowest free port in 30000-32767.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_ports: BTreeMap<u16, u16>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

pub type Service = Resource<ServiceSpec, ServiceStatus>;

// ---- ConfigMap -------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConfigMapSpec {
    #[serde(default)]
    pub data: BTreeMap<String, String>,
    /// base64-encoded values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binary_data: BTreeMap<String, String>,
    #[serde(default)]
    pub immutable: bool,
}

impl Spec for ConfigMapSpec {
    const KIND: &'static str = kinds::CONFIG_MAP;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConfigMapStatus {}

pub type ConfigMap = Resource<ConfigMapSpec, ConfigMapStatus>;

// ---- Job -------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobSpec {
    pub template: PodTemplate,
    #[serde(default = "one")]
    pub completions: u32,
    #[serde(default = "one")]
    pub parallelism: u32,
    #[serde(default = "six")]
    pub backoff_limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_deadline_seconds: Option<u64>,
    /// Delete the job (and its pods) this long after it finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds_after_finished: Option<u64>,
    #[serde(default)]
    pub suspend: bool,
}

fn six() -> u32 {
    6
}

impl Spec for JobSpec {
    const KIND: &'static str = kinds::JOB;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct JobStatus {
    #[serde(default)]
    pub active: u32,
    #[serde(default)]
    pub succeeded: u32,
    #[serde(default)]
    pub failed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_time: Option<DateTime<Utc>>,
    /// Earliest time a replacement pod may be created after a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry: Option<DateTime<Utc>>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

impl JobStatus {
    pub fn has_condition(&self, kind: &str) -> bool {
        self.conditions.iter().any(|c| c.kind == kind && c.status)
    }
    pub fn is_finished(&self) -> bool {
        self.has_condition("Complete") || self.has_condition("Failed")
    }
}

pub type Job = Resource<JobSpec, JobStatus>;

// ---- CronJob ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum ConcurrencyPolicy {
    #[default]
    Allow,
    Forbid,
    Replace,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronJobSpec {
    /// Five-field cron expression, UTC. See [`crate::cron`].
    pub schedule: String,
    pub job_template: JobSpec,
    #[serde(default)]
    pub suspend: bool,
    #[serde(default)]
    pub concurrency_policy: ConcurrencyPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_deadline_seconds: Option<u64>,
    #[serde(default = "three")]
    pub successful_jobs_history_limit: u32,
    #[serde(default = "one")]
    pub failed_jobs_history_limit: u32,
}

impl Spec for CronJobSpec {
    const KIND: &'static str = kinds::CRON_JOB;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CronJobStatus {
    #[serde(default)]
    pub active: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_schedule_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_successful_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_schedule_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

pub type CronJob = Resource<CronJobSpec, CronJobStatus>;

// ---- StatefulSet ----------------------------------------------------------------

/// How a StatefulSet creates and deletes pods.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum PodManagementPolicy {
    /// Create/delete one ordinal at a time, waiting for Ready.
    #[default]
    OrderedReady,
    /// Create/delete every ordinal in one pass.
    Parallel,
}

/// How a StatefulSet rolls a new pod template.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum StatefulSetUpdateStrategy {
    /// Replace pods that do not match the current template, highest ordinal first.
    #[default]
    RollingUpdate,
    /// Leave existing pods alone until they are deleted by hand or scaled away.
    OnDelete,
}

/// A per-ordinal block volume to create in `slab` and attach to the pod.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeClaimTemplate {
    pub name: String,
    pub size_gib: u64,
    #[serde(default = "default_volume_type")]
    pub volume_type: String,
}

fn default_volume_type() -> String {
    "standard".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatefulSetSpec {
    #[serde(default = "one")]
    pub replicas: u32,
    /// Defaults to the template labels when empty.
    #[serde(default)]
    pub selector: Selector,
    pub template: PodTemplate,
    /// Name of the Service that gives pods a stable network identity.
    /// Empty when unused; loom does not invent DNS records here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_claim_templates: Vec<VolumeClaimTemplate>,
    #[serde(default)]
    pub pod_management_policy: PodManagementPolicy,
    #[serde(default)]
    pub update_strategy: StatefulSetUpdateStrategy,
}

impl Spec for StatefulSetSpec {
    const KIND: &'static str = kinds::STATEFUL_SET;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StatefulSetStatus {
    #[serde(default)]
    pub replicas: u32,
    #[serde(default)]
    pub ready: u32,
    /// Pods whose template hash matches the current spec.
    #[serde(default)]
    pub current: u32,
    #[serde(default)]
    pub updated: u32,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

pub type StatefulSet = Resource<StatefulSetSpec, StatefulSetStatus>;

// ---- DaemonSet -------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonSetSpec {
    /// Defaults to the template labels when empty.
    #[serde(default)]
    pub selector: Selector,
    pub template: PodTemplate,
}

impl Spec for DaemonSetSpec {
    const KIND: &'static str = kinds::DAEMON_SET;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DaemonSetStatus {
    /// Eligible hosts the controller wants a pod on.
    #[serde(default)]
    pub desired: u32,
    /// Owned pods that are not being deleted.
    #[serde(default)]
    pub current: u32,
    #[serde(default)]
    pub ready: u32,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

pub type DaemonSet = Resource<DaemonSetSpec, DaemonSetStatus>;

// ---- Ingress ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum PathType {
    #[default]
    Prefix,
    Exact,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngressBackend {
    /// Service name or id.
    pub service: String,
    /// Service port number or name.
    pub port: IntOrName,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum IntOrName {
    Int(u16),
    Name(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngressPath {
    #[serde(default = "root")]
    pub path: String,
    #[serde(default)]
    pub path_type: PathType,
    pub backend: IngressBackend,
}

fn root() -> String {
    "/".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IngressRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default)]
    pub paths: Vec<IngressPath>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IngressTls {
    #[serde(default)]
    pub hosts: Vec<String>,
    /// A `keep` certificate/secret name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IngressSpec {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ingress_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_backend: Option<IngressBackend>,
    #[serde(default)]
    pub rules: Vec<IngressRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tls: Vec<IngressTls>,
}

impl Spec for IngressSpec {
    const KIND: &'static str = kinds::INGRESS;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IngressStatus {
    /// Addresses `relay` exposes the ingress on. Empty until relay realizes it.
    #[serde(default)]
    pub load_balancer: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

pub type Ingress = Resource<IngressSpec, IngressStatus>;

// ---- Autoscaler ------------------------------------------------------------------

/// Workload an autoscaler may adjust.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ScaleTargetKind {
    Deployment,
    StatefulSet,
}

/// Scale a Deployment or StatefulSet from a gauge metric.
///
/// `desired = ceil(current * metric / target_value)`, clamped to
/// `[min_replicas, max_replicas]`. When `current` is zero the ratio uses
/// one replica as the base so a high metric can still scale out.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AutoscalerSpec {
    pub target_kind: ScaleTargetKind,
    pub target_name: String,
    pub metric: String,
    pub target_value: f64,
    #[serde(default = "one")]
    pub min_replicas: u32,
    pub max_replicas: u32,
}

impl Spec for AutoscalerSpec {
    const KIND: &'static str = kinds::AUTOSCALER;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AutoscalerStatus {
    #[serde(default)]
    pub desired_replicas: u32,
    #[serde(default)]
    pub current_replicas: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_metric: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

pub type Autoscaler = Resource<AutoscalerSpec, AutoscalerStatus>;

/// Desired replica count from the latest metric sample.
pub fn desired_replicas(
    current: u32,
    metric_value: f64,
    target_value: f64,
    min_replicas: u32,
    max_replicas: u32,
) -> u32 {
    let lo = min_replicas;
    let hi = max_replicas.max(lo);
    if !metric_value.is_finite() || !target_value.is_finite() || target_value <= 0.0 {
        return current.clamp(lo, hi);
    }
    let base = if current == 0 { 1 } else { current };
    let raw = (base as f64) * metric_value / target_value;
    let desired = if raw.is_finite() {
        raw.ceil().max(0.0) as u32
    } else {
        current
    };
    desired.clamp(lo, hi)
}

// ---- helpers ---------------------------------------------------------------------

/// Set a condition on a list, replacing one of the same type; bumps
/// `last_transition` only when `status` or `reason` changed.
pub fn set_condition(
    conditions: &mut Vec<Condition>,
    kind: &str,
    status: bool,
    reason: &str,
    message: &str,
) {
    if let Some(c) = conditions.iter_mut().find(|c| c.kind == kind) {
        if c.status != status || c.reason != reason {
            c.last_transition = Utc::now();
        }
        c.status = status;
        c.reason = reason.to_owned();
        c.message = message.to_owned();
        return;
    }
    conditions.push(Condition {
        kind: kind.to_owned(),
        status,
        reason: reason.to_owned(),
        message: message.to_owned(),
        last_transition: Utc::now(),
    });
}

/// Whether a delete has been requested (see `DELETE_ANNOTATION`).
pub fn delete_requested<S, T>(r: &Resource<S, T>) -> bool {
    r.meta.annotations.contains_key(DELETE_ANNOTATION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_without_probes_still_decodes() {
        let container: Container = serde_json::from_str(r#"{"name":"web","image":"img"}"#).unwrap();
        assert!(container.liveness_probe.is_none());
        assert!(container.readiness_probe.is_none());
        let encoded = serde_json::to_value(&container).unwrap();
        assert!(encoded.get("liveness_probe").is_none());
        assert!(encoded.get("readiness_probe").is_none());
    }

    #[test]
    fn probe_defaults_fill_omitted_fields_and_zero_means_default() {
        let probe: Probe =
            serde_json::from_str(r#"{"http_get":{"path":"/health","port":8080}}"#).unwrap();
        assert_eq!(probe.initial_delay_seconds, 0);
        assert_eq!(probe.period_seconds, 10);
        assert_eq!(probe.timeout_seconds, 1);
        assert_eq!(probe.success_threshold, 1);
        assert_eq!(probe.failure_threshold, 3);
        assert_eq!(probe.http_get.as_ref().unwrap().path, "/health");
        assert_eq!(probe.handler(), ProbeHandlerKind::Http);

        let tcp: Probe = serde_json::from_str(r#"{"tcp_socket":{"port":80}}"#).unwrap();
        assert_eq!(tcp.handler(), ProbeHandlerKind::Tcp);
        assert_eq!(tcp.period(), 10);

        let zero = Probe {
            period_seconds: 0,
            timeout_seconds: 0,
            success_threshold: 0,
            failure_threshold: 0,
            ..Probe::default()
        };
        assert_eq!(zero.period(), 10);
        assert_eq!(zero.timeout(), 1);
        assert_eq!(zero.successes_required(), 1);
        assert_eq!(zero.failures_required(), 3);

        let exec: Probe = serde_json::from_str(r#"{"exec":{"command":["/bin/true"]}}"#).unwrap();
        assert_eq!(exec.handler(), ProbeHandlerKind::Exec);
        assert_eq!(exec.exec.unwrap().command, vec!["/bin/true".to_string()]);

        let scheme: HttpGetProbe =
            serde_json::from_str(r#"{"path":"/","port":443,"scheme":"https"}"#).unwrap();
        assert_eq!(scheme.scheme, Some(ProbeScheme::Https));
    }

    #[test]
    fn is_ready_reads_the_ready_condition() {
        let mut status = PodStatus::default();
        assert!(!status.is_ready());
        set_condition(&mut status.conditions, POD_READY, false, "Starting", "");
        assert!(!status.is_ready());
        set_condition(&mut status.conditions, POD_READY, true, "Running", "");
        assert!(status.is_ready());
        set_condition(
            &mut status.conditions,
            "PodScheduled",
            true,
            "Scheduled",
            "",
        );
        assert!(status.is_ready());
    }
}
