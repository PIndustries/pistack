// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Per-host agent: runs machines, plugs networks, attaches disks
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Anvil;

#[async_trait]
impl Module for Anvil {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "anvil",
            prefix: "anvil",
            summary: "Per-host agent: runs machines, plugs networks, attaches disks",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "anvil: scaffold" }))
    }
}

/// Options for `pistack agent`.
pub struct AgentOpts {
    pub join: String,
    pub token: Option<String>,
    pub data_dir: std::path::PathBuf,
}

/// Entry point for the host agent. Scaffold: registers nothing yet.
pub async fn run_agent(opts: AgentOpts) -> anyhow::Result<()> {
    tracing::info!(join = %opts.join, "anvil agent starting (scaffold)");
    anyhow::bail!("anvil agent not implemented yet")
}
