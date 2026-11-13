// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The audit log through the router: the identity API under the same audit
//! and authentication layers the gateway puts around it.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    middleware, Router,
};
use gate::{audit, Gate, ADMIN_PASSWORD_ENV};
use pistack_store::Store;
use pistack_types::module::CancellationToken;
use pistack_types::{Module, ModuleContext};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const ADMIN_PASSWORD: &str = "correct-horse";

async fn boot() -> (Router, Store) {
    std::env::set_var(ADMIN_PASSWORD_ENV, ADMIN_PASSWORD);
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
    let app = Router::new()
        .nest("/identity", gate.router(ctx))
        .layer(middleware::from_fn_with_state(store.clone(), audit::layer))
        .layer(middleware::from_fn_with_state(
            gate.verifier().unwrap(),
            gate::attach_caller,
        ));
    (app, store)
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

async fn login(app: &Router, body: Value) -> (StatusCode, Value) {
    call(app, "POST", "/identity/tokens", None, Some(body)).await
}

async fn admin_token(app: &Router) -> String {
    let (s, v) = login(app, json!({"name": "admin", "password": ADMIN_PASSWORD})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["token"].as_str().unwrap().to_owned()
}

/// The log as the admin reads it, oldest first, with `query` appended.
async fn audit_log(app: &Router, token: &str, query: &str) -> Value {
    let path = format!("/identity/audit?limit=1000{query}");
    let (s, v) = call(app, "GET", &path, Some(token), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

async fn count(app: &Router, token: &str, query: &str) -> usize {
    audit_log(app, token, query).await["items"]
        .as_array()
        .unwrap()
        .len()
}

fn find<'a>(page: &'a Value, action: &str, kind: &str) -> &'a Value {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["spec"]["action"] == action && r["spec"]["kind"] == kind)
        .unwrap_or_else(|| panic!("no {action} {kind} record in {page}"))
}

#[tokio::test]
async fn identity_actions_are_recorded_with_who_and_what_and_no_secrets() {
    let (app, _store) = boot().await;

    // A failed login, then a good one.
    let (s, _) = login(&app, json!({"name": "admin", "password": "hunter2-wrong"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let admin = admin_token(&app).await;

    let body = json!({"name": "web"});
    let (s, project) = call(&app, "POST", "/identity/projects", Some(&admin), Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{project}");
    let project_id = project["id"].as_str().unwrap();
    let body = json!({"name": "alice", "password": "alice-pass-phrase", "default_project": "web"});
    let (s, user) = call(
        &app,
        "POST",
        "/identity/principals",
        Some(&admin),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{user}");
    let body = json!({"principal": "alice", "role": "member", "project": "web"});
    let (s, binding) = call(&app, "POST", "/identity/bindings", Some(&admin), Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{binding}");
    let body = json!({"quotas": {"machines": 3}});
    let path = "/identity/projects/web/quotas";
    let (s, v) = call(&app, "PUT", path, Some(&admin), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Alice makes an API key, logs in with it, and is refused a project.
    let body = json!({"name": "alice", "password": "alice-pass-phrase"});
    let (s, v) = login(&app, body).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let alice = v["token"].as_str().unwrap().to_owned();
    let body = json!({"name": "ci"});
    let (s, key) = call(&app, "POST", "/identity/keys", Some(&alice), Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{key}");
    let key_id = key["id"].as_str().unwrap();
    let key_secret = key["secret"].as_str().expect("the key's secret").to_owned();
    let (s, v) = login(&app, json!({"api_key": key_secret})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let by_key = v["token"].as_str().unwrap().to_owned();
    let body = json!({"name": "mine"});
    let (s, _) = call(
        &app,
        "POST",
        "/identity/projects",
        Some(&by_key),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let path = format!("/identity/bindings/{}", binding["id"].as_str().unwrap());
    let (s, _) = call(&app, "DELETE", &path, Some(&admin), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let page = audit_log(&app, &admin, "").await;
    let items = page["items"].as_array().unwrap();
    // Reads are not recorded: every record is one of the writes above.
    assert_eq!(items.len(), 11, "{page}");
    assert_eq!(page["total"], 11);

    // The failed login names who was tried, and nothing about the password.
    let failed = &items[0]["spec"];
    assert_eq!(failed["action"], "login");
    assert_eq!(failed["status"], 401);
    assert_eq!(failed["principal"], "admin");
    assert!(failed.get("principal_id").is_none(), "{failed}");
    let ok = &items[1]["spec"];
    assert_eq!(ok["action"], "login");
    assert_eq!(ok["status"], 200);
    assert_eq!(ok["principal"], "admin");
    assert!(ok["principal_id"].is_string(), "{ok}");

    // A create is recorded with the id it made.
    let created = &find(&page, "create", "projects")["spec"];
    assert_eq!(created["resource"], project_id);
    assert_eq!(created["principal"], "admin");
    assert_eq!(created["method"], "POST");
    assert_eq!(created["path"], "/identity/projects");
    assert_eq!(find(&page, "create", "bindings")["spec"]["status"], 201);
    assert_eq!(find(&page, "delete", "bindings")["spec"]["status"], 204);
    let quota = &find(&page, "update_quotas", "projects")["spec"];
    assert_eq!(quota["resource"], "web");

    // A login with a key says which key, and so does what the token did.
    let key_login = items
        .iter()
        .find(|r| r["spec"]["action"] == "login" && r["spec"]["api_key"] == key_id)
        .unwrap_or_else(|| panic!("no key login in {page}"));
    assert_eq!(key_login["spec"]["principal"], "alice");
    let refused = items
        .iter()
        .find(|r| r["spec"]["status"] == 403)
        .unwrap_or_else(|| panic!("no refusal in {page}"));
    assert_eq!(refused["spec"]["principal"], "alice");
    assert_eq!(refused["spec"]["api_key"], key_id);
    assert_eq!(refused["spec"]["project"], project_id);
    assert_eq!(refused["spec"]["action"], "create");
    assert!(refused["created_at"].is_string());

    // No password, token or key secret is anywhere in the log.
    let text = page.to_string();
    for secret in [
        ADMIN_PASSWORD,
        "hunter2-wrong",
        "alice-pass-phrase",
        key_secret.as_str(),
        admin.as_str(),
        alice.as_str(),
        by_key.as_str(),
    ] {
        assert!(!text.contains(secret), "{secret} is in the audit log");
    }
}

#[tokio::test]
async fn the_log_is_filtered_and_paged() {
    let (app, _store) = boot().await;
    let (s, _) = login(&app, json!({"name": "mallory", "password": "x"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let admin = admin_token(&app).await;
    for name in ["a", "b", "c", "d"] {
        let body = json!({"name": name});
        let (s, v) = call(&app, "POST", "/identity/projects", Some(&admin), Some(body)).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
    }
    let (s, _) = call(&app, "DELETE", "/identity/projects/d", Some(&admin), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(
        &app,
        "DELETE",
        "/identity/projects/nope",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    assert_eq!(count(&app, &admin, "").await, 8);
    assert_eq!(count(&app, &admin, "&action=create").await, 4);
    assert_eq!(count(&app, &admin, "&action=login").await, 2);
    assert_eq!(count(&app, &admin, "&kind=projects").await, 6);
    assert_eq!(count(&app, &admin, "&kind=projects&action=delete").await, 2);
    assert_eq!(count(&app, &admin, "&resource=d").await, 1);
    assert_eq!(count(&app, &admin, "&principal=mallory").await, 1);
    assert_eq!(count(&app, &admin, "&principal=admin").await, 7);
    assert_eq!(count(&app, &admin, "&outcome=failure").await, 2);
    assert_eq!(count(&app, &admin, "&outcome=404").await, 1);
    assert_eq!(count(&app, &admin, "&outcome=success").await, 6);
    // The admin's token is scoped to the admin project, and that is the
    // project on what it did. A project is given by name or id.
    assert_eq!(count(&app, &admin, "&project=admin").await, 7);
    assert_eq!(count(&app, &admin, "&project=a").await, 0);

    // Time range: everything is after an hour ago and nothing before it.
    let hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    assert_eq!(count(&app, &admin, &format!("&since={hour_ago}")).await, 8);
    assert_eq!(count(&app, &admin, &format!("&until={hour_ago}")).await, 0);

    // Pages of three, oldest first, then the newest three.
    let mut seen = Vec::new();
    let mut after = String::new();
    loop {
        let path = format!("/identity/audit?limit=3{after}");
        let (s, page) = call(&app, "GET", &path, Some(&admin), None).await;
        assert_eq!(s, StatusCode::OK, "{page}");
        assert_eq!(page["total"], 8);
        let items = page["items"].as_array().unwrap();
        assert!(items.len() <= 3);
        seen.extend(items.iter().map(|r| r["id"].as_str().unwrap().to_owned()));
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => break,
        }
    }
    assert_eq!(seen.len(), 8);
    let all = audit_log(&app, &admin, "").await;
    let all: Vec<&str> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(seen, all);
    let path = "/identity/audit?limit=3&order=desc";
    let (_, newest) = call(&app, "GET", path, Some(&admin), None).await;
    let newest: Vec<&str> = newest["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    let expect: Vec<&str> = seen.iter().rev().take(3).map(String::as_str).collect();
    assert_eq!(newest, expect);

    for bad in ["outcome=maybe", "order=sideways", "since=yesterday"] {
        let path = format!("/identity/audit?{bad}");
        let (s, v) = call(&app, "GET", &path, Some(&admin), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {v}");
    }
}

#[tokio::test]
async fn only_a_cloud_admin_reads_the_log() {
    let (app, _store) = boot().await;
    let admin = admin_token(&app).await;
    for (path, body) in [
        ("/identity/projects", json!({"name": "web"})),
        (
            "/identity/principals",
            json!({"name": "bob", "password": "bob-pass-phrase", "default_project": "web"}),
        ),
        (
            "/identity/bindings",
            json!({"principal": "bob", "role": "admin", "project": "web"}),
        ),
    ] {
        let (s, v) = call(&app, "POST", path, Some(&admin), Some(body)).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
    }
    // An admin of one project is not a cloud admin.
    let (s, v) = login(&app, json!({"name": "bob", "password": "bob-pass-phrase"})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let bob = v["token"].as_str().unwrap();
    let (s, v) = call(&app, "GET", "/identity/audit", Some(bob), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    let (s, v) = call(&app, "GET", "/identity/audit?project=web", Some(bob), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    let (s, _) = call(&app, "GET", "/identity/audit", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}
