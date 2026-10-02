// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The pi CLI and web console
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Bench;

#[async_trait]
impl Module for Bench {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "bench",
            prefix: "bench",
            summary: "The pi CLI and web console",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "bench: scaffold" }))
    }
}
