// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The resources `forge` owns: `Host`, `Size` and `Machine`.
//!
//! All three are `Resource<Spec, Status>` envelopes from `pistack-types`.
//! `spec` is written by handlers (or, for hosts, by the agent report that
//! the transport delivers); `status` is written only by the reconciler.

use chrono::{DateTime, Utc};
use pistack_types::{kind, Condition, Id, Kind, Meta, Phase, Resource, Resources};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::ops::{Deref, DerefMut};

/// Declare a newtype over `Resource<Spec, Status>` that implements `Kind`.
/// (The orphan rule forbids `impl Kind for Resource<Local, Local>`.) The
/// newtype derefs to the envelope, so `m.meta`, `m.spec` and `m.status`
/// read naturally, and serialises transparently.
macro_rules! resource_kind {
    ($(#[$doc:meta])* $name:ident, $spec:ty, $status:ty, $kind:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        #[serde(transparent)]
        pub struct $name(pub Resource<$spec, $status>);

        impl Deref for $name {
            type Target = Resource<$spec, $status>;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
        impl From<Resource<$spec, $status>> for $name {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self(r)
            }
        }
        impl $name {
            pub fn into_inner(self) -> Resource<$spec, $status> {
                self.0
            }
        }
        impl Kind for $name {
            const KIND: &'static str = $kind;
            fn meta(&self) -> &Meta {
                &self.0.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.0.meta
            }
        }
    };
}

/// Id prefix / store table for sizes. Hosts and machines use the shared
/// `pistack_types::kind` constants.
pub const SIZE_KIND: &str = "size";

/// Annotation a DELETE handler sets to ask the reconciler to tear a machine
/// down. Handlers never write `status`, so the request travels in metadata
/// (the Kubernetes `deletionTimestamp` idea) and the reconciler flips the
/// phase to `Deleting`.
pub const DELETE_ANNOTATION: &str = "pistack.io/delete";

/// While this annotation is non-empty, forge does not place or start the
/// machine. Loom sets it to the wait message until every slab volume the
/// pod names is attached on a node.
pub const WAIT_VOLUME_ANNOTATION: &str = "loom.pistack.io/wait-volume";

/// Host id the pod's slab volumes are attached on. Placement considers
/// only that host.
pub const NODE_PIN_ANNOTATION: &str = "loom.pistack.io/node";

/// Host label that names the availability zone. `MachineSpec::zone` is a
/// hard selector on it.
pub const ZONE_LABEL: &str = "zone";

/// Labels under this prefix are written by PiStack itself. An operator
/// cannot set them, and replacing a host's labels keeps them.
pub const SYSTEM_LABEL_PREFIX: &str = "pistack.io/";

// ---- Host ------------------------------------------------------------------

/// What a host *is*: reported by the agent at hello time and on change.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostSpec {
    /// The agent's node name (usually the hostname).
    pub node_name: String,
    /// Total resources the host offers to the scheduler.
    pub capacity: Resources,
    /// Capabilities the agent detected: "kvm", "libvirt", "youki", "ovs",
    /// "metal", "mock". The scheduler maps `MachineKind` onto these.
    #[serde(default)]
    pub drivers: Vec<String>,
    /// Underlay IPv4 reported by the agent. None means this host is not on
    /// any overlay segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlay_ip: Option<String>,
    /// Set by an operator, not by the agent. The scheduler places nothing
    /// new on a disabled host. Machines already there keep running.
    #[serde(default)]
    pub disabled: bool,
    /// Why the host is disabled, in the operator's words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub disabled_reason: String,
}

/// What is true about a host right now. Written by the reconciler.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostStatus {
    /// Capacity minus everything forge has placed on the host.
    pub free: Resources,
    /// Last heartbeat / report. A host unseen for `HOST_STALE_AFTER` goes
    /// to `Phase::Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<DateTime<Utc>>,
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
    /// OVN on this host, from the agent's last heartbeat. None when the
    /// agent sent no report. The scheduler places a machine on an OVN
    /// network only where this says ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ovn: Option<pistack_rpc::OvnReport>,
}

impl HostStatus {
    /// Keep the agent's OVN report and mirror it as the `OvnReady`
    /// condition. No report removes both.
    pub fn set_ovn(&mut self, report: Option<&pistack_rpc::OvnReport>) {
        if self.ovn.as_ref() == report {
            return;
        }
        self.ovn = report.cloned();
        let Some(report) = report else {
            self.conditions.retain(|c| c.kind != "OvnReady");
            return;
        };
        let reason = if report.ready {
            "ChassisConnected"
        } else {
            "ChassisNotReady"
        };
        match self.conditions.iter_mut().find(|c| c.kind == "OvnReady") {
            Some(c) => {
                if c.status != report.ready {
                    c.last_transition = Utc::now();
                }
                c.status = report.ready;
                c.reason = reason.to_owned();
                c.message = report.message.clone();
            }
            None => self.conditions.push(Condition {
                kind: "OvnReady".to_owned(),
                status: report.ready,
                reason: reason.to_owned(),
                message: report.message.clone(),
                last_transition: Utc::now(),
            }),
        }
    }
}

resource_kind!(
    /// A host the scheduler can place machines on.
    Host,
    HostSpec,
    HostStatus,
    kind::HOST
);

/// Build a host resource. `labels` land in `meta.labels` so the scheduler's
/// `host_selector` can match them.
pub fn new_host(
    name: impl Into<String>,
    node_name: impl Into<String>,
    capacity: Resources,
    drivers: Vec<String>,
    labels: BTreeMap<String, String>,
) -> Host {
    let mut meta = Meta::new(kind::HOST, name, None);
    meta.labels = labels;
    Host(Resource {
        meta,
        spec: HostSpec {
            node_name: node_name.into(),
            capacity,
            drivers,
            ..Default::default()
        },
        status: HostStatus {
            free: capacity,
            last_seen: Some(Utc::now()),
            phase: Phase::Ready,
            ..Default::default()
        },
    })
}

// ---- Size ------------------------------------------------------------------

/// A flavor. The name is the handle users pass (`--size m.2x4`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SizeSpec {
    #[serde(flatten)]
    pub resources: Resources,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SizeStatus {}

resource_kind!(
    /// A flavor: named bundle of resources.
    Size,
    SizeSpec,
    SizeStatus,
    SIZE_KIND
);

/// Sizes are cloud-scoped (project `None`).
pub fn new_size(name: impl Into<String>, resources: Resources) -> Size {
    Size(Resource {
        meta: Meta::new(SIZE_KIND, name, None),
        spec: SizeSpec { resources },
        status: SizeStatus {},
    })
}

/// The sizes seeded on first start.
pub fn default_sizes() -> Vec<Size> {
    let r = |vcpus, memory_mib, disk_gib| Resources {
        vcpus,
        memory_mib,
        disk_gib,
        gpus: 0,
    };
    vec![
        new_size("s.1x1", r(1, 1024, 10)),
        new_size("m.2x4", r(2, 4096, 20)),
        new_size("l.4x8", r(4, 8192, 40)),
        new_size("xl.8x16", r(8, 16384, 80)),
    ]
}

// ---- Machine ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum MachineKind {
    #[default]
    Vm,
    Container,
    Metal,
}

impl MachineKind {
    /// Host driver capabilities that can run this kind. A host advertising
    /// any one of them qualifies. "mock" runs everything (dev mode).
    pub fn accepted_drivers(self) -> &'static [&'static str] {
        match self {
            MachineKind::Vm => &["kvm", "libvirt", "cloud-hypervisor", "mock"],
            MachineKind::Container => &["youki", "mock"],
            MachineKind::Metal => &["metal", "ipmi", "redfish", "mock"],
        }
    }
}

/// Desired power state. `pi machine stop` flips this to `Off`; the
/// reconciler makes it true and records the observed state in `status`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum DesiredPower {
    #[default]
    On,
    Off,
}

/// Observed power state.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    On,
    Off,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MachineSpec {
    #[serde(default)]
    pub kind: MachineKind,
    /// Size name (or id).
    pub size: String,
    pub image: String,
    #[serde(default)]
    pub networks: Vec<String>,
    /// Security group names or ids. Empty uses the project's default group.
    #[serde(default)]
    pub security_groups: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_data: Option<String>,
    #[serde(default)]
    pub ssh_keys: Vec<String>,
    /// Soft hint: prefer hosts that already run machines with these names.
    #[serde(default)]
    pub affinity: Vec<String>,
    /// Soft hint: avoid hosts that run machines with these names.
    #[serde(default)]
    pub anti_affinity: Vec<String>,
    /// Hard constraint: every pair must match a host label.
    #[serde(default)]
    pub host_selector: BTreeMap<String, String>,
    /// Availability zone. Hard constraint: the host's `zone` label must
    /// equal it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    #[serde(default)]
    pub power: DesiredPower,
    /// Monotonic counter bumped by `POST /machines/{id}/reboot`. The
    /// reconciler restarts the machine when it exceeds `status.restarts`.
    #[serde(default)]
    pub restarts: u64,
    /// Secret names this guest may read. Payloads stay in keep.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    /// Baseboard address for a chassis that is not this host.
    /// The agent does not power off the host it is running on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmc_address: Option<String>,
    /// IPMI username. The password is a file under `PISTACK_IPMI_DIR`,
    /// named by the machine id, and is not stored on the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmc_user: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MachineStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Id>,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub power: PowerState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// Restarts the reconciler has performed; compare with `spec.restarts`.
    #[serde(default)]
    pub restarts: u64,
    /// Probe streaks last reported by the host agent. Empty when the
    /// machine has no liveness or readiness probes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<pistack_rpc::proto::ProbeReport>,
}

impl MachineStatus {
    /// Set a condition, replacing one of the same type; bumps
    /// `last_transition` only when `status` or `reason` changed.
    pub fn set_condition(&mut self, kind: &str, status: bool, reason: &str, message: &str) {
        if let Some(c) = self.conditions.iter_mut().find(|c| c.kind == kind) {
            if c.status != status || c.reason != reason {
                c.last_transition = Utc::now();
            }
            c.status = status;
            c.reason = reason.to_owned();
            c.message = message.to_owned();
            return;
        }
        self.conditions.push(Condition {
            kind: kind.to_owned(),
            status,
            reason: reason.to_owned(),
            message: message.to_owned(),
            last_transition: Utc::now(),
        });
    }
}

resource_kind!(
    /// A VM, container or bare-metal machine.
    Machine,
    MachineSpec,
    MachineStatus,
    kind::MACHINE
);

pub fn new_machine(name: impl Into<String>, project: Option<Id>, spec: MachineSpec) -> Machine {
    Machine(Resource {
        meta: Meta::new(kind::MACHINE, name, project),
        spec,
        status: MachineStatus::default(),
    })
}

/// Usable underlay address: IPv4 unicast in 10/8, 172.16/12, 192.168/16,
/// or 100.64/10. A missing or rejected value is `None`.
pub(crate) fn usable_underlay(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let ip: Ipv4Addr = raw.parse().ok()?;
    underlay_ip_ok(ip).then(|| ip.to_string())
}

fn underlay_ip_ok(ip: Ipv4Addr) -> bool {
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || ip.is_loopback() {
        return false;
    }
    let [a, b, _, _] = ip.octets();
    a == 10
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
}

/// Whether a delete has been requested (see `DELETE_ANNOTATION`).
pub fn delete_requested(m: &Machine) -> bool {
    m.meta.annotations.contains_key(DELETE_ANNOTATION) || m.status.phase == Phase::Deleting
}
