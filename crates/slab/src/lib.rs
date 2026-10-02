// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Block volumes, snapshots, backups
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Slab;

#[async_trait]
impl Module for Slab {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "slab",
            prefix: "slab",
            summary: "Block volumes, snapshots, backups",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "slab: scaffold" }))
    }
}
