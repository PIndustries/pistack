// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Wire types shared by the control plane and `anvil`.
//!
//! The protocol is JSON over HTTP with a long-poll for commands:
//!
//! | Method | Path                       | Body            | Reply            |
//! | ------ | -------------------------- | --------------- | ---------------- |
//! | POST   | `/agents/enroll`           | [`EnrollRequest`] | [`EnrollResponse`] |
//! | POST   | `/agents/hello`            | [`AgentHello`]  | [`HelloAck`]     |
//! | POST   | `/agents/heartbeat`        | [`Heartbeat`]   | [`HeartbeatAck`] |
//! | GET    | `/agents/{node}/commands`  | `?wait=&max=`   | `Vec<Command>`   |
//! | POST   | `/agents/{node}/results`   | `Vec<CommandResult>` | [`ReportAck`] |
//! | GET    | `/metadata/networks/{node}` |                | `Vec<MetadataNetwork>` |
//! | POST   | `/metadata/guest`          | [`MetadataQuery`] | [`MetadataAnswer`] |
//!
//! Every type here is `serde` and nothing else; the transport can be swapped
//! for gRPC later without touching the queue or the modules.

use chrono::{DateTime, Utc};
use pistack_types::{ApiError, Id, Phase, Resources};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Longest a command long-poll may hang before an empty reply.
pub const MAX_LONG_POLL_SECS: u64 = 30;
/// Default `max` for a command poll.
pub const DEFAULT_POLL_BATCH: usize = 16;

/// Clear-text join token on each agent request. Omitted when the agent has
/// no token. Not an `Authorization` bearer: the gateway treats those as gate
/// session tokens.
pub const JOIN_HEADER: &str = "x-pistack-join";

/// Spend a join token once and receive a CA-signed client certificate.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrollRequest {
    /// Becomes the certificate common name and the agent node name.
    pub node_name: String,
}

/// Material returned by `/agents/enroll`. Persist and present on TLS.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrollResponse {
    pub cert_pem: String,
    pub key_pem: String,
    pub ca_pem: String,
    pub grant: keep::JoinGrant,
}

/// First message an agent sends after it starts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentHello {
    pub node_name: String,
    /// What this host can run: "kvm", "container", "baremetal", "ovn",
    /// "lvm", "rbd", ...
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Total resources of the host.
    pub resources: Resources,
    /// Agent build version, for the operator's eyes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// Labels the operator set on the host (rack, zone, ...).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
    /// IPv4 address of this host on the underlay. Empty when the agent could
    /// not learn a usable address. The scheduler will not place a networked
    /// machine on a host that leaves this empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlay_ip: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HelloAck {
    pub node_name: String,
    /// Server's idea of now; agents may use it to detect clock skew.
    pub server_time: DateTime<Utc>,
    /// How often the agent should send [`Heartbeat`].
    pub heartbeat_interval_secs: u64,
    /// Upper bound the server will honour for `?wait=` on the command poll.
    pub max_long_poll_secs: u64,
    /// Set when the cluster's networks are OVN and the server knows the
    /// southbound address. The agent points this host's Open vSwitch at it.
    /// Absent leaves the host as the operator configured it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ovn: Option<OvnHostConfig>,
}

/// What a host needs to join the cluster's OVN as a chassis.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OvnHostConfig {
    /// Southbound database, as `ovn-controller` takes it:
    /// `tcp:10.0.0.11:6642`, or several separated by commas.
    pub sb_remote: String,
    /// Tunnel encapsulation: `geneve` or `vxlan`.
    pub encap_type: String,
}

/// OVN on one host, as its agent last saw it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OvnReport {
    /// `external_ids:system-id`, the name OVN knows this chassis by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chassis: String,
    /// The integration bridge exists and `ovn-controller` is connected to
    /// the southbound database.
    #[serde(default)]
    pub ready: bool,
    /// Why the host is not ready. Empty when it is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// `external_ids:ovn-bridge-mappings`, `physnet:bridge` pairs. A host
    /// that has one can carry a router's gateway port.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bridge_mappings: String,
}

/// Periodic state report.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Heartbeat {
    pub node_name: String,
    pub resources_free: Resources,
    #[serde(default)]
    pub machines: Vec<MachineReport>,
    #[serde(default = "Utc::now")]
    pub at: DateTime<Utc>,
    /// OVN on this host. Absent when the host has no Open vSwitch or the
    /// agent has not looked yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ovn: Option<OvnReport>,
    /// What the host and its machines are using. Sent at most every
    /// [`USAGE_INTERVAL_SECS`]; a heartbeat in between leaves it out, and
    /// so does an agent from before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageReport>,
}

/// The shortest time between two usage reports from one agent.
pub const USAGE_INTERVAL_SECS: u64 = 10;

/// Usage measured on a host. Every figure is an integer so the report
/// compares exactly; counters are since the machine or the host started.
///
/// In JSON a machine with one disk and one interface is about 300 bytes,
/// so a host with a hundred machines adds some 30 KiB to a heartbeat.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageReport {
    pub host: HostUsage,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<MachineUsage>,
}

/// The host itself.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostUsage {
    /// Load averages over 1, 5 and 15 minutes, times 1000.
    pub load_milli: [u32; 3],
    /// Absent where the agent cannot read memory (not Linux).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total_bytes: Option<u64>,
    /// What the kernel says is available without swapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_available_bytes: Option<u64>,
    /// The filesystem that holds the agent's data directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_disk_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_disk_free_bytes: Option<u64>,
}

/// One running machine.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineUsage {
    pub id: Id,
    /// CPU time used, in nanoseconds.
    pub cpu_ns: u64,
    /// Memory of the host the machine holds (resident set).
    pub memory_bytes: u64,
    /// Memory the machine is given now (the balloon). 0 when unknown.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub memory_assigned_bytes: u64,
    /// Memory in use inside the guest, when the guest reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_guest_used_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<DiskUsage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nics: Vec<NicUsage>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiskUsage {
    /// Device name in the guest, `vda`.
    pub device: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_ops: u64,
    pub write_ops: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NicUsage {
    /// Interface name on the host, `vnet0`.
    pub name: String,
    /// Bytes the machine received and sent.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// One machine as the host sees it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineReport {
    pub id: Id,
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Resources>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    /// Observed power state, when the runtime knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<PowerState>,
    /// Latest liveness and readiness streaks for a pod sandbox. Empty for
    /// a machine that has no probes. The control plane copies this onto
    /// the machine status; it is not a separate call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<ProbeReport>,
}

/// `kind` on a [`ProbeReport`] for a liveness probe.
pub const PROBE_LIVENESS: &str = "liveness";
/// `kind` on a [`ProbeReport`] for a readiness probe.
pub const PROBE_READINESS: &str = "readiness";

/// One container probe as the host last observed it.
///
/// `successes` and `failures` are the current consecutive streaks. A
/// success clears `failures` and the other way around. The pod controller
/// compares the streaks with the probe thresholds on the pod spec.
/// `unsupported` means the runtime cannot run this probe. The streaks stay
/// at zero. That is not a pass and not a failure.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProbeReport {
    pub container: String,
    /// [`PROBE_LIVENESS`] or [`PROBE_READINESS`].
    pub kind: String,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub successes: u32,
    #[serde(default)]
    pub failures: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// The runtime cannot run this probe. Streaks stay at zero.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unsupported: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

// ---- machine command payloads ---------------------------------------------
//
// `forge` builds these from its `Machine` resource and `anvil` executes them
// with a `MachineRuntime`; neither crate depends on the other, so the shape
// they agree on lives here.

/// What kind of machine a request is for. Mirrors `forge::MachineKind`.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum MachineKind {
    #[default]
    Vm,
    Container,
    Metal,
}

/// Observed power state of a machine. Mirrors `forge::PowerState`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    On,
    Off,
    #[default]
    Unknown,
}

/// What fabric recorded once a port was realised. Mirrors the port binding:
/// `vif_type` `ovs` carries `vif_details` `bridge` and `iface_id`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NicBinding {
    /// `ovs`, `hostnat`, `overlay`, `mock`, ... Empty keeps the Linux bridge.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vif_type: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vif_details: BTreeMap<String, String>,
}

/// One NIC whose address was allocated by fabric. Empty `nics` on a
/// [`MachineRequest`] means the shared `pistack` network.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NicAttachment {
    /// Fabric network id. The host turns this into a libvirt network name.
    pub network_id: String,
    pub mac: String,
    /// IPv4 address with no prefix.
    pub address: String,
    pub prefix: u8,
    /// Subnet gateway. The guest uses it as a default route only when `routed`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gateway: String,
    /// A router on this subnet has an external network, so the host NATs it.
    #[serde(default)]
    pub routed: bool,
    /// VXLAN VNI. Set only for the overlay datapath. Absent keeps the
    /// host NAT bridge, including the shared `pistack` network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<u32>,
    /// Port binding. `vif_type` `ovs` plugs the interface into the OVS
    /// bridge named in `vif_details` instead of a Linux bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<NicBinding>,
}

/// A catalog image the server holds, for an agent that does not have the
/// file. The agent downloads it from `/rpc/images/{id}/data`, checks the
/// digest, and keeps it under its data directory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageSource {
    /// Image id in the catalog.
    pub id: String,
    /// Lowercase hex SHA-256 of the image bytes.
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Disk or archive format as the catalog records it, e.g. `qcow2`.
    #[serde(default)]
    pub format: String,
}

/// Payload of [`CommandKind::StartMachine`]: everything a runtime needs to
/// bring a machine up.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineRequest {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub kind: MachineKind,
    pub image: String,
    /// Set when `image` came from the catalog. `image` may then be a path
    /// on the server that this host does not have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_source: Option<ImageSource>,
    #[serde(default)]
    pub resources: Resources,
    #[serde(default)]
    pub networks: Vec<String>,
    /// Port addresses. Empty keeps the shared host network.
    #[serde(default)]
    pub nics: Vec<NicAttachment>,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_data: Option<String>,
    #[serde(default)]
    pub ssh_keys: Vec<String>,
    /// Overlay plan for this host. The agent applies it before plugging a
    /// NIC that carries [`NicAttachment::segment_id`]. Absent on host NAT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<serde_json::Value>,
    /// Baseboard address when this machine is a remote chassis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmc_address: Option<String>,
    /// IPMI username. The password stays in a file on the agent host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmc_user: Option<String>,
}

/// Payload of [`CommandKind::StopMachine`], [`CommandKind::DeleteMachine`],
/// and [`CommandKind::ReadLogs`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineRef {
    pub id: Id,
}

/// Payload of [`CommandKind::PlugPort`] and [`CommandKind::UnplugPort`].
///
/// The NIC is the fabric port. When [`NicAttachment::segment_id`] is set, the
/// host attaches the overlay bridge named `po` plus 12 hex digits of
/// `network_id` — the bridge the overlay plan already computed. That path
/// does not use `pistack0`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortPlug {
    pub machine: Id,
    #[serde(flatten)]
    pub nic: NicAttachment,
}

/// Result payload of [`CommandKind::ReadLogs`]. `text` is utf-8 from the
/// youki bundle log or the libvirt console log.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogText {
    pub text: String,
}

/// Default wait for [`CommandKind::Exec`] when the caller omits a timeout.
pub const EXEC_TIMEOUT_DEFAULT_SECS: u64 = 30;
/// Longest [`CommandKind::Exec`] wait the agent will accept.
pub const EXEC_TIMEOUT_MAX_SECS: u64 = 3600;

/// Turn an optional timeout into a duration. Absent means
/// [`EXEC_TIMEOUT_DEFAULT_SECS`]. Zero and values above
/// [`EXEC_TIMEOUT_MAX_SECS`] are rejected.
pub fn exec_timeout(secs: Option<u64>) -> Result<std::time::Duration, ApiError> {
    let secs = secs.unwrap_or(EXEC_TIMEOUT_DEFAULT_SECS);
    if !(1..=EXEC_TIMEOUT_MAX_SECS).contains(&secs) {
        return Err(ApiError::bad_request(format!(
            "timeout_secs must be between 1 and {EXEC_TIMEOUT_MAX_SECS}"
        )));
    }
    Ok(std::time::Duration::from_secs(secs))
}

/// One environment variable for [`CommandKind::Exec`]. The value is literal.
/// It is not read from a config map or a secret.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecEnv {
    pub name: String,
    #[serde(default)]
    pub value: String,
}

/// Payload of [`CommandKind::Exec`]. `id` is the pod's machine. `container`
/// is that sandbox's container name. `command` is argv, not a shell line.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecRequest {
    pub id: Id,
    pub container: String,
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<ExecEnv>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Absent means [`EXEC_TIMEOUT_DEFAULT_SECS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Result payload of [`CommandKind::Exec`]. A non-zero `exit_code` means the
/// command ran and failed. It is not a transport error.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Payload of [`CommandKind::MigrateMachine`]. `dest` is a libvirt URI,
/// `qemu+ssh://HOST/system`. The source agent live-migrates the domain;
/// the guest XML is not redefined.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrateRequest {
    pub id: Id,
    /// `qemu+ssh://DEST/system`.
    pub dest: String,
}

/// How a host reaches a volume that is not only a filesystem path.
///
/// `Local` is a file or an LVM device path. `Rbd` is the attach target
/// `slab` reports for a Ceph image. `key_secret`, when set, is a keyring
/// file path. It is not written into guest XML and it is not shown in Debug.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum DiskTarget {
    Local {
        path: String,
    },
    Rbd {
        pool: String,
        image: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        monitors: Vec<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        user: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_secret: Option<String>,
    },
}

impl std::fmt::Debug for DiskTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiskTarget::Local { path } => f.debug_struct("Local").field("path", path).finish(),
            DiskTarget::Rbd {
                pool,
                image,
                monitors,
                user,
                key_secret,
            } => f
                .debug_struct("Rbd")
                .field("pool", pool)
                .field("image", image)
                .field("monitors", monitors)
                .field("user", user)
                .field("key_secret", &key_secret.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

/// Payload of [`CommandKind::AttachVolume`]. `path` is the host file `slab`
/// created, or an LVM device path under `/dev`. `target` is set when the
/// volume is an RBD image. A VM agent attaches that image as a network disk
/// and leaves `path` empty. A pod agent maps the image, mounts a filesystem
/// on the block device, and records that directory. The agent may stage a
/// copy of a regular file under its libvirt image directory when AppArmor
/// will not let qemu open `path` in place. A block device is attached in
/// place.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeAttach {
    pub machine: Id,
    pub volume: Id,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default)]
    pub read_only: bool,
    /// Slab attach target. Absent for a host path carried in `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<DiskTarget>,
    /// Filesystem to create when the block device has none.
    ///
    /// Empty means `ext4`. A pod attach formats only when `blkid` reports
    /// no type. A VM attach ignores this field.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fs_type: String,
}

/// What the guest disk was named, and the host path the agent checked.
///
/// For a pod, `host_path` is the directory where the volume is mounted.
/// For a VM, it is a local file or an LVM device node. A VM network disk
/// has no host map, so `host_path` is absent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeAttached {
    pub device: String,
    /// Mount directory for a pod, or the file or device node for a VM.
    /// Absent for a VM network disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_path: Option<String>,
}

/// Payload of [`CommandKind::DetachVolume`].
///
/// `target` is set when the disk was attached from an RBD image. A pod agent
/// unmaps the host device it mapped at attach. A VM agent removes the network
/// disk and does not unmap a host device.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeDetach {
    pub machine: Id,
    pub volume: Id,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Present when the disk was attached from an RBD target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<DiskTarget>,
}

/// Result payload of a successful [`CommandKind::StartMachine`]: what the
/// runtime reports about the machine it now holds.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunningMachine {
    pub id: Id,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub power: PowerState,
    /// Set when the runtime could not attach the recorded address.
    /// Empty when the guest has that address, or when there was nothing to attach.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

/// Something an agent said, re-broadcast by the control plane so modules
/// can react (register a host, refresh `last_seen`, reconcile reports).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    Hello(AgentHello),
    Heartbeat(Heartbeat),
}

impl AgentEvent {
    pub fn node_name(&self) -> &str {
        match self {
            AgentEvent::Hello(h) => &h.node_name,
            AgentEvent::Heartbeat(h) => &h.node_name,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatAck {
    pub server_time: DateTime<Utc>,
    /// Commands waiting for this node; a hint to poll right away.
    pub commands_waiting: usize,
    /// `false` when the server has never seen a hello from this node (for
    /// example after a control-plane restart) and wants one.
    pub known: bool,
    /// Where this agent reaches every server in the cluster, so it can try
    /// another one if the server it talks to stops answering: the TLS
    /// listener of each server that has one, for an agent that called over
    /// TLS. Empty for a single server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<String>,
}

/// What the control plane asks a host to do.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    StartMachine,
    StopMachine,
    DeleteMachine,
    AttachVolume,
    DetachVolume,
    PlugPort,
    UnplugPort,
    /// Install or refresh this host's overlay plan. The payload is the plan
    /// document. It does not redefine a guest that is already running.
    SyncOverlay,
    /// Live-migrate a running domain. The payload is [`MigrateRequest`].
    MigrateMachine,
    /// Read the youki bundle log or the libvirt console log. The payload is
    /// [`MachineRef`]. The result is [`LogText`].
    ReadLogs,
    /// Run a command in one container of a pod sandbox. The payload is
    /// [`ExecRequest`]. The result is [`ExecOutput`]. It is not a shell
    /// and it does not attach a terminal.
    Exec,
    /// Ask an edge host to publish a listener. The payload is the listener
    /// id, the listen port, and the upstream. It does not power a host.
    Publish,
}

impl std::fmt::Display for CommandKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CommandKind::StartMachine => "start_machine",
            CommandKind::StopMachine => "stop_machine",
            CommandKind::DeleteMachine => "delete_machine",
            CommandKind::AttachVolume => "attach_volume",
            CommandKind::DetachVolume => "detach_volume",
            CommandKind::PlugPort => "plug_port",
            CommandKind::UnplugPort => "unplug_port",
            CommandKind::SyncOverlay => "sync_overlay",
            CommandKind::MigrateMachine => "migrate_machine",
            CommandKind::ReadLogs => "read_logs",
            CommandKind::Exec => "exec",
            CommandKind::Publish => "publish",
        };
        f.write_str(s)
    }
}

/// Identifier of one command; a UUIDv7 so ids sort by issue time.
pub type CommandId = Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub kind: CommandKind,
    /// Kind-specific body (machine spec, volume attachment, port), owned by
    /// the module that issued it.
    #[serde(default)]
    pub payload: serde_json::Value,
    pub issued_at: DateTime<Utc>,
    /// The agent keeps the result of this command for a while and answers
    /// a second delivery of the same id from that record, without running
    /// it again. Set on commands whose id outlives a leader change.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remember: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandResult {
    pub id: CommandId,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl CommandResult {
    pub fn ok(id: CommandId) -> Self {
        Self {
            id,
            ok: true,
            message: String::new(),
            payload: serde_json::Value::Null,
        }
    }
    pub fn failed(id: CommandId, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            message: message.into(),
            payload: serde_json::Value::Null,
        }
    }
    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }

    /// A failure carrying the full [`ApiError`] in the payload so the
    /// issuing module can keep its code (`not_found` vs `unavailable`).
    pub fn from_api_error(id: CommandId, e: &ApiError) -> Self {
        Self {
            id,
            ok: false,
            message: e.message.clone(),
            payload: serde_json::to_value(e).unwrap_or(serde_json::Value::Null),
        }
    }

    /// `Ok(())` for a successful result; otherwise the [`ApiError`] the
    /// agent sent (see [`CommandResult::from_api_error`]), or an
    /// `internal` error built from `message` when it sent none.
    pub fn into_api_result(self) -> Result<serde_json::Value, ApiError> {
        if self.ok {
            return Ok(self.payload);
        }
        match serde_json::from_value::<ApiError>(self.payload.clone()) {
            Ok(e) => Err(e),
            Err(_) => Err(ApiError::internal(if self.message.is_empty() {
                "command failed on host".to_owned()
            } else {
                self.message
            })),
        }
    }
}

/// A network with a guest on the asking host, and what the host needs to
/// answer metadata for it when the networks are OVN.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataNetwork {
    /// Fabric network id.
    pub network: String,
    /// Name of the network's metadata port in OVN. The host sets it as
    /// `iface-id` on its end.
    pub port: String,
    /// MAC of that port.
    pub mac: String,
    /// The network's IPv4 subnets, the way back to the guests.
    #[serde(default)]
    pub cidrs: Vec<String>,
}

/// One metadata request a host passes on for a guest. The host fills in
/// `network` and `address` from where the connection arrived, never from
/// what the guest sent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataQuery {
    pub node_name: String,
    pub network: String,
    /// The guest's address on that network.
    pub address: std::net::IpAddr,
    /// Request path, e.g. `/latest/meta-data/hostname`.
    pub path: String,
}

/// What to send back to the guest.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataAnswer {
    /// HTTP status for the guest. An unknown guest or path is 404.
    pub status: u16,
    #[serde(default)]
    pub body: Vec<u8>,
}

/// Reply to a batch of results.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReportAck {
    /// Results matched to a pending command.
    pub accepted: usize,
    /// Ids the server no longer knew (already timed out or duplicated).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<CommandId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_kind_wire_names() {
        let v = serde_json::to_value(CommandKind::AttachVolume).unwrap();
        assert_eq!(v, serde_json::json!("attach_volume"));
        assert_eq!(CommandKind::AttachVolume.to_string(), "attach_volume");
        let back: CommandKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, CommandKind::AttachVolume);
        let v = serde_json::to_value(CommandKind::MigrateMachine).unwrap();
        assert_eq!(v, serde_json::json!("migrate_machine"));
        assert_eq!(CommandKind::MigrateMachine.to_string(), "migrate_machine");
        let v = serde_json::to_value(CommandKind::ReadLogs).unwrap();
        assert_eq!(v, serde_json::json!("read_logs"));
        assert_eq!(CommandKind::ReadLogs.to_string(), "read_logs");
        let back: CommandKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, CommandKind::ReadLogs);
        let v = serde_json::to_value(CommandKind::Exec).unwrap();
        assert_eq!(v, serde_json::json!("exec"));
        assert_eq!(CommandKind::Exec.to_string(), "exec");
        let back: CommandKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, CommandKind::Exec);
        assert_eq!(CommandKind::PlugPort.to_string(), "plug_port");
        assert_eq!(CommandKind::UnplugPort.to_string(), "unplug_port");
    }

    #[test]
    fn exec_request_round_trips_and_timeout_is_bounded() {
        let id = sample_id("mach");
        let req = ExecRequest {
            id: Id::parse(pistack_types::kind::MACHINE, &id).unwrap(),
            container: "web".into(),
            command: vec!["/bin/echo".into(), "hi".into()],
            env: vec![ExecEnv {
                name: "MODE".into(),
                value: "prod".into(),
            }],
            cwd: Some("/work".into()),
            timeout_secs: Some(5),
        };
        let v = serde_json::to_value(&req).unwrap();
        let back: ExecRequest = serde_json::from_value(v).unwrap();
        assert_eq!(back, req);
        let bare: ExecRequest = serde_json::from_value(serde_json::json!({
            "id": id,
            "container": "web",
            "command": ["/bin/true"]
        }))
        .unwrap();
        assert!(bare.env.is_empty());
        assert!(bare.cwd.is_none());
        assert!(bare.timeout_secs.is_none());
        assert_eq!(
            exec_timeout(None).unwrap(),
            std::time::Duration::from_secs(EXEC_TIMEOUT_DEFAULT_SECS)
        );
        assert_eq!(
            exec_timeout(Some(5)).unwrap(),
            std::time::Duration::from_secs(5)
        );
        assert!(exec_timeout(Some(0)).is_err());
        assert!(exec_timeout(Some(EXEC_TIMEOUT_MAX_SECS + 1)).is_err());
    }

    #[test]
    fn probe_report_omits_a_false_unsupported_flag() {
        let report = ProbeReport {
            container: "web".into(),
            kind: PROBE_LIVENESS.into(),
            success: false,
            successes: 0,
            failures: 1,
            message: "exit 1".into(),
            unsupported: false,
        };
        let value = serde_json::to_value(&report).unwrap();
        assert!(value.get("unsupported").is_none());
        let back: ProbeReport = serde_json::from_value(value).unwrap();
        assert_eq!(back, report);

        let unsupported = ProbeReport {
            container: "web".into(),
            kind: PROBE_READINESS.into(),
            message: "exec probes are not supported".into(),
            unsupported: true,
            ..ProbeReport::default()
        };
        let value = serde_json::to_value(&unsupported).unwrap();
        assert_eq!(value["unsupported"], true);
        let back: ProbeReport = serde_json::from_value(value).unwrap();
        assert!(back.unsupported);
        assert_eq!(back.successes, 0);
        assert_eq!(back.failures, 0);
    }

    fn sample_id(kind: &str) -> String {
        format!("{kind}-018f2c3a-7b6e-7a11-8c22-123456789abc")
    }

    #[test]
    fn volume_attach_accepts_an_lvm_path_and_an_rbd_target() {
        let old: VolumeAttach = serde_json::from_str(&format!(
            r#"{{"machine":"{}","volume":"{}","path":"/dev/vg/lv"}}"#,
            sample_id("mach"),
            sample_id("vol")
        ))
        .unwrap();
        assert!(old.target.is_none());
        assert_eq!(old.path, "/dev/vg/lv");
        assert!(old.fs_type.is_empty());

        let secret = "super-secret-value";
        let with = serde_json::json!({
            "machine": sample_id("mach"),
            "volume": sample_id("vol"),
            "path": "",
            "target": {
                "protocol": "rbd",
                "pool": "volumes",
                "image": "vol-1",
                "monitors": ["10.1.0.5:6789"],
                "user": "pistack",
                "key_secret": secret
            }
        });
        let parsed: VolumeAttach = serde_json::from_value(with.clone()).unwrap();
        match &parsed.target {
            Some(DiskTarget::Rbd {
                pool,
                image,
                monitors,
                user,
                key_secret,
            }) => {
                assert_eq!(pool, "volumes");
                assert_eq!(image, "vol-1");
                assert_eq!(monitors, &vec!["10.1.0.5:6789".to_string()]);
                assert_eq!(user, "pistack");
                assert_eq!(key_secret.as_deref(), Some(secret));
            }
            other => panic!("expected rbd target, got {other:?}"),
        }
        let shown = format!("{:?}", parsed.target);
        assert!(!shown.contains(secret));
        assert!(shown.contains("<redacted>"));
        let back = serde_json::to_value(&parsed).unwrap();
        assert_eq!(back["target"]["pool"], "volumes");
    }

    #[test]
    fn volume_attached_host_path_is_optional() {
        let old: VolumeAttached = serde_json::from_str(r#"{"device":"vdb"}"#).unwrap();
        assert_eq!(old.device, "vdb");
        assert!(old.host_path.is_none());
        let wire = serde_json::to_value(&old).unwrap();
        assert!(wire.get("host_path").is_none());

        let with = VolumeAttached {
            device: "vdc".into(),
            host_path: Some("/dev/rbd0".into()),
        };
        let value = serde_json::to_value(&with).unwrap();
        assert_eq!(value["host_path"], "/dev/rbd0");
        let back: VolumeAttached = serde_json::from_value(value).unwrap();
        assert_eq!(back, with);
    }

    #[test]
    fn port_plug_and_log_text_round_trip() {
        let plug = PortPlug {
            machine: Id::parse("mach", &sample_id("mach")).unwrap(),
            nic: NicAttachment {
                network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
                mac: "52:54:00:aa:bb:cc".into(),
                address: "10.90.0.10".into(),
                prefix: 24,
                gateway: "10.90.0.1".into(),
                routed: true,
                segment_id: Some(7),
                binding: None,
            },
        };
        let text = serde_json::to_string(&plug).unwrap();
        assert!(text.contains("\"segment_id\":7"));
        assert!(!text.contains("\"nic\""));
        assert!(!text.contains("binding"));
        let back: PortPlug = serde_json::from_str(&text).unwrap();
        assert_eq!(back, plug);
        let old: NicAttachment =
            serde_json::from_str(r#"{"network_id":"n","mac":"m","address":"a","prefix":24}"#)
                .unwrap();
        assert!(old.binding.is_none());
        let logs: LogText = serde_json::from_str(r#"{"text":"boot ok"}"#).unwrap();
        assert_eq!(logs.text, "boot ok");
    }

    #[test]
    fn hello_minimal_json() {
        let h: AgentHello =
            serde_json::from_str(r#"{"node_name":"n1","resources":{"vcpus":4,"memory_mib":8192}}"#)
                .unwrap();
        assert_eq!(h.node_name, "n1");
        assert!(h.capabilities.is_empty());
        assert_eq!(h.resources.vcpus, 4);
        assert!(h.underlay_ip.is_none());
    }

    #[test]
    fn result_helpers() {
        let id = Uuid::now_v7();
        let r = CommandResult::failed(id, "boom").with_payload(serde_json::json!({"code": 7}));
        assert!(!r.ok);
        let text = serde_json::to_string(&r).unwrap();
        let back: CommandResult = serde_json::from_str(&text).unwrap();
        assert_eq!(back, r);
        let ok = CommandResult::ok(id);
        assert!(!serde_json::to_string(&ok).unwrap().contains("message"));
    }
}
