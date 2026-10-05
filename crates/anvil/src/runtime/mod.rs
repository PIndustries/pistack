// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Machine runtimes: the host-side counterpart of `forge::HostDriver`.
//!
//! `anvil` does not depend on `forge`. Both crates speak the wire types in
//! `pistack_rpc::proto`: `forge::RpcDriver` turns a `forge::Machine` into a
//! [`MachineRequest`] and a [`RunningMachine`] back into
//! `forge::MachineRuntime`; both are plain data and map one-to-one.

pub mod cloud;
pub mod libvirt;
mod metal;
pub mod mock;
pub mod youki;

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::BTreeMap;
use std::sync::Arc;

pub use cloud::CloudRuntime;
pub use libvirt::LibvirtRuntime;
pub use metal::MetalRuntime;
pub use mock::MockRuntime;
pub use youki::YoukiRuntime;

/// The request/result shapes are the wire types from `pistack-rpc`, so a
/// `Command` payload deserialises straight into what a runtime executes.
pub use pistack_rpc::proto::{
    MachineKind, MachineRef, MachineRequest, NicAttachment, PowerState, RunningMachine,
    VolumeAttach, VolumeDetach,
};

/// One per machine kind the host supports. Same semantics as
/// `forge::HostDriver`: `start` is idempotent, `delete` of an unknown id is
/// fine, `stop` of an unknown id is `not_found`.
#[async_trait]
pub trait MachineRuntime: Send + Sync + 'static {
    /// Short name reported in host capabilities ("mock", "libvirt", "youki").
    fn name(&self) -> &'static str;
    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine>;
    async fn stop(&self, id: &Id) -> ApiResult<()>;
    async fn delete(&self, id: &Id) -> ApiResult<()>;
    async fn list(&self) -> ApiResult<Vec<RunningMachine>>;

    /// Hotplug `req` into a machine this runtime holds. The returned string
    /// is the guest device name (`vdb`). Default: this runtime has no disks.
    async fn attach_volume(&self, req: &VolumeAttach) -> ApiResult<String> {
        let _ = req;
        Err(ApiError::not_implemented(format!(
            "{} does not attach volumes",
            self.name()
        )))
    }

    /// Unplug a volume. Missing disk is success. Default: nothing to detach.
    async fn detach_volume(&self, req: &VolumeDetach) -> ApiResult<()> {
        let _ = req;
        Err(ApiError::not_implemented(format!(
            "{} does not detach volumes",
            self.name()
        )))
    }
}

pub type SharedRuntime = Arc<dyn MachineRuntime>;

/// The runtimes one agent drives, one per machine kind it can run. The same
/// `Arc` may serve several kinds (the mock serves all three), so
/// [`Runtimes::distinct`] deduplicates before listing or probing by id.
#[derive(Clone, Default)]
pub struct Runtimes {
    by_kind: BTreeMap<MachineKind, SharedRuntime>,
}

impl std::fmt::Debug for Runtimes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut m = f.debug_map();
        for (k, rt) in &self.by_kind {
            m.entry(k, &rt.name());
        }
        m.finish()
    }
}

impl Runtimes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serve every kind with one runtime (tests, mock-only hosts).
    pub fn all(rt: SharedRuntime) -> Self {
        let mut r = Self::new();
        for k in [MachineKind::Vm, MachineKind::Container, MachineKind::Metal] {
            r.by_kind.insert(k, rt.clone());
        }
        r
    }

    pub fn insert(&mut self, kind: MachineKind, rt: SharedRuntime) -> &mut Self {
        self.by_kind.insert(kind, rt);
        self
    }

    pub fn get(&self, kind: MachineKind) -> Option<&SharedRuntime> {
        self.by_kind.get(&kind)
    }

    pub fn is_empty(&self) -> bool {
        self.by_kind.is_empty()
    }

    /// Every runtime once, in first-kind order.
    pub fn distinct(&self) -> Vec<SharedRuntime> {
        let mut out: Vec<SharedRuntime> = Vec::new();
        for rt in self.by_kind.values() {
            if !out.iter().any(|o| Arc::ptr_eq(o, rt)) {
                out.push(rt.clone());
            }
        }
        out
    }

    /// Runtime names, deduplicated: what the agent adds to its hello
    /// capabilities ("mock", "libvirt", "youki").
    pub fn names(&self) -> Vec<String> {
        self.distinct()
            .iter()
            .map(|r| r.name().to_owned())
            .collect()
    }

    pub async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        let rt = self.get(req.kind).ok_or_else(|| {
            ApiError::unprocessable(format!("this host has no runtime for {:?}", req.kind))
        })?;
        rt.start(req).await
    }

    /// Stop on whichever runtime holds `id`; `not_found` when none does.
    pub async fn stop(&self, id: &Id) -> ApiResult<()> {
        let mut last = ApiError::not_found(format!("machine {id} is not on this host"));
        for rt in self.distinct() {
            match rt.stop(id).await {
                Ok(()) => return Ok(()),
                Err(e) if e.code == pistack_types::error::ErrorCode::NotFound => {}
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Attach on the first runtime that holds the machine. `not_found` and
    /// `not_implemented` move on to the next runtime.
    pub async fn attach_volume(&self, req: &VolumeAttach) -> ApiResult<String> {
        let mut last = ApiError::not_implemented("no runtime on this host attaches volumes");
        for rt in self.distinct() {
            match rt.attach_volume(req).await {
                Ok(dev) => return Ok(dev),
                Err(e)
                    if e.code == pistack_types::error::ErrorCode::NotFound
                        || e.code == pistack_types::error::ErrorCode::NotImplemented =>
                {
                    last = e;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// Detach on the first runtime that holds the machine.
    pub async fn detach_volume(&self, req: &VolumeDetach) -> ApiResult<()> {
        let mut last = ApiError::not_implemented("no runtime on this host detaches volumes");
        for rt in self.distinct() {
            match rt.detach_volume(req).await {
                Ok(()) => return Ok(()),
                Err(e)
                    if e.code == pistack_types::error::ErrorCode::NotFound
                        || e.code == pistack_types::error::ErrorCode::NotImplemented =>
                {
                    last = e;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// Delete from every runtime; deleting an unknown id is not an error.
    pub async fn delete(&self, id: &Id) -> ApiResult<()> {
        let mut first_err = None;
        for rt in self.distinct() {
            if let Err(e) = rt.delete(id).await {
                if e.code != pistack_types::error::ErrorCode::NotFound {
                    first_err.get_or_insert(e);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Everything every runtime holds. A runtime that cannot list (a stub)
    /// contributes nothing rather than failing the whole report.
    pub async fn list(&self) -> Vec<RunningMachine> {
        let mut out = Vec::new();
        for rt in self.distinct() {
            match rt.list().await {
                Ok(v) => out.extend(v),
                Err(e) => tracing::debug!(runtime = rt.name(), error = %e, "anvil: list failed"),
            }
        }
        out
    }
}

/// Build the runtime set for a host from its detected capabilities. The
/// mock stands in for any kind with no real runtime when `allow_mock`;
/// kinds with neither are simply absent and the control plane will not
/// schedule them here (the hello advertises only what is present).
pub fn select_runtimes(capabilities: &[String], allow_mock: bool) -> Runtimes {
    let mock: SharedRuntime = Arc::new(MockRuntime::default());
    let mut r = Runtimes::new();
    for kind in [MachineKind::Vm, MachineKind::Container, MachineKind::Metal] {
        let has = |c: &str| capabilities.iter().any(|x| x == c);
        let rt: Option<SharedRuntime> = match kind {
            // libvirt remains the runtime when kvm or libvirt is present.
            MachineKind::Vm if has("libvirt") || has("kvm") => Some(Arc::new(LibvirtRuntime)),
            MachineKind::Vm if has("cloud-hypervisor") => Some(Arc::new(CloudRuntime)),
            MachineKind::Container if has("youki") => Some(Arc::new(YoukiRuntime)),
            MachineKind::Metal if has("metal") || has("ipmi") || has("redfish") => {
                Some(Arc::new(MetalRuntime))
            }
            _ if allow_mock => Some(mock.clone()),
            _ => None,
        };
        if let Some(rt) = rt {
            r.insert(kind, rt);
        }
    }
    r
}

/// Pick a runtime for a kind from the detected capabilities. libvirt
/// remains the runtime when `kvm` or `libvirt` is present.
/// `cloud-hypervisor` is selected when that capability is present and
/// those are not. youki runs containers when that capability is present.
/// Metal is this host when `metal`, `ipmi` or `redfish` is advertised.
/// The mock covers kinds with no real runtime.
pub fn select(
    kind: MachineKind,
    capabilities: &[String],
    allow_mock: bool,
) -> Option<SharedRuntime> {
    let has = |c: &str| capabilities.iter().any(|x| x == c);
    match kind {
        MachineKind::Vm if has("libvirt") || has("kvm") => Some(Arc::new(LibvirtRuntime)),
        MachineKind::Vm if has("cloud-hypervisor") => Some(Arc::new(CloudRuntime)),
        MachineKind::Container if has("youki") => Some(Arc::new(YoukiRuntime)),
        MachineKind::Metal if has("metal") || has("ipmi") || has("redfish") => {
            Some(Arc::new(MetalRuntime))
        }
        _ if allow_mock => Some(Arc::new(MockRuntime::default())),
        _ => None,
    }
}
