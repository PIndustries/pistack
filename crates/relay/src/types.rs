// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The load-balancer resource model: `Balancer`, `Listener`, `Pool`,
//! `Member`.
//!
//! A balancer owns a VIP. Listeners bind a protocol and port on it and
//! point at a default pool. A pool holds members and a health monitor.
//! `spec` is the request. `status` is what the provider reported.

use crate::door::DoorKind;
use pistack_types::{kind, Id, Kind, Meta, Phase, Resource};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Define a resource struct with the `Resource<Spec, Status>` wire shape
/// (`meta` flattened, then `spec` and `status`). A named struct rather than
/// the alias because the orphan rule forbids `impl Kind for Resource<..>`
/// outside `pistack-types`; `From` both ways keeps them interchangeable.
macro_rules! resource {
    ($(#[$doc:meta])* $t:ident, $spec:ty, $status:ty, $k:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        pub struct $t {
            #[serde(flatten)]
            pub meta: Meta,
            pub spec: $spec,
            pub status: $status,
        }
        impl Kind for $t {
            const KIND: &'static str = $k;
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.meta
            }
        }
        impl From<Resource<$spec, $status>> for $t {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
        impl From<$t> for Resource<$spec, $status> {
            fn from(r: $t) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
    };
}

/// Id kinds relay owns beyond the shared `lb`.
pub mod kinds {
    pub const LISTENER: &str = "lsn";
    pub const POOL: &str = "pool";
    pub const MEMBER: &str = "mbr";
}

/// Every kind relay owns, for the reconciler's event filter.
pub const KINDS: &[&str] = &[kind::BALANCER, kinds::LISTENER, kinds::POOL, kinds::MEMBER];

/// Label relay puts on balancers it creates for other modules.
pub const MANAGED_LABEL: &str = "pistack.io/managed-by";

// ---- Balancer ----------------------------------------------------------------

/// Which data plane realises the balancer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// L4 TCP and UDP. The in-process forwarder serves this value. The default.
    /// Older rows and documents that say `ovn` read as this value.
    #[default]
    #[serde(alias = "ovn")]
    Forward,
    /// HTTP. The in-process reverse proxy serves this value.
    Relay,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BalancerSpec {
    /// Network (id or name) the VIP lives on.
    pub vip_network: String,
    /// Explicit VIP; allocated by the provider when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vip_address: Option<IpAddr>,
    /// Bind this exact port on the VIP instead of an ephemeral one. Used
    /// for service node ports; a port already in use stays unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vip_port: Option<u16>,
    /// Floating IP (fabric id) to publish the VIP on, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floating_ip: Option<Id>,
    #[serde(default)]
    pub provider: Provider,
    #[serde(default = "default_true")]
    pub admin_up: bool,
}

fn default_true() -> bool {
    true
}

impl Default for BalancerSpec {
    fn default() -> Self {
        Self {
            vip_network: String::new(),
            vip_address: None,
            vip_port: None,
            floating_ip: None,
            provider: Provider::Forward,
            admin_up: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BalancerStatus {
    #[serde(default)]
    pub phase: Phase,
    /// The VIP as realised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vip: Option<IpAddr>,
    /// Provider-side handle (OVN `Load_Balancer` uuid, Pingora listener id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    /// A load balancer: one VIP, many listeners.
    Balancer, BalancerSpec, BalancerStatus, kind::BALANCER
);

// ---- Listener ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ListenerProtocol {
    Tcp,
    Udp,
    Http,
    /// HTTPS passed through to members untouched (L4).
    Https,
    /// TLS terminated at the balancer with a `keep` certificate.
    Terminated {
        cert: Id,
    },
}

impl ListenerProtocol {
    /// True for protocols only the L7 provider can serve.
    pub fn is_l7(&self) -> bool {
        matches!(
            self,
            ListenerProtocol::Http | ListenerProtocol::Terminated { .. }
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListenerSpec {
    pub balancer: Id,
    pub protocol: ListenerProtocol,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_pool: Option<Id>,
    /// Max concurrent connections; 0 means unlimited.
    #[serde(default)]
    pub connection_limit: u32,
    #[serde(default = "default_true")]
    pub admin_up: bool,
    /// Publish this listener off the host. Absent means the listener is
    /// only the local forwarder. WireGuard may create only a `pw` interface.
    /// Edge queues a publish and starts no process on this server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub door: Option<DoorKind>,
    /// HTTP routes, tried before the default pool. A request goes to the
    /// route whose host matches most specifically, then the longest
    /// matching path. When no route matches, the default pool serves it;
    /// without a default member the answer is 404. Only `Http` and
    /// `Terminated` listeners may carry routes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
}

/// How a route's path is compared with the request path.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathMatch {
    /// Whole path segments: `/app` matches `/app` and `/app/x`, not `/apps`.
    #[default]
    Prefix,
    /// The request path must equal `path`.
    Exact,
}

/// One host and path rule on an HTTP listener.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Route {
    /// Exact host name, or `*.example.com` for any single leading label.
    /// Absent matches every host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Starts with `/`.
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default)]
    pub path_match: PathMatch,
    /// Pool on the same balancer that takes matching requests.
    pub pool: Id,
}

fn root_path() -> String {
    "/".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ListenerStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    /// A protocol and port on a balancer's VIP.
    Listener, ListenerSpec, ListenerStatus, kinds::LISTENER
);

// ---- Pool --------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum PoolProtocol {
    #[default]
    Tcp,
    Udp,
    Http,
    Https,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    #[default]
    RoundRobin,
    LeastConn,
    SourceIp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionPersistence {
    SourceIp,
    HttpCookie,
    AppCookie { cookie: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum MonitorType {
    #[default]
    Tcp,
    Http,
    Https,
}

/// Active health check settings; absent means members are `NoMonitor`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthMonitor {
    #[serde(default)]
    pub kind: MonitorType,
    /// Seconds between probes.
    #[serde(default = "default_interval")]
    pub interval: u32,
    /// Seconds before a probe is a failure.
    #[serde(default = "default_timeout")]
    pub timeout: u32,
    /// Consecutive successes before Offline -> Online.
    #[serde(default = "default_rise")]
    pub rise: u32,
    /// Consecutive failures before Online -> Offline.
    #[serde(default = "default_fall")]
    pub fall: u32,
    /// HTTP(S) only: request path and accepted status codes.
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "default_codes")]
    pub expected_codes: Vec<u16>,
}

fn default_interval() -> u32 {
    5
}
fn default_timeout() -> u32 {
    3
}
fn default_rise() -> u32 {
    2
}
fn default_fall() -> u32 {
    3
}
fn default_path() -> String {
    "/".into()
}
fn default_codes() -> Vec<u16> {
    vec![200]
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self {
            kind: MonitorType::Tcp,
            interval: default_interval(),
            timeout: default_timeout(),
            rise: default_rise(),
            fall: default_fall(),
            path: default_path(),
            expected_codes: default_codes(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolSpec {
    pub balancer: Id,
    #[serde(default)]
    pub protocol: PoolProtocol,
    #[serde(default)]
    pub algorithm: Algorithm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_persistence: Option<SessionPersistence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_monitor: Option<HealthMonitor>,
    #[serde(default = "default_true")]
    pub admin_up: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PoolStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
    /// Members `Online` at the last check.
    #[serde(default)]
    pub online: u32,
    #[serde(default)]
    pub members: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    /// A set of members with a balancing algorithm and a health monitor.
    Pool, PoolSpec, PoolStatus, kinds::POOL
);

// ---- Member ------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberSpec {
    pub pool: Id,
    pub address: IpAddr,
    pub port: u16,
    #[serde(default = "default_weight")]
    pub weight: u16,
    /// Serve existing connections but take no new ones.
    #[serde(default)]
    pub backup: bool,
    #[serde(default = "default_true")]
    pub admin_up: bool,
}

fn default_weight() -> u16 {
    1
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatingStatus {
    Online,
    Offline,
    /// Administratively down or being removed; finishing in-flight work.
    Draining,
    /// The pool has no health monitor; status is unknown and traffic flows.
    #[default]
    NoMonitor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MemberStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub operating_status: OperatingStatus,
    /// Consecutive probe results in the current direction, for rise/fall.
    #[serde(default)]
    pub streak: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    /// One backend address:port in a pool.
    Member, MemberSpec, MemberStatus, kinds::MEMBER
);

impl Member {
    pub fn addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.spec.address, self.spec.port)
    }
}
