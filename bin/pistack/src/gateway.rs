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
//!
//! Role enforcement for writes also lives here (`enforce_write_role`): a
//! caller who is neither admin nor `member` is refused every mutating
//! request, and the machine console ticket, before the module sees it.
//! `gate` (`/identity`) applies its own policy, and `/rpc` is the agent
//! transport, so both are left alone. Silo's SigV4 callers are attached
//! inside silo, which applies the same rule there.

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
    /// Schema version of this server's store.
    store_schema: u32,
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
    // The image download sits on the same transport and behind the same
    // checks: an agent has a client certificate, not a gate token.
    let images = agent_image_routes(mold::Client::new(store.clone(), asm.images.clone()));
    app = app.nest(
        "/rpc",
        pistack_rpc::router_with_extra(asm.queue.clone(), store.clone(), images),
    );

    let discovery = Discovery {
        name: "pistack",
        version: pistack_types::BUILD,
        store_schema: store
            .schema_version()
            .unwrap_or(pistack_store::SCHEMA_VERSION),
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

    // Before the two layers below, so removing a server is audited like any
    // other mutation. The peer routes are merged after them.
    if let Some(handle) = &cluster {
        app = app.merge(crate::cluster::admin_router(handle.clone()));
    }

    // What `/metrics` serves. The same figures are recorded on a timer so
    // alarms can watch them.
    let scrape = crate::metrics::Scrape {
        verifier: verifier.clone(),
        token: crate::metrics::scrape_token(
            std::env::var(crate::metrics::SCRAPE_TOKEN_ENV)
                .ok()
                .as_deref(),
        ),
        dev,
        gauge: gauge::Gauge.client(store.clone()),
        control: std::sync::Arc::new(crate::metrics::ControlPlane::new(
            cluster.clone(),
            asm.queue.clone(),
            store.clone(),
            cluster.as_ref().map(|handle| handle.db_path.clone()),
        )),
    };
    crate::metrics::spawn(
        scrape.control.clone(),
        scrape.gauge.clone(),
        ctx.shutdown.clone(),
    );

    app = guarded(app, store);

    if let Some(handle) = cluster {
        app = app.merge(crate::cluster::router(handle.clone()));
        app = app.layer(middleware::from_fn_with_state(
            handle.clone(),
            reads_at_leader,
        ));
        app = app.layer(middleware::from_fn_with_state(
            handle,
            crate::cluster::reject_follower_writes,
        ));
    }

    if dev {
        app = app.layer(middleware::from_fn(dev_admin));
    }
    let app = app
        .layer(middleware::from_fn_with_state(
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
        .layer(cors_layer(std::env::var(CORS_ORIGINS_ENV).ok().as_deref()));

    // Outside the layers above: a scraper's token is not a gate token, so
    // `attach_caller` would refuse it. The route checks for itself.
    Router::new()
        .route("/metrics", get(crate::metrics::scrape))
        .with_state(scrape)
        .merge(app)
}

/// Comma-separated origins allowed to call the API from a browser page
/// served somewhere else. Unset, there are none: the console is served by
/// this gateway and needs no cross-origin access.
pub const CORS_ORIGINS_ENV: &str = "PISTACK_CORS_ORIGINS";

/// Cross-origin rules for the listed origins. An empty or unset list sends
/// no CORS headers, so a browser refuses a page on another origin. An entry
/// that is not a valid header value is skipped with a warning.
fn cors_layer(origins: Option<&str>) -> CorsLayer {
    let allowed: Vec<axum::http::HeaderValue> = origins
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .filter_map(|o| match o.trim_end_matches('/').parse() {
            Ok(v) => Some(v),
            Err(_) => {
                tracing::warn!(
                    origin = o,
                    "ignoring a malformed entry in {CORS_ORIGINS_ENV}"
                );
                None
            }
        })
        .collect();
    if allowed.is_empty() {
        return CorsLayer::new();
    }
    CorsLayer::new()
        .allow_origin(allowed)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

fn agent_image_routes(images: mold::Client) -> Router {
    Router::new()
        .route("/images/{id}/data", get(agent_image))
        .with_state(images)
}

/// `GET /rpc/images/{id}/data`: a ready catalog image's bytes for an agent
/// that has to boot from it. By id only; the start request names the id.
async fn agent_image(
    axum::extract::State(images): axum::extract::State<mold::Client>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Response, pistack_types::ApiError> {
    let not_found = || pistack_types::ApiError::not_found(format!("image {id} not found"));
    let id = Id::parse(pistack_types::kind::IMAGE, &id).ok_or_else(not_found)?;
    let image = images.images().get(&id)?;
    let data = images.open_image(&image).await?;
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
        data,
        256 * 1024,
    ));
    let mut response = axum::response::IntoResponse::into_response(body);
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    );
    if let Some(size) = image.status.size_bytes {
        headers.insert(axum::http::header::CONTENT_LENGTH, size.into());
    }
    Ok(response)
}

fn is_mutation(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// Paths the write-role check leaves to their own rules: `gate` runs the
/// RBAC policy on `/identity`, `/rpc` is the agent transport, and redeeming
/// a join token is authorised by the token itself.
fn write_role_exempt(path: &str) -> bool {
    path == "/identity"
        || path.starts_with("/identity/")
        || path == "/rpc"
        || path.starts_with("/rpc/")
        || path == "/secrets/join-tokens/redeem"
}

/// A read by method that hands out control of a resource: the machine
/// console ticket (`GET /compute/machines/{id}/console`).
fn is_privileged_read(path: &str) -> bool {
    path.starts_with("/compute/machines/") && path.ends_with("/console")
}

/// A caller without the admin or `member` role may only read. A request
/// with no caller passes through; the module answers it as unauthorized.
async fn enforce_write_role(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if (is_mutation(req.method().as_str()) || is_privileged_read(path)) && !write_role_exempt(path)
    {
        if let Some(caller) = req.extensions().get::<Caller>() {
            if let Err(err) = caller.require_write() {
                return axum::response::IntoResponse::into_response(err);
            }
        }
    }
    next.run(req).await
}

/// The role check and the audit log around every route in `app`. Both are
/// inner to authentication, so the `Caller`, when there is one, is already
/// attached. The role check is inner to the audit, so a refused write is
/// recorded.
fn guarded(app: Router, store: pistack_store::Store) -> Router {
    app.layer(middleware::from_fn(enforce_write_role))
        .layer(middleware::from_fn_with_state(store, gate::audit::layer))
        .layer(middleware::from_fn(note_client))
}

/// Tell the audit log where the request came from: the peer address of the
/// connection. Forwarding headers are not trusted, so behind a proxy this
/// is the proxy.
async fn note_client(mut req: Request, next: Next) -> Response {
    let addr = req
        .extensions()
        .get::<keep::PeerIdentity>()
        .and_then(|peer| peer.addr);
    if let Some(addr) = addr {
        req.extensions_mut()
            .insert(gate::audit::ClientAddr(addr.ip()));
    }
    next.run(req).await
}

/// What a machine or a host is using is known where the agents report,
/// which is the leader. A follower would answer with nothing.
fn usage_read(path: &str) -> bool {
    let s: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    matches!(s.as_slice(), ["compute", "machines" | "hosts", _, "stats"])
}

/// Two kinds of read are answered by the leader only, and a follower sends
/// the client there the way it does for a write. A read that is audited: a
/// follower cannot write the record, and a secret must not be read without
/// one. And current usage, which only the leader has.
async fn reads_at_leader(
    axum::extract::State(handle): axum::extract::State<
        std::sync::Arc<crate::cluster::ClusterHandle>,
    >,
    req: Request,
    next: Next,
) -> Response {
    let leading = handle.leader.load(std::sync::atomic::Ordering::Acquire);
    let path = req.uri().path();
    if leading
        || req.method() != axum::http::Method::GET
        || !(gate::audit::audited_read(path) || usage_read(path))
    {
        return next.run(req).await;
    }
    let leader = handle
        .leader_api
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    axum::response::IntoResponse::into_response(
        pistack_types::ApiError::conflict("not the leader").with_detail("leader", leader),
    )
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
    use super::{cors_layer, dev_admin, enforce_write_role};
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

    async fn allowed_origin(layer: tower_http::cors::CorsLayer, origin: &str) -> Option<String> {
        let app = Router::new().route("/healthz", get(who)).layer(layer);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        resp.headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap().to_owned())
    }

    #[tokio::test]
    async fn only_a_listed_origin_is_allowed_cross_origin() {
        let other = "https://elsewhere.example";
        assert_eq!(allowed_origin(cors_layer(None), other).await, None);
        assert_eq!(allowed_origin(cors_layer(Some(" , ")), other).await, None);
        let listed = Some("https://portal.example/, https://ops.example");
        assert_eq!(allowed_origin(cors_layer(listed), other).await, None);
        assert_eq!(
            allowed_origin(cors_layer(listed), "https://portal.example").await,
            Some("https://portal.example".into())
        );
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

    fn caller(roles: &[&str]) -> Caller {
        Caller {
            principal: pistack_types::Id::new(pistack_types::kind::PRINCIPAL),
            principal_name: "someone".into(),
            project: Some(pistack_types::Id::new(pistack_types::kind::PROJECT)),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            system: false,
        }
    }

    /// Every path answers 200 unless the role check refuses it first.
    fn guarded(caller: Option<Caller>) -> Router {
        let mut app = Router::new()
            .fallback(|| async { "ok" })
            .layer(middleware::from_fn(enforce_write_role));
        if let Some(c) = caller {
            app = app.layer(Extension(c));
        }
        app
    }

    async fn status(app: &Router, method: &str, path: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    const WRITES: [(&str, &str); 12] = [
        ("POST", "/compute/machines"),
        ("DELETE", "/compute/machines/mach-1"),
        ("POST", "/network/networks"),
        ("PATCH", "/network/ports/port-1"),
        ("POST", "/volume/volumes/vol-1/attach"),
        ("POST", "/dns/zones"),
        ("POST", "/balancers/balancers"),
        ("POST", "/blueprint/stacks/stack-1/apply"),
        ("POST", "/gauge/alarms"),
        ("PUT", "/silo/buckets/b/objects/k"),
        ("POST", "/loom/pods/pod-1/exec"),
        ("GET", "/compute/machines/mach-1/console"),
    ];

    #[tokio::test]
    async fn reader_cannot_write_in_any_module() {
        let app = guarded(Some(caller(&["reader"])));
        for (method, path) in WRITES {
            assert_eq!(
                status(&app, method, path).await,
                StatusCode::FORBIDDEN,
                "{method} {path}"
            );
        }
        // Reads stay open.
        for path in ["/compute/machines", "/network/networks/net-1", "/"] {
            assert_eq!(status(&app, "GET", path).await, StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn caller_with_no_role_cannot_write() {
        let app = guarded(Some(caller(&[])));
        assert_eq!(
            status(&app, "POST", "/compute/machines").await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn member_and_admin_can_write() {
        for roles in [&["member"][..], &["admin"][..]] {
            let app = guarded(Some(caller(roles)));
            for (method, path) in WRITES {
                assert_eq!(
                    status(&app, method, path).await,
                    StatusCode::OK,
                    "{method} {path}"
                );
            }
        }
    }

    #[tokio::test]
    async fn exempt_paths_and_anonymous_requests_pass_through() {
        let reader = guarded(Some(caller(&["reader"])));
        for path in [
            "/identity/tokens",
            "/identity/principals/p/password",
            "/rpc/agents/hello",
            "/secrets/join-tokens/redeem",
        ] {
            assert_eq!(
                status(&reader, "POST", path).await,
                StatusCode::OK,
                "{path}"
            );
        }
        // No caller: the module decides (it answers unauthorized).
        let anonymous = guarded(None);
        assert_eq!(
            status(&anonymous, "POST", "/compute/machines").await,
            StatusCode::OK
        );
    }

    fn records(store: &Store) -> Vec<gate::audit::AuditSpec> {
        gate::audit::list(store, &gate::audit::AuditQuery::default())
            .unwrap()
            .items
            .into_iter()
            .map(|record| record.spec)
            .collect()
    }

    #[tokio::test]
    async fn a_refused_write_and_a_secret_read_are_recorded_with_the_client_address() {
        let store = Store::in_memory().unwrap();
        let peer = keep::PeerIdentity::plain("10.0.0.9:51234".parse().unwrap());
        let app = |who: Caller| {
            super::guarded(Router::new().fallback(|| async { "ok" }), store.clone())
                .layer(Extension(peer.clone()))
                .layer(Extension(who))
        };
        let reader = app(caller(&["reader"]));
        assert_eq!(
            status(&reader, "POST", "/compute/machines").await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(&reader, "GET", "/secrets/secrets/db/payload").await,
            StatusCode::OK
        );
        // An ordinary read leaves no record.
        assert_eq!(
            status(&reader, "GET", "/compute/machines").await,
            StatusCode::OK
        );
        let mut admin = caller(&["admin"]);
        admin.principal_name = "root".into();
        admin.system = true;
        let admin = app(admin);
        for (method, path) in [
            ("POST", "/compute/hosts/rack1/drain"),
            ("POST", "/compute/hosts/rack1/evacuate"),
            ("DELETE", "/cluster/members/n2"),
            ("POST", "/rpc/agents/enroll"),
            ("POST", "/rpc/agents/heartbeat"),
        ] {
            assert_eq!(status(&admin, method, path).await, StatusCode::OK);
        }

        let log = records(&store);
        let seen: Vec<String> = log
            .iter()
            .map(|r| format!("{} {} {} {}", r.action, r.kind, r.resource, r.status))
            .collect();
        assert_eq!(
            seen,
            [
                "create machines  403",
                "read_payload secrets db 200",
                "drain hosts rack1 200",
                "evacuate hosts rack1 200",
                "delete members n2 200",
                "enroll agents  200",
            ]
        );
        assert_eq!(log[0].principal.as_deref(), Some("someone"));
        assert!(log[0].principal_id.is_some());
        assert!(log[0].project.is_some());
        assert_eq!(log[2].principal.as_deref(), Some("root"));
        for record in &log {
            assert_eq!(record.client.as_deref(), Some("10.0.0.9"));
        }
    }

    #[tokio::test]
    async fn a_follower_sends_an_audited_read_and_a_usage_read_to_the_leader() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        let leadership = pistack_types::Leadership::new(false);
        let established = crate::cluster::Established {
            role: pistack_cluster::Role::Voter,
            leader: false,
            leader_api: "http://10.0.0.11:8443".into(),
            term: 1,
            members: Vec::new(),
        };
        let handle = std::sync::Arc::new(crate::cluster::handle_from(
            "n2".into(),
            "http://10.0.0.12:8443".into(),
            "c1".into(),
            store.clone(),
            dir.path().join("state.redb"),
            &established,
            None,
            leadership,
        ));
        let app = Router::new()
            .fallback(|| async { "ok" })
            .layer(middleware::from_fn_with_state(
                handle.clone(),
                super::reads_at_leader,
            ));
        let secret = "/secrets/secrets/db/payload";
        let response = app
            .clone()
            .oneshot(Request::builder().uri(secret).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let text = body(response).await;
        assert!(text.contains("not the leader"), "{text}");
        assert!(text.contains("http://10.0.0.11:8443"), "{text}");
        // Other reads are still answered here, and so is this one once
        // this server leads.
        assert_eq!(
            status(&app, "GET", "/secrets/secrets/db").await,
            StatusCode::OK
        );
        // Usage is only known where the agents report.
        for path in ["/compute/machines/web/stats", "/compute/hosts/h1/stats"] {
            assert_eq!(status(&app, "GET", path).await, StatusCode::CONFLICT);
        }
        handle
            .leader
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(status(&app, "GET", secret).await, StatusCode::OK);
        assert_eq!(
            status(&app, "GET", "/compute/machines/web/stats").await,
            StatusCode::OK
        );
    }

    /// A server holding one ready image, with the agent transport on a real
    /// socket. Returns the transport's base URL and what forge would send.
    async fn image_server(
        auth: pistack_rpc::RpcAuth,
        bytes: &'static [u8],
    ) -> (String, pistack_rpc::ImageSource, tempfile::TempDir) {
        let blobs = tempfile::tempdir().unwrap();
        let images = mold::Client::local(blobs.path()).unwrap();
        let created = images
            .create(mold::CreateImage {
                name: "noble".into(),
                description: String::new(),
                labels: Default::default(),
                annotations: Default::default(),
                project: None,
                spec: serde_json::from_value(serde_json::json!({ "format": "raw" })).unwrap(),
            })
            .unwrap();
        let ready = images
            .upload(&created.meta.id, Box::pin(bytes))
            .await
            .unwrap();
        let source = pistack_rpc::ImageSource {
            id: ready.meta.id.to_string(),
            sha256: ready.status.sha256.clone().unwrap(),
            size_bytes: ready.status.size_bytes,
            format: ready.spec.format.to_string(),
        };
        let app = Router::new().nest(
            "/rpc",
            pistack_rpc::router_with_auth_and(
                pistack_rpc::CommandQueue::default(),
                images.store().clone(),
                auth,
                super::agent_image_routes(images),
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        (base, source, blobs)
    }

    #[tokio::test]
    async fn an_agent_downloads_a_catalog_image_it_does_not_have() {
        let bytes: &[u8] = b"not a real disk, but the same bytes on both ends";
        let (base, source, _blobs) = image_server(pistack_rpc::RpcAuth::open(), bytes).await;
        let client = pistack_rpc::AgentClient::new(format!("{base}/rpc"), "n1").unwrap();
        let host = tempfile::tempdir().unwrap();
        let cache = anvil::images::ImageCache::new(host.path());

        let path = cache.ensure(&client, &source).await.unwrap();
        assert!(path.starts_with(host.path()));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        // An id the catalog does not have is an error, not an empty file.
        let mut missing = source.clone();
        missing.id = pistack_types::Id::new(pistack_types::kind::IMAGE).to_string();
        missing.sha256 = "0".repeat(64);
        let err = cache.ensure(&client, &missing).await.unwrap_err();
        assert_eq!(
            err.code,
            pistack_types::error::ErrorCode::NotFound,
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn the_image_download_needs_the_agent_certificate() {
        let (base, source, _blobs) =
            image_server(pistack_rpc::RpcAuth::mtls_only(), b"bytes").await;
        let response = reqwest::get(format!("{base}/rpc/images/{}/data", source.id))
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 401);
    }
}
