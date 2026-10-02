// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Per-host agent: runs machines, plugs networks, attaches disks.
//!
//! * [`facts`] — what this host is (cpus, memory, disk) and can run (kvm,
//!   libvirt, youki, ovs).
//! * [`runtime`] — the `MachineRuntime` trait with a working mock and
//!   libvirt/youki stubs.
//! * [`Anvil`] — the module mounted at `/host` on a control-plane node,
//!   serving this node's facts.
//! * [`run_agent`] — entry point for `pistack agent`. Gathers facts and
//!   stops: the control-plane transport (`pistack-rpc`) is not wired yet.

pub mod facts;
pub mod runtime;

pub use facts::HostFacts;
pub use runtime::{MachineRequest, MachineRuntime, RunningMachine};

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
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { Json(facts::gather()) }))
    }
}

/// Options for `pistack agent`.
pub struct AgentOpts {
    pub join: String,
    pub token: Option<String>,
    pub data_dir: std::path::PathBuf,
}

/// Entry point for the host agent.
///
/// Today this gathers and logs the host facts, then bails: the hello /
/// heartbeat / command loop lives in `pistack-rpc` and is wired in by the
/// binary once that crate lands. Keeping the signature stable is the point.
pub async fn run_agent(opts: AgentOpts) -> anyhow::Result<()> {
    let facts = facts::gather_at(&opts.data_dir);
    tracing::info!(
        join = %opts.join,
        node = %facts.node_name,
        vcpus = facts.resources.vcpus,
        memory_mib = facts.resources.memory_mib,
        disk_gib = facts.resources.disk_gib,
        capabilities = ?facts.capabilities,
        has_token = opts.token.is_some(),
        "anvil agent starting"
    );
    anyhow::bail!("anvil agent: control-plane transport (pistack-rpc) not wired yet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_agent_bails_cleanly() {
        let err = run_agent(AgentOpts {
            join: "http://127.0.0.1:1".into(),
            token: None,
            data_dir: std::env::temp_dir(),
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not wired"));
    }
}
