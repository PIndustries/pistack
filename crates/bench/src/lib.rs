// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `bench`: the `pi` command-line client and (later) the web console.
//!
//! The CLI lives in [`cli`] as a library so it can be driven from tests
//! with captured output; `src/bin/pi.rs` is a four-line wrapper. The
//! `Bench` module mounts under `/console` and today only says that the
//! console is not built yet.

pub mod cli;

use async_trait::async_trait;
use axum::{routing::get, Json, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

#[derive(Default)]
pub struct Bench;

#[async_trait]
impl Module for Bench {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "bench",
            prefix: "console",
            summary: "The pi CLI and web console",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route(
            "/",
            get(|| async {
                Json(serde_json::json!({
                    "console": "not built yet",
                    "cli": "pi",
                }))
            }),
        )
    }
}
