// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Secrets, keys, certificates and the cloud CA
//!
//! Status: scaffold. See docs/ARCHITECTURE.md for what this module owns.

use async_trait::async_trait;
use axum::{routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Keep;

#[async_trait]
impl Module for Keep {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "keep",
            prefix: "keep",
            summary: "Secrets, keys, certificates and the cloud CA",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(|| async { "keep: scaffold" }))
    }
}
