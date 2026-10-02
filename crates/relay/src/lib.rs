// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Load balancers and ingress
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Relay;

#[async_trait]
impl Module for Relay {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "relay",
            prefix: "relay",
            summary: "Load balancers and ingress",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "relay: scaffold" }))
    }
}
