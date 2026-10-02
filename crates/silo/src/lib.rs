// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Object storage, S3 API
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Silo;

#[async_trait]
impl Module for Silo {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "silo",
            prefix: "silo",
            summary: "Object storage, S3 API",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "silo: scaffold" }))
    }
}
