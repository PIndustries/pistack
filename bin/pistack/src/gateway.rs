// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The API gateway: one port, every module under its prefix, plus the
//! discovery document at `/` and `/healthz`.
//!
//! Cross-cutting layers (request-id, tracing, CORS, auth) live here so no
//! module re-implements them. Auth is a `gate` concern: once `gate` exposes
//! its token verifier the `require_auth` layer below calls it; until then it
//! only attaches a request id.

use axum::{routing::get, Json, Router};
use pistack_types::{Module, ModuleContext};
use serde::Serialize;
use std::sync::Arc;
use tower_http::{
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

#[derive(Serialize, Clone)]
struct Discovery {
    name: &'static str,
    version: &'static str,
    public_url: String,
    modules: Vec<ModuleEntry>,
}

#[derive(Serialize, Clone)]
struct ModuleEntry {
    name: &'static str,
    prefix: &'static str,
    summary: &'static str,
    version: &'static str,
    url: String,
}

pub fn build(modules: &[Arc<dyn Module>], ctx: ModuleContext) -> Router {
    let mut app = Router::new();
    let mut entries = Vec::new();
    for m in modules {
        let info = m.info();
        entries.push(ModuleEntry {
            name: info.name,
            prefix: info.prefix,
            summary: info.summary,
            version: info.version,
            url: format!("{}/{}", ctx.public_url, info.prefix),
        });
        app = app.nest(&format!("/{}", info.prefix), m.router(ctx.clone()));
    }
    let discovery = Discovery {
        name: "pistack",
        version: env!("CARGO_PKG_VERSION"),
        public_url: ctx.public_url.clone(),
        modules: entries,
    };
    app.route("/", get({ let d = discovery.clone(); move || async move { Json(d) } }))
        .route("/healthz", get(|| async { "ok" }))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}
