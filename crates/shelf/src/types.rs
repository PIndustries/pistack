// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Share resources. `spec` is the request. NFS is [`Phase::Ready`] only
//! after the managed exports file is loaded with `exportfs -ra`. CephFS is
//! [`Phase::Ready`] only after `ceph` creates the subvolume. An NFS export
//! that fails is [`Phase::Error`]; the reconcile loop retries it.

use chrono::{DateTime, Utc};
use pistack_types::{kind, ApiError, ApiResult, Phase, Resource, Spec};
use serde::{Deserialize, Serialize};

/// Status message before a share is realized.
pub const PENDING_MESSAGE: &str = "this build has no real NFS/Ceph daemon";

/// `nfs` or `cephfs`. Stored lowercase after validation.
pub const PROTOCOL_NFS: &str = "nfs";
pub const PROTOCOL_CEPHFS: &str = "cephfs";

/// Who may mount a share, and how.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShareAccess {
    /// Principal name or id. Set this or [`Self::cidr`] (or both).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// Client CIDR, for example `10.0.0.0/24`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cidr: Option<String>,
    pub mode: AccessMode,
}

/// Read-only or read-write mount.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    Read,
    Write,
}

/// What the user asked for. `name` mirrors `meta.name`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShareSpec {
    #[serde(default)]
    pub name: String,
    pub protocol: String,
    pub size_gib: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<ShareAccess>,
}

impl Spec for ShareSpec {
    const KIND: &'static str = kind::SHARE;
}

impl ShareSpec {
    /// Canonicalize protocol and access. Rejects an unknown protocol,
    /// `size_gib == 0`, and an access entry with neither principal nor cidr.
    pub fn validate(&mut self) -> ApiResult<()> {
        self.protocol = self.protocol.trim().to_ascii_lowercase();
        match self.protocol.as_str() {
            PROTOCOL_NFS | PROTOCOL_CEPHFS => {}
            other => {
                return Err(ApiError::bad_request(format!(
                    "unknown protocol '{other}'; expected \"{PROTOCOL_NFS}\" or \"{PROTOCOL_CEPHFS}\""
                )));
            }
        }
        if self.size_gib == 0 {
            return Err(ApiError::bad_request("size_gib must be greater than 0"));
        }
        for (i, rule) in self.access.iter_mut().enumerate() {
            let principal = nonempty(rule.principal.as_deref());
            let cidr = nonempty(rule.cidr.as_deref());
            if principal.is_none() && cidr.is_none() {
                return Err(ApiError::bad_request(format!(
                    "access[{i}] requires a principal or a cidr"
                )));
            }
            rule.principal = principal;
            rule.cidr = cidr;
        }
        Ok(())
    }
}

fn nonempty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Observed state. `Ready` means the share is exported or the subvolume exists.
/// `Error` keeps the last NFS failure in [`Self::message`] until a later pass
/// succeeds.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShareStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub message: String,
    /// Failures since the last `Ready`. Drives retry backoff.
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub failures: u32,
    /// Earliest time a failed NFS share may be retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<DateTime<Utc>>,
    /// Revision this status describes. A pass that finds this equal to
    /// `meta.revision` does not run again until [`Self::retry_at`].
    #[serde(default)]
    pub observed_revision: u64,
}

fn u32_is_zero(n: &u32) -> bool {
    *n == 0
}

impl Default for ShareStatus {
    fn default() -> Self {
        Self {
            phase: Phase::Pending,
            message: PENDING_MESSAGE.to_owned(),
            failures: 0,
            retry_at: None,
            observed_revision: 0,
        }
    }
}

pub type Share = Resource<ShareSpec, ShareStatus>;

/// Pick the share name: an explicit request name, else `spec.name`, else
/// `current` (the stored name on update). Writes the winner back onto `spec`.
pub fn assign_name(
    spec: &mut ShareSpec,
    top: Option<&str>,
    current: Option<&str>,
) -> ApiResult<()> {
    let top = top.map(str::trim).filter(|s| !s.is_empty());
    let from_spec = nonempty(Some(spec.name.as_str()));
    let chosen = top
        .map(str::to_owned)
        .or(from_spec)
        .or_else(|| current.map(str::to_owned))
        .unwrap_or_default();
    spec.name = validate_name(&chosen)?;
    Ok(())
}

pub fn validate_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters and must not contain '/'",
        ));
    }
    Ok(name.to_owned())
}
