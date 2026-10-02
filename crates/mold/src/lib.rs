// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Boot images and templates
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Mold;

#[async_trait]
impl Module for Mold {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "mold",
            prefix: "mold",
            summary: "Boot images and templates",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "mold: scaffold" }))
    }
}
