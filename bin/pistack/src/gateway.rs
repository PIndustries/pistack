// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The API gateway: one port, every module under its prefix, the agent RPC
//! endpoints, plus the discovery document at `/` and `/healthz`.
//!
//! Cross-cutting layers live here so no module re-implements them:
//! request-id, tracing, CORS, and authentication. `gate::attach_caller`
//! turns a bearer token into a `Caller` in request extensions (401 on a bad
//! token, pass-through when absent). In dev mode a missing caller is
//! replaced by a synthetic admin so a laptop works with no login.

use crate::server::Assembly;
use axum::{
    extract::Request,
    middleware::{self, Next},
    response::Response,
    routing::get,
    Json, Router,
};
use pistack_types::{Caller, Id, ModuleContext};
use serde::Serialize;
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
    dev_mode: bool,
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

pub fn build(asm: &Assembly, ctx: ModuleContext, verifier: gate::Verifier, dev: bool) -> Router {
    let mut app = Router::new();
    let mut entries = Vec::new();
    for m in &asm.modules {
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
    // Agent transport (anvil <-> control plane).
    app = app.nest("/rpc", pistack_rpc::router(asm.queue.clone()));

    let discovery = Discovery {
        name: "pistack",
        version: env!("CARGO_PKG_VERSION"),
        public_url: ctx.public_url.clone(),
        dev_mode: dev,
        modules: entries,
    };
    let mut app = app
        .route(
            "/",
            get({
                let d = discovery.clone();
                move || async move { Json(d) }
            }),
        )
        .route("/healthz", get(|| async { "ok" }));

    if dev {
        app = app.layer(middleware::from_fn(dev_admin));
    }
    app.layer(middleware::from_fn_with_state(
        verifier,
        gate::attach_caller,
    ))
    .layer(PropagateRequestIdLayer::x_request_id())
    .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
    .layer(TraceLayer::new_for_http())
    .layer(CorsLayer::permissive())
}

/// Dev mode only: requests with no caller become the admin.
async fn dev_admin(mut req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        req.extensions_mut().insert(Caller {
            principal: Id::new(pistack_types::kind::PRINCIPAL),
            principal_name: "dev-admin".into(),
            project: None,
            roles: vec!["admin".into()],
            system: true,
        });
    }
    next.run(req).await
}
