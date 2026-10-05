// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `pi` command tree (clap derive).

use super::output::Output;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "pi",
    version,
    about = "PiStack command-line client",
    long_about = "pi talks to a PiStack control plane. Log in once with `pi login URL --name USER`; \
                  the token is kept in ~/.config/pistack/config.toml under a named profile.",
    propagate_version = true
)]
pub struct Cli {
    /// Config file (default ~/.config/pistack/config.toml).
    #[arg(long, global = true, env = "PI_CONFIG", value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Profile to use (default: the one `pi profile use` picked).
    #[arg(long, global = true, env = "PI_PROFILE", value_name = "NAME")]
    pub profile: Option<String>,
    /// API URL; overrides the profile.
    #[arg(long, global = true, env = "PI_URL", value_name = "URL")]
    pub url: Option<String>,
    /// Bearer token; overrides the profile.
    #[arg(
        long,
        global = true,
        env = "PI_TOKEN",
        hide_env_values = true,
        value_name = "TOKEN"
    )]
    pub token: Option<String>,
    /// Project scope; overrides the profile.
    #[arg(long, global = true, env = "PI_PROJECT", value_name = "PROJECT")]
    pub project: Option<String>,
    /// Skip TLS certificate verification.
    #[arg(long, global = true)]
    pub insecure: bool,
    /// Output format.
    #[arg(short = 'o', long, global = true, value_enum, default_value_t = Output::Table)]
    pub output: Output,
    /// Omit the header row in table output.
    #[arg(long, global = true)]
    pub no_headers: bool,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Log in with a password and store the token in a profile.
    Login(LoginArgs),
    /// Manage named connection profiles.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Show who the current token is.
    Whoami,
    /// List the modules the server has mounted.
    Modules,
    /// Print client and server versions.
    Version,
    /// Make a raw API request: `pi api GET /compute/machines`.
    Api(ApiArgs),
    /// Create or update resources from a YAML/JSON manifest.
    Apply(ApplyArgs),
    /// Shell completion script.
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Projects (tenants).
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },
    /// Users and service accounts.
    User {
        #[command(subcommand)]
        cmd: UserCmd,
    },
    /// Roles.
    Role {
        #[command(subcommand)]
        cmd: RoleCmd,
    },
    /// Role bindings.
    Binding {
        #[command(subcommand)]
        cmd: BindingCmd,
    },
    /// API keys.
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// Machines: VMs, containers, bare metal.
    Machine {
        #[command(subcommand)]
        cmd: MachineCmd,
    },
    /// Sizes (flavors).
    Size {
        #[command(subcommand)]
        cmd: SizeCmd,
    },
    /// Hosts (admin).
    Host {
        #[command(subcommand)]
        cmd: HostCmd,
    },
    /// Networks.
    Network {
        #[command(subcommand)]
        cmd: NetworkCmd,
    },
    /// Subnets.
    Subnet {
        #[command(subcommand)]
        cmd: SubnetCmd,
    },
    /// Ports.
    Port {
        #[command(subcommand)]
        cmd: PortCmd,
    },
    /// Routers.
    Router {
        #[command(subcommand)]
        cmd: RouterCmd,
    },
    /// Security groups.
    Secgroup {
        #[command(subcommand)]
        cmd: SecgroupCmd,
    },
    /// Floating IPs.
    Fip {
        #[command(subcommand)]
        cmd: FipCmd,
    },
    /// Block volumes.
    Volume {
        #[command(subcommand)]
        cmd: VolumeCmd,
    },
    /// Volume snapshots.
    Snapshot {
        #[command(subcommand)]
        cmd: SnapshotCmd,
    },
    /// Volume types.
    Voltype {
        #[command(subcommand)]
        cmd: VoltypeCmd,
    },
    /// Images.
    Image {
        #[command(subcommand)]
        cmd: ImageCmd,
    },
    /// StatefulSets (loom).
    Statefulset {
        #[command(subcommand)]
        cmd: StatefulsetCmd,
    },
    /// DaemonSets (loom).
    Daemonset {
        #[command(subcommand)]
        cmd: DaemonsetCmd,
    },
    /// Autoscalers (loom).
    Autoscaler {
        #[command(subcommand)]
        cmd: AutoscalerCmd,
    },
    /// Stacks (blueprint).
    Stack {
        #[command(subcommand)]
        cmd: StackCmd,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Elvish,
    Powershell,
}

impl From<Shell> for clap_complete::Shell {
    fn from(s: Shell) -> Self {
        match s {
            Shell::Bash => clap_complete::Shell::Bash,
            Shell::Zsh => clap_complete::Shell::Zsh,
            Shell::Fish => clap_complete::Shell::Fish,
            Shell::Elvish => clap_complete::Shell::Elvish,
            Shell::Powershell => clap_complete::Shell::PowerShell,
        }
    }
}

// ---- shared argument groups --------------------------------------------------

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// API URL, e.g. https://cloud.example:8443
    pub url: String,
    /// Principal name.
    #[arg(long)]
    pub name: String,
    /// Read the password from stdin instead of prompting.
    #[arg(long)]
    pub password_stdin: bool,
    /// Project to scope the token to (name or id).
    #[arg(long = "scope-project", value_name = "PROJECT")]
    pub scope_project: Option<String>,
    /// Token lifetime in seconds.
    #[arg(long)]
    pub ttl: Option<i64>,
}

#[derive(Subcommand, Debug)]
pub enum ProfileCmd {
    /// List profiles.
    List,
    /// Make a profile current.
    Use { name: String },
    /// Show a profile (default: current).
    Show { name: Option<String> },
    /// Remove a profile.
    Delete { name: String },
}

#[derive(Args, Debug)]
pub struct ApiArgs {
    /// HTTP method.
    pub method: String,
    /// Path (e.g. /compute/machines) or full URL.
    pub path: String,
    /// Request body: inline JSON or @file.
    #[arg(short = 'd', long = "data")]
    pub data: Option<String>,
}

#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// Manifest file (YAML or JSON, multi-document ok); `-` for stdin.
    #[arg(short = 'f', long = "file", required = true)]
    pub files: Vec<String>,
}

#[derive(Args, Debug, Default)]
pub struct ListArgs {
    /// Every project (admin).
    #[arg(long)]
    pub all: bool,
    /// Label selector `key=value` (repeatable).
    #[arg(short = 'l', long = "selector", value_name = "K=V")]
    pub selector: Vec<String>,
}

#[derive(Args, Debug)]
pub struct NameArg {
    /// Name or id.
    pub name: String,
}

#[derive(Args, Debug)]
pub struct DeleteArgs {
    /// Name or id.
    pub name: String,
    /// Do not ask for confirmation.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Wait until the resource is gone.
    #[arg(long)]
    pub wait: bool,
}

/// Metadata every create accepts.
#[derive(Args, Debug, Default)]
pub struct MetaArgs {
    #[arg(long)]
    pub description: Option<String>,
    /// Label `key=value` (repeatable).
    #[arg(long = "label", value_name = "K=V")]
    pub labels: Vec<String>,
}

macro_rules! crud {
    ($name:ident, $create:ty $(, $(#[$m:meta])* $var:ident($args:ty))* $(,)?) => {
        #[derive(Subcommand, Debug)]
        #[allow(clippy::large_enum_variant)]
        pub enum $name {
            /// List.
            #[command(alias = "ls")]
            List(ListArgs),
            /// Show one by name or id.
            #[command(alias = "get")]
            Show(NameArg),
            /// Create.
            Create($create),
            /// Delete by name or id.
            #[command(alias = "rm")]
            Delete(DeleteArgs),
            $( $(#[$m])* $var($args), )*
        }
    };
}

// ---- identity ----------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ProjectCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Parent project (name or id).
    #[arg(long)]
    pub parent: Option<String>,
    /// Quota `resource=limit` (repeatable).
    #[arg(long = "quota", value_name = "RES=N")]
    pub quotas: Vec<String>,
    #[arg(long)]
    pub disabled: bool,
}
crud!(ProjectCmd, ProjectCreate);

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum PrincipalKind {
    #[default]
    User,
    ServiceAccount,
}

#[derive(Args, Debug)]
pub struct UserCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    #[arg(long, value_enum, default_value_t = PrincipalKind::User)]
    pub kind: PrincipalKind,
    #[arg(long)]
    pub email: Option<String>,
    /// Password (prefer --password-stdin).
    #[arg(long, conflicts_with = "password_stdin")]
    pub password: Option<String>,
    /// Read the password from stdin.
    #[arg(long)]
    pub password_stdin: bool,
    /// Default project (name or id).
    #[arg(long)]
    pub default_project: Option<String>,
    #[arg(long)]
    pub disabled: bool,
}
crud!(UserCmd, UserCreate);

#[derive(Args, Debug)]
pub struct RoleCreate {
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
}
crud!(RoleCmd, RoleCreate);

#[derive(Args, Debug)]
pub struct BindingCreate {
    /// Principal (name or id).
    #[arg(long)]
    pub principal: String,
    /// Role name.
    #[arg(long)]
    pub role: String,
    /// Project (name or id); omit for system scope.
    #[arg(long = "in-project", value_name = "PROJECT")]
    pub in_project: Option<String>,
}
crud!(BindingCmd, BindingCreate);

#[derive(Args, Debug)]
pub struct KeyCreate {
    pub name: String,
    /// Principal (name or id); default: you.
    #[arg(long)]
    pub principal: Option<String>,
    /// Project the key's tokens are scoped to (name or id).
    #[arg(long = "scope-project", value_name = "PROJECT")]
    pub scope_project: Option<String>,
    /// Expiry, RFC 3339.
    #[arg(long)]
    pub expires_at: Option<String>,
}
crud!(KeyCmd, KeyCreate);

// ---- compute -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum MachineKind {
    #[default]
    Vm,
    Container,
    Metal,
}

#[derive(Args, Debug)]
pub struct MachineCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Size name, e.g. m.2x4.
    #[arg(long)]
    pub size: String,
    /// Image name.
    #[arg(long)]
    pub image: String,
    /// Network (repeatable).
    #[arg(long = "network", value_name = "NETWORK")]
    pub networks: Vec<String>,
    /// Volume (repeatable).
    #[arg(long = "volume", value_name = "VOLUME")]
    pub volumes: Vec<String>,
    #[arg(long, value_enum, default_value_t = MachineKind::Vm)]
    pub kind: MachineKind,
    /// SSH public key (repeatable); `@file` reads a file.
    #[arg(long = "ssh-key", value_name = "KEY")]
    pub ssh_keys: Vec<String>,
    /// cloud-init user data; `@file` reads a file.
    #[arg(long)]
    pub user_data: Option<String>,
    /// Host selector `key=value` (repeatable).
    #[arg(long = "host-selector", value_name = "K=V")]
    pub host_selector: Vec<String>,
    /// Wait until the machine is ready (or failed).
    #[arg(long)]
    pub wait: bool,
}

#[derive(Args, Debug)]
pub struct PowerArgs {
    pub name: String,
    #[arg(long)]
    pub wait: bool,
}

crud!(
    MachineCmd,
    MachineCreate,
    /// Power on.
    Start(PowerArgs),
    /// Power off.
    Stop(PowerArgs),
    /// Reboot.
    Reboot(PowerArgs),
);

#[derive(Args, Debug)]
pub struct SizeCreate {
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long)]
    pub vcpus: u32,
    /// Memory in MiB.
    #[arg(long = "memory", value_name = "MIB")]
    pub memory_mib: u64,
    /// Root disk in GiB.
    #[arg(long = "disk", value_name = "GIB", default_value_t = 0)]
    pub disk_gib: u64,
    #[arg(long, default_value_t = 0)]
    pub gpus: u32,
}
crud!(SizeCmd, SizeCreate);

#[derive(Subcommand, Debug)]
pub enum HostCmd {
    /// List hosts.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Show a host.
    #[command(alias = "get")]
    Show(NameArg),
}

// ---- network -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum NetworkKind {
    #[default]
    Overlay,
    Provider,
}

#[derive(Args, Debug)]
pub struct NetworkCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    #[arg(long, value_enum, default_value_t = NetworkKind::Overlay)]
    pub kind: NetworkKind,
    /// VLAN tag (provider networks).
    #[arg(long)]
    pub vlan: Option<u16>,
    /// Physical network name (provider networks).
    #[arg(long, default_value = "physnet1")]
    pub physnet: String,
    #[arg(long, default_value_t = 1500)]
    pub mtu: u16,
    /// External network: routers get gateways and floating IPs here.
    #[arg(long)]
    pub external: bool,
    #[arg(long)]
    pub dns_domain: Option<String>,
}
crud!(NetworkCmd, NetworkCreate);

#[derive(Args, Debug)]
pub struct SubnetCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Network (name or id).
    #[arg(long)]
    pub network: String,
    /// CIDR, e.g. 10.1.0.0/24.
    #[arg(long)]
    pub cidr: String,
    #[arg(long)]
    pub gateway: Option<String>,
    /// DNS nameserver (repeatable).
    #[arg(long = "dns", value_name = "IP")]
    pub dns: Vec<String>,
    /// Allocation pool `start-end` (repeatable).
    #[arg(long = "pool", value_name = "START-END")]
    pub pools: Vec<String>,
    #[arg(long)]
    pub no_dhcp: bool,
}
crud!(SubnetCmd, SubnetCreate);

#[derive(Args, Debug)]
pub struct PortCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Network (name or id).
    #[arg(long)]
    pub network: String,
    /// Subnet (name or id).
    #[arg(long)]
    pub subnet: Option<String>,
    /// Fixed IP (repeatable).
    #[arg(long = "ip", value_name = "IP")]
    pub ips: Vec<String>,
    #[arg(long)]
    pub mac: Option<String>,
    /// Security group (repeatable, name or id).
    #[arg(long = "secgroup", value_name = "SG")]
    pub secgroups: Vec<String>,
    /// Machine to plug into (name or id).
    #[arg(long)]
    pub machine: Option<String>,
    #[arg(long)]
    pub no_port_security: bool,
}
crud!(PortCmd, PortCreate);

#[derive(Args, Debug)]
pub struct RouterCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// External network for the gateway (name or id).
    #[arg(long)]
    pub external_network: Option<String>,
    /// Subnet to attach (repeatable, name or id).
    #[arg(long = "subnet", value_name = "SUBNET")]
    pub subnets: Vec<String>,
}

#[derive(Args, Debug)]
pub struct RouterInterfaces {
    pub name: String,
    /// Subnet (repeatable, name or id). Replaces the current set.
    #[arg(long = "subnet", value_name = "SUBNET")]
    pub subnets: Vec<String>,
}

crud!(
    RouterCmd,
    RouterCreate,
    /// Replace the router's subnet interfaces.
    SetInterfaces(RouterInterfaces),
);

#[derive(Args, Debug)]
pub struct SecgroupCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Direction {
    Ingress,
    Egress,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum Proto {
    Tcp,
    Udp,
    Icmp,
    #[default]
    Any,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum EtherType {
    #[default]
    Ipv4,
    Ipv6,
}

#[derive(Args, Debug)]
pub struct RuleAdd {
    pub name: String,
    #[arg(long, value_enum)]
    pub direction: Direction,
    #[arg(long, value_enum, default_value_t = Proto::Any)]
    pub proto: Proto,
    /// Port or range: `22` or `8000-9000`.
    #[arg(long)]
    pub port: Option<String>,
    /// Remote CIDR.
    #[arg(long, conflicts_with = "remote_group")]
    pub cidr: Option<String>,
    /// Remote security group (name or id).
    #[arg(long)]
    pub remote_group: Option<String>,
    #[arg(long, value_enum, default_value_t = EtherType::Ipv4)]
    pub ethertype: EtherType,
}

#[derive(Args, Debug)]
pub struct RuleRemove {
    pub name: String,
    /// Rule id (see `pi secgroup show`).
    pub rule: String,
}

crud!(
    SecgroupCmd,
    SecgroupCreate,
    /// Add a rule.
    AddRule(RuleAdd),
    /// Remove a rule by id.
    RemoveRule(RuleRemove),
);

#[derive(Args, Debug)]
pub struct FipCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// External network (name or id).
    #[arg(long)]
    pub network: String,
    /// Specific address (default: allocate).
    #[arg(long)]
    pub address: Option<String>,
    /// Port to associate at once (name or id).
    #[arg(long)]
    pub port: Option<String>,
}

#[derive(Args, Debug)]
pub struct FipAssociate {
    pub name: String,
    /// Port (name or id).
    #[arg(long, required_unless_present = "machine", conflicts_with = "machine")]
    pub port: Option<String>,
    /// Machine (name or id). Uses that guest's current IPv4.
    #[arg(long, required_unless_present = "port")]
    pub machine: Option<String>,
}

crud!(
    FipCmd,
    FipCreate,
    /// Associate with a machine or a port.
    Associate(FipAssociate),
    /// Disassociate.
    Disassociate(NameArg),
);

// ---- volume ------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct VolumeCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Size in GiB.
    #[arg(long = "size", value_name = "GIB")]
    pub size_gib: u64,
    /// Volume type.
    #[arg(long = "type", default_value = "standard")]
    pub volume_type: String,
    /// Populate from an image (name or id).
    #[arg(long, conflicts_with_all = ["snapshot", "source_volume"])]
    pub image: Option<String>,
    /// Restore from a snapshot (name or id).
    #[arg(long, conflicts_with = "source_volume")]
    pub snapshot: Option<String>,
    /// Clone another volume (name or id).
    #[arg(long)]
    pub source_volume: Option<String>,
    #[arg(long)]
    pub bootable: bool,
    #[arg(long)]
    pub encrypted: bool,
    #[arg(long)]
    pub wait: bool,
}

#[derive(Args, Debug)]
pub struct VolumeAttach {
    pub name: String,
    /// Machine (name or id).
    #[arg(long)]
    pub machine: String,
    /// Guest device name, e.g. vdb.
    #[arg(long)]
    pub device: Option<String>,
}

#[derive(Args, Debug)]
pub struct VolumeExtend {
    pub name: String,
    /// New size in GiB.
    #[arg(long = "size", value_name = "GIB")]
    pub size_gib: u64,
}

crud!(
    VolumeCmd,
    VolumeCreate,
    /// Attach to a machine.
    Attach(VolumeAttach),
    /// Detach.
    Detach(NameArg),
    /// Grow.
    Extend(VolumeExtend),
);

#[derive(Args, Debug)]
pub struct SnapshotCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Volume (name or id).
    #[arg(long)]
    pub volume: String,
}
crud!(SnapshotCmd, SnapshotCreate);

#[derive(Args, Debug)]
pub struct VoltypeCreate {
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
    /// Backend: local, lvm, rbd.
    #[arg(long, default_value = "local")]
    pub backend: String,
    /// Backend parameter `key=value` (repeatable).
    #[arg(long = "param", value_name = "K=V")]
    pub params: Vec<String>,
}
crud!(VoltypeCmd, VoltypeCreate);

// ---- image -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ImageFormat {
    Raw,
    Qcow2,
    Oci,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum Visibility {
    #[default]
    Private,
    Project,
    Public,
}

#[derive(Args, Debug)]
pub struct ImageCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    #[arg(long, value_enum)]
    pub format: ImageFormat,
    #[arg(long)]
    pub os: Option<String>,
    #[arg(long, default_value = "x86_64")]
    pub arch: String,
    #[arg(long = "min-disk", value_name = "GIB", default_value_t = 0)]
    pub min_disk_gib: u64,
    #[arg(long = "min-ram", value_name = "MIB", default_value_t = 0)]
    pub min_ram_mib: u64,
    #[arg(long, value_enum, default_value_t = Visibility::Private)]
    pub visibility: Visibility,
    /// Import from a URL after creating.
    #[arg(long, conflicts_with = "file")]
    pub source_url: Option<String>,
    /// Upload this file after creating.
    #[arg(long)]
    pub file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ImageUpload {
    pub name: String,
    #[arg(long)]
    pub file: PathBuf,
}

#[derive(Args, Debug)]
pub struct ImageDownload {
    pub name: String,
    /// Output file (default: <name>.<format>).
    #[arg(short = 'O', long = "out", value_name = "FILE")]
    pub out: Option<PathBuf>,
}

crud!(
    ImageCmd,
    ImageCreate,
    /// Upload image data.
    Upload(ImageUpload),
    /// Download image data.
    Download(ImageDownload),
);

// ---- loom / blueprint --------------------------------------------------------

#[derive(Args, Debug)]
pub struct StatefulsetCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Container image.
    #[arg(long)]
    pub image: String,
    /// Container name (default: the StatefulSet name).
    #[arg(long)]
    pub container: Option<String>,
    /// Desired replicas.
    #[arg(long, default_value_t = 1)]
    pub replicas: u32,
    /// Service that gives pods a stable network identity.
    #[arg(long = "service-name")]
    pub service_name: Option<String>,
}

#[derive(Args, Debug)]
pub struct ScaleArgs {
    pub name: String,
    #[arg(long)]
    pub replicas: u32,
}

crud!(
    StatefulsetCmd,
    StatefulsetCreate,
    /// Scale replicas.
    Scale(ScaleArgs),
);

#[derive(Args, Debug)]
pub struct DaemonsetCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Container image.
    #[arg(long)]
    pub image: String,
    /// Container name (default: the DaemonSet name).
    #[arg(long)]
    pub container: Option<String>,
}
crud!(DaemonsetCmd, DaemonsetCreate);

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ScaleTargetKindArg {
    Deployment,
    StatefulSet,
}

#[derive(Args, Debug)]
pub struct AutoscalerCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Workload kind to scale.
    #[arg(long = "target-kind", value_enum)]
    pub target_kind: ScaleTargetKindArg,
    /// Target Deployment or StatefulSet (name or id).
    #[arg(long = "target")]
    pub target: String,
    /// Gauge metric name.
    #[arg(long)]
    pub metric: String,
    /// Target metric value.
    #[arg(long = "target-value")]
    pub target_value: f64,
    /// Minimum replicas.
    #[arg(long = "min", default_value_t = 1)]
    pub min_replicas: u32,
    /// Maximum replicas.
    #[arg(long = "max")]
    pub max_replicas: u32,
}
crud!(AutoscalerCmd, AutoscalerCreate);

#[derive(Args, Debug)]
pub struct StackCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Stack document (YAML/JSON text); `@file` reads a file, `-` reads stdin.
    #[arg(long, short = 'd')]
    pub document: String,
}

#[derive(Args, Debug)]
pub struct StackUpdate {
    /// Name or id.
    pub name: String,
    /// Replacement document; `@file` or `-` for stdin.
    #[arg(long, short = 'd')]
    pub document: String,
}

#[derive(Args, Debug)]
pub struct StackRollback {
    /// Name or id.
    pub name: String,
    /// Revision from status.history (default: previous).
    #[arg(long)]
    pub revision: Option<u64>,
}

crud!(
    StackCmd,
    StackCreate,
    /// Store a new document (honours revision / If-Match).
    Update(StackUpdate),
    /// Apply the stored document.
    Apply(NameArg),
    /// Re-apply an earlier revision.
    Rollback(StackRollback),
);
