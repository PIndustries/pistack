// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The resource model shared by every module.
//!
//! Every object a user can create (a machine, a network, a volume, a pod)
//! is a `Resource<Spec, Status>`: common `Meta` plus a module-owned `spec`
//! (what the user asked for) and `status` (what is true right now). This is
//! deliberately the Kubernetes shape, so `loom` can speak the k8s API later
//! without a translation layer, and so every module gets list/get/watch for
//! free from `pistack-store`.

use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use uuid::Uuid;

/// Every id is a UUIDv7 (time-ordered) rendered with a short kind prefix,
/// e.g. `mach-018f...`, `proj-018f...`. The prefix makes logs readable and
/// stops a volume id being passed where a machine id was meant.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id {
    pub kind: &'static str,
    pub uuid: Uuid,
}

impl Id {
    pub fn new(kind: &'static str) -> Self {
        Self { kind, uuid: Uuid::now_v7() }
    }
    pub fn parse(kind: &'static str, s: &str) -> Option<Self> {
        let rest = s.strip_prefix(kind)?.strip_prefix('-')?;
        Uuid::parse_str(rest).ok().map(|uuid| Self { kind, uuid })
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.kind, self.uuid)
    }
}
impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let (kind, rest) = s
            .split_once('-')
            .ok_or_else(|| serde::de::Error::custom("id must be <kind>-<uuid>"))?;
        let uuid = Uuid::parse_str(rest).map_err(serde::de::Error::custom)?;
        // Leak the kind so we can hold a &'static str. Kinds are a small closed set.
        let kind: &'static str = Box::leak(kind.to_owned().into_boxed_str());
        Ok(Self { kind, uuid })
    }
}

/// Well-known id kinds. Modules add their own; these are the cross-module ones.
pub mod kind {
    pub const PROJECT: &str = "proj";
    pub const PRINCIPAL: &str = "prin";
    pub const ROLE: &str = "role";
    pub const TOKEN: &str = "tok";
    pub const HOST: &str = "host";
    pub const MACHINE: &str = "mach";
    pub const POD: &str = "pod";
    pub const NETWORK: &str = "net";
    pub const SUBNET: &str = "subnet";
    pub const PORT: &str = "port";
    pub const ROUTER: &str = "rtr";
    pub const SECGROUP: &str = "sg";
    pub const FLOATING_IP: &str = "fip";
    pub const VOLUME: &str = "vol";
    pub const SNAPSHOT: &str = "snap";
    pub const IMAGE: &str = "img";
    pub const BUCKET: &str = "bkt";
    pub const SHARE: &str = "share";
    pub const ZONE: &str = "zone";
    pub const RECORD: &str = "rec";
    pub const BALANCER: &str = "lb";
    pub const SECRET: &str = "sec";
    pub const CERT: &str = "cert";
    pub const STACK: &str = "stack";
    pub const ALARM: &str = "alarm";
}

/// Metadata every resource carries.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub id: Id,
    /// Project (tenant) that owns the resource. `None` only for cloud-scoped
    /// things like hosts and the admin project itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<Id>,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Monotonic per-resource version; every write bumps it. Used for
    /// optimistic concurrency (`If-Match`) and watch ordering.
    pub revision: u64,
}

impl Meta {
    pub fn new(kind: &'static str, name: impl Into<String>, project: Option<Id>) -> Self {
        let now = Utc::now();
        Self {
            id: Id::new(kind),
            project,
            name: name.into(),
            description: String::new(),
            labels: BTreeMap::new(),
            annotations: BTreeMap::new(),
            created_at: now,
            updated_at: now,
            revision: 1,
        }
    }
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
        self.revision += 1;
    }
}

/// The envelope. `spec` is what was asked for; `status` is observed state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Resource<S, T> {
    #[serde(flatten)]
    pub meta: Meta,
    pub spec: S,
    pub status: T,
}

/// Marker trait a module implements for each of its resource kinds so the
/// store can key it. Keep it tiny; behaviour lives in the module.
pub trait Kind: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    /// Table name in the store and id prefix, e.g. "mach".
    const KIND: &'static str;
    fn meta(&self) -> &Meta;
    fn meta_mut(&mut self) -> &mut Meta;
}

/// Lifecycle phase shared by workloads, volumes, balancers and anything
/// else that is "being made", "ready" or "broken".
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Pending,
    Creating,
    Ready,
    Updating,
    Deleting,
    Error,
}

/// A condition, Kubernetes-style: one line of "what is true and why".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Condition {
    #[serde(rename = "type")]
    pub kind: String,
    pub status: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub last_transition: DateTime<Utc>,
}

/// Paged list response used by every list endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    pub total: usize,
}

/// Common list query parameters.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub after: Option<String>,
    /// `labels=env=prod,tier=web`
    #[serde(default)]
    pub labels: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl ListQuery {
    pub fn label_selector(&self) -> BTreeMap<String, String> {
        self.labels
            .as_deref()
            .unwrap_or("")
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .collect()
    }
}

/// Who is making the request, as resolved by `gate` and attached to every
/// request by the gateway. Modules read this; only `gate` constructs it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Caller {
    pub principal: Id,
    pub principal_name: String,
    /// Project scope of the token, if any.
    pub project: Option<Id>,
    pub roles: Vec<String>,
    /// System-scoped (cloud admin) token.
    pub system: bool,
}

impl Caller {
    pub fn has_role(&self, r: &str) -> bool {
        self.roles.iter().any(|x| x == r)
    }
    pub fn is_admin(&self) -> bool {
        self.system || self.has_role("admin")
    }
}

/// Resource sizes used by forge, loom and the scheduler.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
pub struct Resources {
    pub vcpus: u32,
    pub memory_mib: u64,
    #[serde(default)]
    pub disk_gib: u64,
    #[serde(default)]
    pub gpus: u32,
}

impl Resources {
    pub fn fits_in(&self, free: &Resources) -> bool {
        self.vcpus <= free.vcpus
            && self.memory_mib <= free.memory_mib
            && self.disk_gib <= free.disk_gib
            && self.gpus <= free.gpus
    }
    pub fn saturating_sub(&self, other: &Resources) -> Resources {
        Resources {
            vcpus: self.vcpus.saturating_sub(other.vcpus),
            memory_mib: self.memory_mib.saturating_sub(other.memory_mib),
            disk_gib: self.disk_gib.saturating_sub(other.disk_gib),
            gpus: self.gpus.saturating_sub(other.gpus),
        }
    }
    pub fn add(&self, other: &Resources) -> Resources {
        Resources {
            vcpus: self.vcpus + other.vcpus,
            memory_mib: self.memory_mib + other.memory_mib,
            disk_gib: self.disk_gib + other.disk_gib,
            gpus: self.gpus + other.gpus,
        }
    }
}

/// An event emitted on the bus whenever a resource changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub kind: String,
    pub id: Id,
    pub project: Option<Id>,
    pub action: EventAction,
    pub revision: u64,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventAction {
    Created,
    Updated,
    Deleted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip() {
        let id = Id::new(kind::MACHINE);
        let s = id.to_string();
        assert!(s.starts_with("mach-"));
        let back: Id = serde_json::from_str(&format!("\"{s}\"")).unwrap();
        assert_eq!(back, id);
        assert!(Id::parse(kind::MACHINE, &s).is_some());
        assert!(Id::parse(kind::VOLUME, &s).is_none());
    }

    #[test]
    fn resources_fit() {
        let want = Resources { vcpus: 2, memory_mib: 2048, disk_gib: 20, gpus: 0 };
        let free = Resources { vcpus: 8, memory_mib: 32768, disk_gib: 500, gpus: 0 };
        assert!(want.fits_in(&free));
        assert!(!free.fits_in(&want));
        assert_eq!(free.saturating_sub(&want).vcpus, 6);
    }

    #[test]
    fn label_selector_parses() {
        let q = ListQuery { labels: Some("env=prod, tier=web".into()), ..Default::default() };
        let sel = q.label_selector();
        assert_eq!(sel.get("env").unwrap(), "prod");
        assert_eq!(sel.get("tier").unwrap(), "web");
    }
}
