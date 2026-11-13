// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end through the router: boot on an in-memory store, log in as the
//! bootstrap admin, create a project and a user, check what the user may do.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use gate::{Gate, ADMIN_PASSWORD_ENV};
use pistack_store::Store;
use pistack_types::module::CancellationToken;
use pistack_types::{Module, ModuleContext};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn boot() -> Router {
    std::env::set_var(ADMIN_PASSWORD_ENV, "correct-horse");
    let store = Store::in_memory().unwrap();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let gate = Gate::default();
    gate.start(ctx.clone()).await.unwrap();
    assert_eq!(gate.info().prefix, "identity");
    gate.router(ctx)
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

async fn login(app: &Router, name: &str, password: &str) -> (StatusCode, Value) {
    call(
        app,
        "POST",
        "/tokens",
        None,
        Some(json!({"name": name, "password": password})),
    )
    .await
}

#[tokio::test]
async fn admin_creates_project_and_user_and_user_is_scoped() {
    let app = boot().await;

    // Bad password, then good.
    let (s, _) = login(&app, "admin", "wrong").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v) = login(&app, "admin", "correct-horse").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let admin_tok = v["token"].as_str().unwrap().to_owned();
    assert_eq!(v["caller"]["system"], true);
    assert_eq!(v["caller"]["roles"], json!(["admin"]));

    // Whoami + missing/garbage auth.
    let (s, v) = call(&app, "GET", "/tokens/self", Some(&admin_tok), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["principal_name"], "admin");
    let (s, v) = call(&app, "GET", "/tokens/self", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["code"], "unauthorized");
    let (s, _) = call(&app, "GET", "/tokens/self", Some("nope"), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Project.
    let (s, proj) = call(
        &app,
        "POST",
        "/projects",
        Some(&admin_tok),
        Some(json!({"name": "web", "quotas": {"machines": 3}, "labels": {"env": "prod"}})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{proj}");
    let proj_id = proj["id"].as_str().unwrap().to_owned();
    assert!(proj_id.starts_with("proj-"));
    assert_eq!(proj["spec"]["quotas"]["machines"], 3);
    let (s, _) = call(
        &app,
        "POST",
        "/projects",
        Some(&admin_tok),
        Some(json!({"name": "web"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = call(
        &app,
        "POST",
        "/projects",
        Some(&admin_tok),
        Some(json!({"name": "bad name!"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // List includes admin + web; label filter narrows.
    let (s, v) = call(&app, "GET", "/projects", Some(&admin_tok), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["total"], 2);
    let (_, v) = call(
        &app,
        "GET",
        "/projects?labels=env=prod",
        Some(&admin_tok),
        None,
    )
    .await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["name"], "web");

    // Stale If-Match conflicts; fresh one works.
    let (s, _) = call(
        &app,
        "PUT",
        "/projects/web",
        Some(&admin_tok),
        Some(json!({"description": "x"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let req = Request::builder()
        .method("PUT")
        .uri("/projects/web")
        .header(header::AUTHORIZATION, format!("Bearer {admin_tok}"))
        .header("if-match", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"description": "y"}).to_string()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::CONFLICT
    );

    // User with member role in web.
    let (s, user) = call(
        &app,
        "POST",
        "/principals",
        Some(&admin_tok),
        Some(json!({"name": "alice", "password": "pw-alice", "email": "a@example.com", "default_project": "web"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{user}");
    assert!(
        user["spec"].get("password_hash").is_none(),
        "hash must be redacted: {user}"
    );
    assert_eq!(user["spec"]["default_project"], proj_id);

    let (s, b) = call(
        &app,
        "POST",
        "/bindings",
        Some(&admin_tok),
        Some(json!({"principal": "alice", "role": "member", "project": "web"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["name"], "alice:member:web");
    assert_eq!(b["spec"]["scope"]["type"], "project");

    // Alice logs in; token is scoped to web with the member role.
    let (s, v) = login(&app, "alice", "pw-alice").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let alice_tok = v["token"].as_str().unwrap().to_owned();
    assert_eq!(v["caller"]["project"], proj_id);
    assert_eq!(v["caller"]["roles"], json!(["member"]));
    assert_eq!(v["caller"]["system"], false);

    // Alice may not list all projects, create projects or principals...
    let (s, v) = call(&app, "GET", "/projects", Some(&alice_tok), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["error"]["code"], "forbidden");
    let (s, _) = call(
        &app,
        "POST",
        "/projects",
        Some(&alice_tok),
        Some(json!({"name": "mine"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&app, "GET", "/principals", Some(&alice_tok), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // ...but may read her own project, herself, the roles and the policy.
    let (s, v) = call(
        &app,
        "GET",
        &format!("/projects/{proj_id}"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["name"], "web");
    let (s, _) = call(&app, "GET", "/projects/admin", Some(&alice_tok), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&app, "GET", "/principals/alice", Some(&alice_tok), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&app, "GET", "/principals/admin", Some(&alice_tok), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = call(&app, "GET", "/roles", Some(&alice_tok), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["total"], 3);
    let (s, v) = call(&app, "GET", "/policy", Some(&alice_tok), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["engine"], "builtin-rbac");

    // Alice cannot log in scoped to the admin project with any role... she
    // can, but gets no roles there.
    let (s, v) = call(
        &app,
        "POST",
        "/tokens",
        None,
        Some(json!({"name": "alice", "password": "pw-alice", "project": "admin"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["caller"]["roles"], json!([]));

    // API keys: alice makes one, logs in with it, admin sees it, alice deletes it.
    let (s, k) = call(
        &app,
        "POST",
        "/keys",
        Some(&alice_tok),
        Some(json!({"name": "laptop"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{k}");
    let secret = k["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("pik_"));
    assert_eq!(k["name"], "alice/laptop");
    let (s, v) = call(
        &app,
        "POST",
        "/tokens",
        None,
        Some(json!({"api_key": secret})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["caller"]["principal_name"], "alice");
    assert_eq!(v["caller"]["roles"], json!(["member"]));
    let (s, _) = call(
        &app,
        "POST",
        "/tokens",
        None,
        Some(json!({"api_key": "pik_bogus"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(
        &app,
        "POST",
        "/keys",
        Some(&alice_tok),
        Some(json!({"name": "x", "principal": "admin"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (_, v) = call(&app, "GET", "/keys", Some(&admin_tok), None).await;
    assert_eq!(v["total"], 1);
    let (s, _) = call(&app, "DELETE", "/keys/laptop", Some(&alice_tok), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(
        &app,
        "POST",
        "/tokens",
        None,
        Some(json!({"api_key": secret})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Built-in roles are protected; custom roles are not.
    let (s, _) = call(&app, "DELETE", "/roles/admin", Some(&admin_tok), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &app,
        "POST",
        "/roles",
        Some(&admin_tok),
        Some(json!({"name": "auditor"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = call(&app, "DELETE", "/roles/auditor", Some(&admin_tok), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Disabled principals cannot log in; admin cannot delete itself.
    let (s, _) = call(
        &app,
        "PUT",
        "/principals/alice",
        Some(&admin_tok),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = login(&app, "alice", "pw-alice").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(&app, "DELETE", "/principals/admin", Some(&admin_tok), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(&app, "DELETE", "/projects/admin", Some(&admin_tok), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Deleting the project removes its bindings.
    let (s, _) = call(&app, "DELETE", "/projects/web", Some(&admin_tok), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, v) = call(&app, "GET", "/bindings", Some(&admin_tok), None).await;
    assert_eq!(v["total"], 1, "{v}");

    // Unknown route inside the module gets the ApiError shape.
    let (s, v) = call(&app, "GET", "/nothing", Some(&admin_tok), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "not_found");
}

#[tokio::test]
async fn verifier_and_middleware_work_standalone() {
    use axum::{middleware, routing::get, Extension};
    use pistack_types::Caller;

    let app = boot().await;
    let (_, v) = login(&app, "admin", "correct-horse").await;
    let tok = v["token"].as_str().unwrap().to_owned();

    // A verifier built from the same store checks the token with no lookup.
    std::env::set_var(ADMIN_PASSWORD_ENV, "correct-horse");
    let store = Store::in_memory().unwrap();
    let other = gate::Verifier::from_store(&store).unwrap();
    assert!(
        other.verify(&tok).is_err(),
        "a different cloud's key must not verify"
    );

    // The gateway-style middleware attaches the caller as an extension.
    let store2 = Store::in_memory().unwrap();
    let keys = gate::Keys::load_or_create(&store2).unwrap();
    let caller = Caller {
        principal: pistack_types::Id::new("prin"),
        principal_name: "svc".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    };
    let (tok, _) = keys.sign(&caller, chrono::Duration::minutes(5)).unwrap();
    let inner = Router::new()
        .route(
            "/who",
            get(|Extension(c): Extension<Caller>| async move { c.principal_name }),
        )
        .layer(middleware::from_fn_with_state(
            keys.verifier(),
            gate::attach_caller,
        ));
    let res = inner
        .clone()
        .oneshot(
            Request::builder()
                .uri("/who")
                .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(to_bytes(res.into_body(), 1024).await.unwrap(), "svc");
    let res = inner
        .oneshot(
            Request::builder()
                .uri("/who")
                .header(header::AUTHORIZATION, "Bearer junk")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn attach_caller_aws4_rules() {
    use axum::{middleware, routing::get};
    use gate::is_aws4_authorization;

    assert!(is_aws4_authorization(
        "AWS4-HMAC-SHA256 Credential=x/20200101/r/s3/aws4_request, Signature=ab"
    ));
    assert!(!is_aws4_authorization(
        "AWS4-HMAC-SHA256garbage Credential=x"
    ));

    let store = Store::in_memory().unwrap();
    let keys = gate::Keys::load_or_create(&store).unwrap();
    let app = Router::new()
        .route("/who", get(|| async { "ok" }))
        .route("/silo/{*rest}", get(|| async { "silo-ok" }))
        .layer(middleware::from_fn_with_state(
            keys.verifier(),
            gate::attach_caller,
        ));

    // AWS4 on a non-silo path → 401 (unrecognized scheme).
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/who")
                .header(
                    header::AUTHORIZATION,
                    "AWS4-HMAC-SHA256 Credential=x/20200101/r/s3/aws4_request, Signature=ab",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Garbage scheme suffix on non-silo → 401.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/who")
                .header(header::AUTHORIZATION, "AWS4-HMAC-SHA256garbage x")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Duplicate Authorization → 401.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/who")
                .header(header::AUTHORIZATION, "Bearer a")
                .header(header::AUTHORIZATION, "Bearer b")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Proper AWS4 under /silo is left alone (no bearer verify) → handler runs.
    let res = app
        .oneshot(
            Request::builder()
                .uri("/silo/bucket/key")
                .header(
                    header::AUTHORIZATION,
                    "AWS4-HMAC-SHA256 Credential=x/20200101/r/s3/aws4_request, Signature=ab",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(to_bytes(res.into_body(), 1024).await.unwrap(), "silo-ok");
}

#[tokio::test]
async fn a_token_stops_working_when_what_it_stood_on_is_gone() {
    let app = boot().await;
    let (_, v) = login(&app, "admin", "correct-horse").await;
    let admin = v["token"].as_str().unwrap().to_owned();
    let post = |path: &'static str, tok: String, body: Value| {
        let app = app.clone();
        async move { call(&app, "POST", path, Some(&tok), Some(body)).await }
    };
    let (s, _) = post("/projects", admin.clone(), json!({"name": "web"})).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = post(
        "/principals",
        admin.clone(),
        json!({"name": "alice", "password": "pw-alice", "default_project": "web"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, binding) = post(
        "/bindings",
        admin.clone(),
        json!({"principal": "alice", "role": "member", "project": "web"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    let (_, v) = login(&app, "alice", "pw-alice").await;
    let by_password = v["token"].as_str().unwrap().to_owned();
    let (s, key) = post("/keys", by_password.clone(), json!({"name": "laptop"})).await;
    assert_eq!(s, StatusCode::CREATED, "{key}");
    let (s, v) = call(
        &app,
        "POST",
        "/tokens",
        None,
        Some(json!({"api_key": key["secret"]})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let by_key = v["token"].as_str().unwrap().to_owned();
    let who = |tok: String| {
        let app = app.clone();
        async move { call(&app, "GET", "/tokens/self", Some(&tok), None).await }
    };
    assert_eq!(who(by_key.clone()).await.0, StatusCode::OK);

    // The key is deleted: its token dies, the password one does not.
    let path = format!("/keys/{}", key["id"].as_str().unwrap());
    let (s, _) = call(&app, "DELETE", &path, Some(&by_password), None).await;
    assert!(s.is_success(), "{s}");
    let (s, v) = who(by_key).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["message"], "token revoked");
    let (s, v) = who(by_password.clone()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["roles"], json!(["member"]));

    // The binding is removed: the token says so on the next request.
    let path = format!("/bindings/{}", binding["id"].as_str().unwrap());
    let (s, _) = call(&app, "DELETE", &path, Some(&admin), None).await;
    assert!(s.is_success(), "{s}");
    let (s, v) = who(by_password.clone()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["roles"], json!([]));

    // The user is disabled: the token is refused.
    let (_, alice) = call(&app, "GET", "/principals/alice", Some(&admin), None).await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/principals/alice")
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::IF_MATCH, alice["revision"].to_string())
                .body(Body::from(json!({"enabled": false}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(who(by_password).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(who(admin).await.0, StatusCode::OK);
}

#[tokio::test]
async fn admin_sets_limits_and_a_member_reads_them_with_usage() {
    std::env::set_var(ADMIN_PASSWORD_ENV, "correct-horse");
    let store = Store::in_memory().unwrap();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let gate = Gate::default();
    gate.start(ctx.clone()).await.unwrap();
    let app = gate.router(ctx);
    let (_, v) = login(&app, "admin", "correct-horse").await;
    let admin = v["token"].as_str().unwrap().to_owned();

    for (path, body) in [
        (
            "/projects",
            json!({"name": "web", "quotas": {"machines": 3}}),
        ),
        ("/projects", json!({"name": "other"})),
        (
            "/principals",
            json!({"name": "alice", "password": "pw-alice", "default_project": "web"}),
        ),
        (
            "/bindings",
            json!({"principal": "alice", "role": "member", "project": "web"}),
        ),
    ] {
        let (s, v) = call(&app, "POST", path, Some(&admin), Some(body)).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
    }
    let (_, v) = login(&app, "alice", "pw-alice").await;
    let alice = v["token"].as_str().unwrap().to_owned();

    // Set two limits; the one that was there stays.
    let (s, v) = call(
        &app,
        "PUT",
        "/projects/web/quotas",
        Some(&admin),
        Some(json!({"quotas": {"vcpus": 16, "volumes_gib": 500}})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["quotas"]["machines"]["limit"], 3);
    assert_eq!(v["quotas"]["vcpus"]["limit"], 16);
    assert_eq!(v["quotas"]["volumes_gib"]["limit"], 500);

    // A module records what it counted; the report shows it.
    let web = gate::resolve_project(&store, "web").unwrap().meta.id;
    gate::quota::mirror_usage(&store, &web, &[("vcpus", 6)]);

    // null clears one limit and leaves the rest.
    let (s, v) = call(
        &app,
        "PUT",
        "/projects/web/quotas",
        Some(&admin),
        Some(json!({"quotas": {"volumes_gib": null}})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["quotas"]["volumes_gib"]["limit"], Value::Null);
    assert_eq!(v["quotas"]["vcpus"], json!({"limit": 16, "used": 6}));

    // A name no module enforces is refused, and nothing is stored.
    let (s, v) = call(
        &app,
        "PUT",
        "/projects/web/quotas",
        Some(&admin),
        Some(json!({"quotas": {"vcpu": 4}})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("'vcpu'"));

    // The member reads limits and usage of her own project only, and
    // cannot change them.
    let (s, v) = call(&app, "GET", "/projects/web/quotas", Some(&alice), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["name"], "web");
    assert_eq!(v["quotas"]["vcpus"], json!({"limit": 16, "used": 6}));
    assert_eq!(v["quotas"]["machines"], json!({"limit": 3, "used": 0}));
    assert_eq!(v["quotas"]["networks"], json!({"limit": null, "used": 0}));
    assert!(v["quotas"].get("vcpu").is_none());
    let (s, _) = call(&app, "GET", "/projects/other/quotas", Some(&alice), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &app,
        "PUT",
        "/projects/web/quotas",
        Some(&alice),
        Some(json!({"quotas": {"vcpus": 1000}})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Editing the project keeps the usage that was recorded.
    let (s, v) = call(
        &app,
        "PUT",
        "/projects/web",
        Some(&admin),
        Some(json!({"description": "front end"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"]["usage"]["vcpus"], 6);
    assert_eq!(v["spec"]["quotas"]["vcpus"], 16);
}
