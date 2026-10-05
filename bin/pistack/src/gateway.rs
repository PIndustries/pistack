// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The API gateway: one port, every module under its prefix, the agent RPC
//! endpoints, plus the discovery document at `/` and `/healthz`.
//!
//! Cross-cutting layers live here so no module re-implements them:
//! request-id, tracing, CORS, and authentication. `gate::attach_caller`
//! turns a bearer token into a `Caller` in request extensions (401 on a bad
//! token, pass-through when absent). In dev mode a missing caller is
//! replaced by a synthetic admin so a laptop works with no login, except
//! `/silo` SigV4 (header or presigned), which must reach silo without a
//! bearer `Caller`.

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

pub fn build(
    asm: &Assembly,
    ctx: ModuleContext,
    verifier: gate::Verifier,
    dev: bool,
    cluster: Option<std::sync::Arc<crate::cluster::ClusterHandle>>,
) -> Router {
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
    // Agent transport (anvil <-> control plane). `pistack agent --join URL`
    // talks to URL/rpc/agents/... Strict by default: `/rpc` requires a
    // cloud-CA client certificate (escape hatch `PISTACK_RPC_INSECURE=1`).
    // Enroll spends `x-pistack-join` once for that certificate. Optional
    // per-call join checks stay behind `PISTACK_REQUIRE_JOIN=1`. The token
    // is not Authorization: a bearer that is not a gate token is rejected by
    // `attach_caller` below.
    let store = (*ctx.store::<pistack_store::Store>()).clone();
    app = app.nest(
        "/rpc",
        pistack_rpc::router_with_store(asm.queue.clone(), store.clone()),
    );

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

    // Inner to authentication so the Caller, when present, is already attached.
    app = app.layer(middleware::from_fn_with_state(store, audit_mutation));

    if let Some(handle) = cluster {
        app = app.merge(crate::cluster::router(handle.clone()));
        app = app.layer(middleware::from_fn_with_state(
            handle,
            crate::cluster::reject_follower_writes,
        ));
    }

    if dev {
        app = app.layer(middleware::from_fn(dev_admin));
    }
    app.layer(middleware::from_fn_with_state(
        verifier,
        gate::attach_caller,
    ))
    .layer(PropagateRequestIdLayer::x_request_id())
    .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
    .layer(
        TraceLayer::new_for_http().make_span_with(|request: &axum::extract::Request| {
            // Path only. The console ticket is a query parameter and must not be logged.
            tracing::debug_span!(
                "request",
                method = %request.method(),
                path = %request.uri().path(),
                version = ?request.version(),
            )
        }),
    )
    .layer(CorsLayer::permissive())
}

fn is_mutation(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// After the response, record a mutation. The path has no query string.
async fn audit_mutation(
    axum::extract::State(store): axum::extract::State<pistack_store::Store>,
    req: Request,
    next: Next,
) -> Response {
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().to_owned();
    let principal = req
        .extensions()
        .get::<Caller>()
        .map(|c| c.principal_name.clone());
    let response = next.run(req).await;
    if is_mutation(&method) {
        gate::audit::record(
            &store,
            &method,
            &path,
            principal.as_deref(),
            response.status().as_u16(),
        );
    }
    response
}

/// Dev mode only: requests with no caller become the admin.
///
/// Skip `/silo` SigV4 (AWS4 header or presigned query): attaching a
/// synthetic caller would make silo reject the request as bearer+SigV4.
async fn dev_admin(mut req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none()
        && !gate::silo_sigv4_skips_bearer(req.uri().path(), req.headers(), req.uri().query())
    {
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

#[cfg(test)]
mod tests {
    use super::dev_admin;
    use axum::{
        body::{to_bytes, Body},
        extract::Request,
        http::{header, StatusCode},
        middleware,
        routing::get,
        Extension, Router,
    };
    use gate::{Keys, Verifier};
    use pistack_store::Store;
    use pistack_types::Caller;
    use tower::ServiceExt;

    async fn who(ext: Option<Extension<Caller>>) -> &'static str {
        if let Some(Extension(c)) = ext {
            if c.principal_name == "dev-admin" {
                "dev-admin"
            } else {
                "other-caller"
            }
        } else {
            "no-caller"
        }
    }

    fn app(dev: bool, verifier: Verifier) -> Router {
        let mut app = Router::new()
            .route("/silo/{*rest}", get(who))
            .route("/healthz", get(who));
        if dev {
            app = app.layer(middleware::from_fn(dev_admin));
        }
        app.layer(middleware::from_fn_with_state(
            verifier,
            gate::attach_caller,
        ))
    }

    async fn body(resp: axum::response::Response) -> String {
        String::from_utf8(to_bytes(resp.into_body(), 1024).await.unwrap().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn dev_admin_skips_silo_sigv4_header_and_presign() {
        let store = Store::in_memory().unwrap();
        let keys = Keys::load_or_create(&store).unwrap();
        let app = app(true, keys.verifier());

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/silo/bucket/key")
                    .header(
                        header::AUTHORIZATION,
                        "AWS4-HMAC-SHA256 Credential=x/20200101/pistack/s3/aws4_request, Signature=ab",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body(res).await, "no-caller");

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/silo/bucket/key?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature=deadbeef")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body(res).await, "no-caller");

        // Non-silo still gets the synthetic admin.
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body(res).await, "dev-admin");
    }

    #[tokio::test]
    async fn without_dev_non_silo_stays_unauthenticated() {
        let store = Store::in_memory().unwrap();
        let keys = Keys::load_or_create(&store).unwrap();
        let app = app(false, keys.verifier());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body(res).await, "no-caller");
    }
}
