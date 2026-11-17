// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Module`: the contract `pistack-core` mounts each module against.

use async_trait::async_trait;
use axum::Router;

use super::module_context::ModuleContext;
use super::module_info::ModuleInfo;

#[async_trait]
pub trait Module: Send + Sync + 'static {
    fn info(&self) -> ModuleInfo;
    /// Routes relative to the module prefix. The gateway adds auth and
    /// request-id layers; modules add nothing global.
    fn router(&self, ctx: ModuleContext) -> Router;
    /// Spawn long-running tasks. Default: nothing.
    async fn start(&self, _ctx: ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }
}
