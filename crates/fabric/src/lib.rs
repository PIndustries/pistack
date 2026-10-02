// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Networks, subnets, routers, security groups, floating IPs (OVN)
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Fabric;

#[async_trait]
impl Module for Fabric {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "fabric",
            prefix: "fabric",
            summary: "Networks, subnets, routers, security groups, floating IPs (OVN)",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "fabric: scaffold" }))
    }
}
