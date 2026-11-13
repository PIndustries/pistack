// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Per-host agent: runs machines, plugs networks, attaches disks.
//!
//! * [`facts`] — what this host is (cpus, memory, disk) and can run (kvm,
//!   libvirt, youki, ovs).
//! * [`runtime`] — the `MachineRuntime` trait. The mock is in-process. libvirt
//!   drives KVM through `virsh`. youki runs pods as OCI bundles.
//! * [`Anvil`] — the module mounted at `/host` on a control-plane node,
//!   serving this node's facts.
//! * `ovnhost` — points Open vSwitch at the cluster's OVN and reports
//!   whether this host is a working chassis.
//! * [`usage`] — what the host and its machines are using, for the
//!   heartbeat.
//! * [`agent`] — [`run_agent`], the entry point for `pistack agent`: hello,
//!   heartbeats and the command loop over `pistack-rpc`.

pub mod agent;
pub mod done;
pub mod facts;
pub mod images;
mod overlay;
mod ovnhost;
mod probe;
pub mod runtime;
pub mod usage;

pub use agent::{run_agent, run_agent_with, run_agent_with_facts, AgentOpts};
pub use facts::HostFacts;
pub use runtime::{
    select_runtimes, MachineKind, MachineRef, MachineRequest, MachineRuntime, MockRuntime,
    PowerState, RunningMachine, Runtimes, SharedRuntime,
};

use async_trait::async_trait;
use axum::{routing::get, Json, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Anvil;

#[async_trait]
impl Module for Anvil {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "anvil",
            prefix: "host",
            summary: "Per-host agent: runs machines, plugs networks, attaches disks",
            version: pistack_types::BUILD,
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { Json(facts::gather()) }))
    }
}
