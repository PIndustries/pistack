// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Machine runtimes: the host-side counterpart of `forge::HostDriver`.
//!
//! `anvil` does not depend on `forge`, so the request and result types here
//! are the agent's own. The transport (`pistack-rpc`) translates
//! `forge::Machine` into a [`MachineRequest`] and a [`RunningMachine`] back
//! into `forge::MachineRuntime`; both are plain data and map one-to-one.

pub mod libvirt;
pub mod mock;
pub mod youki;

use async_trait::async_trait;
use pistack_types::{ApiResult, Id, Resources};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use libvirt::LibvirtRuntime;
pub use mock::MockRuntime;
pub use youki::YoukiRuntime;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum MachineKind {
    #[default]
    Vm,
    Container,
    Metal,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    On,
    Off,
    #[default]
    Unknown,
}

/// Everything a runtime needs to bring a machine up.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineRequest {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub kind: MachineKind,
    pub image: String,
    pub resources: Resources,
    #[serde(default)]
    pub networks: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_data: Option<String>,
    #[serde(default)]
    pub ssh_keys: Vec<String>,
}

/// What the runtime reports about a machine it holds.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunningMachine {
    pub id: Id,
    pub addresses: Vec<String>,
    pub power: PowerState,
}

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
}

pub type SharedRuntime = Arc<dyn MachineRuntime>;

/// Pick a runtime for a kind from the detected capabilities. Today only the
/// mock is real; the others exist so the wiring compiles end to end.
pub fn select(
    kind: MachineKind,
    capabilities: &[String],
    allow_mock: bool,
) -> Option<SharedRuntime> {
    let has = |c: &str| capabilities.iter().any(|x| x == c);
    match kind {
        MachineKind::Vm if has("libvirt") || has("kvm") => Some(Arc::new(LibvirtRuntime)),
        MachineKind::Container if has("youki") => Some(Arc::new(YoukiRuntime)),
        _ if allow_mock => Some(Arc::new(MockRuntime::default())),
        _ => None,
    }
}
