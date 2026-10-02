// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Machines: VMs, containers and bare metal as one resource
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Forge;

#[async_trait]
impl Module for Forge {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "forge",
            prefix: "forge",
            summary: "Machines: VMs, containers and bare metal as one resource",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "forge: scaffold" }))
    }
}
