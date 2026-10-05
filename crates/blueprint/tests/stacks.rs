// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Create a stack from a JSON document, apply it, and reject an empty one.
//! Update a stack's spec, apply the change, and roll it back.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use blueprint::{Blueprint, ACCEPTED_MESSAGE, WORKFLOW_ACCEPTED_MESSAGE};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn router(dir: &std::path::Path) -> axum::Router {
    let (events, _rx) = tokio::sync::broadcast::channel(64);
    let store = Store::open(dir.join("blueprint.redb"), events).expect("temp store");
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: pistack_types::module::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let module = Blueprint;
    assert_eq!(module.info().name, "blueprint");
    assert_eq!(module.info().prefix, "blueprint");
    module.router(ctx)
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call_if_match(app, method, path, body, None).await
}

async fn call_if_match(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    if_match: Option<u64>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(rev) = if_match {
        req = req.header(header::IF_MATCH, rev.to_string());
    }
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = req.body(body).unwrap();
    req.extensions_mut().insert(admin_caller());
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, v)
}

#[tokio::test]
async fn create_apply_and_reject_empty_document() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = router(dir.path());
    let document = r#"{"kind":"network","name":"app","spec":{}}"#;

    let (status, created) = call(
        &app,
        "POST",
        "/stacks",
        Some(json!({
            "name": "web",
            "spec": {
                "name": "web",
                "document": document,
                "parameters": {"size": "small"}
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap();
    assert!(id.starts_with("stack-"), "{id}");
    assert_eq!(created["spec"]["document"], document);
    assert_eq!(created["spec"]["parameters"]["size"], "small");
    assert_eq!(created["status"]["phase"], "accepted");
    assert_eq!(created["status"]["message"], ACCEPTED_MESSAGE);
    assert_eq!(created["revision"], 1);

    let (status, applied) = call(&app, "POST", &format!("/stacks/{id}/apply"), None).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["id"], id);
    assert_eq!(applied["status"]["phase"], "ready");
    assert_eq!(applied["status"]["revision"], 1);
    let created_ids = applied["status"]["created"].as_array().expect("created");
    assert_eq!(created_ids.len(), 1, "{applied}");
    let net_id = created_ids[0].as_str().unwrap();
    assert!(net_id.starts_with("net-"), "{net_id}");
    assert!(
        applied["status"]["message"]
            .as_str()
            .unwrap()
            .contains(net_id),
        "{applied}"
    );
    assert_eq!(applied["spec"]["document"], document);
    // One write marks the stack applying, one stores the result.
    assert_eq!(applied["revision"], 3);

    let (status, rejected) = call(
        &app,
        "POST",
        "/stacks",
        Some(json!({
            "name": "empty",
            "spec": {"name": "empty", "document": ""}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
    assert_eq!(rejected["error"]["code"], "bad_request");
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("empty"),
        "{rejected}"
    );
}

#[tokio::test]
async fn kit_and_workflow_apply_over_http() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = router(dir.path());

    let (status, info) = call(&app, "GET", "/", None).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    let resources = info["resources"].as_array().expect("resources");
    assert!(resources.iter().any(|v| v == "stacks"));
    assert!(resources.iter().any(|v| v == "kits"));
    assert!(resources.iter().any(|v| v == "workflows"));

    let (status, kit) = call(
        &app,
        "POST",
        "/kits",
        Some(json!({
            "name": "base",
            "spec": {
                "name": "base",
                "document": "kind: network\nname: kit-net\nspec: {}\n"
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{kit}");
    assert_eq!(kit["status"]["message"], ACCEPTED_MESSAGE);
    let kit_id = kit["id"].as_str().unwrap();
    assert!(kit_id.starts_with("kit-"), "{kit_id}");

    let (status, applied) = call(&app, "POST", &format!("/kits/{kit_id}/apply"), None).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["status"]["phase"], "ready");
    assert!(applied["status"].get("history").is_none(), "{applied}");
    assert_eq!(applied["status"]["created"].as_array().unwrap().len(), 1);

    let (status, again) = call(&app, "POST", &format!("/kits/{kit_id}/apply"), None).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert!(again["status"]["created"].as_array().unwrap().is_empty());
    assert!(again["status"]["message"]
        .as_str()
        .unwrap()
        .contains("present"));

    let (status, first) = call(
        &app,
        "POST",
        "/stacks",
        Some(json!({
            "name": "first",
            "spec": {
                "name": "first",
                "document": "kind: network\nname: app\nspec: {}\n"
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    let (status, bad) = call(
        &app,
        "POST",
        "/stacks",
        Some(json!({
            "name": "bad",
            "spec": {
                "name": "bad",
                "document": "kind: machine\nname: broken\nspec: {}\n"
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{bad}");

    let (status, wf) = call(
        &app,
        "POST",
        "/workflows",
        Some(json!({
            "name": "rollout",
            "spec": {
                "name": "rollout",
                "stacks": ["first", "bad"]
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wf}");
    assert_eq!(wf["status"]["message"], WORKFLOW_ACCEPTED_MESSAGE);
    let wf_id = wf["id"].as_str().unwrap();
    assert!(wf_id.starts_with("workflow-"), "{wf_id}");

    let (status, stopped) = call(&app, "POST", &format!("/workflows/{wf_id}/apply"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{stopped}");
    assert_eq!(stopped["error"]["code"], "bad_request");
    assert!(
        stopped["error"]["message"]
            .as_str()
            .unwrap()
            .contains("stopped on stack bad"),
        "{stopped}"
    );

    let (status, stored) = call(&app, "GET", &format!("/workflows/{wf_id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(stored["status"]["phase"], "failed");
    assert_eq!(stored["status"]["applied"], json!(["first"]));
}

#[tokio::test]
async fn update_apply_and_roll_back_over_http() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = router(dir.path());
    let v1 = "kind: network\nname: app\nspec: {}\n";
    let v2 = "kind: network\nname: app\nspec:\n  mtu: 1400\n";

    let (status, created) = call(
        &app,
        "POST",
        "/stacks",
        Some(json!({"name": "web", "spec": {"name": "web", "document": v1}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();

    let (status, none) = call(&app, "POST", &format!("/stacks/{id}/rollback"), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{none}");

    let (status, first) = call(&app, "POST", &format!("/stacks/{id}/apply"), None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["status"]["phase"], "ready");
    let net_id = first["status"]["created"][0].as_str().unwrap().to_owned();
    assert_eq!(first["status"]["managed"], json!([net_id]));
    let rev = first["revision"].as_u64().unwrap();

    // A stale If-Match is refused and changes nothing.
    let (status, stale) = call_if_match(
        &app,
        "PUT",
        &format!("/stacks/{id}"),
        Some(json!({"spec": {"name": "web", "document": v2}})),
        Some(rev - 1),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");

    let (status, empty) = call(
        &app,
        "PUT",
        &format!("/stacks/{id}"),
        Some(json!({"spec": {"name": "web", "document": ""}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{empty}");

    let (status, edited) = call_if_match(
        &app,
        "PUT",
        &format!("/stacks/{id}"),
        Some(json!({
            "description": "bigger mtu",
            "spec": {"name": "web", "document": v2}
        })),
        Some(rev),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["spec"]["document"], v2);
    assert_eq!(edited["description"], "bigger mtu");
    assert_eq!(edited["status"]["revision"], 1, "PUT does not apply");

    let (status, second) = call(&app, "POST", &format!("/stacks/{id}/apply"), None).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["status"]["phase"], "ready");
    assert_eq!(second["status"]["revision"], 2);
    assert_eq!(second["status"]["updated"], json!([net_id]));
    assert!(second["status"]["created"].as_array().unwrap().is_empty());
    assert_eq!(second["status"]["history"].as_array().unwrap().len(), 2);

    let (status, bad) = call(
        &app,
        "POST",
        &format!("/stacks/{id}/rollback"),
        Some(json!({"revision": 99})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{bad}");

    let (status, back) = call(&app, "POST", &format!("/stacks/{id}/rollback"), None).await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert_eq!(back["spec"]["document"], v1);
    assert_eq!(back["status"]["phase"], "ready");
    assert_eq!(back["status"]["revision"], 3);
    assert_eq!(back["status"]["updated"], json!([net_id]));
    let history = back["status"]["history"].as_array().unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[2]["restored_from"], 1);
    assert_eq!(history[2]["spec"]["document"], v1);

    let (status, forward) = call(
        &app,
        "POST",
        &format!("/stacks/{id}/rollback"),
        Some(json!({"revision": 2})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{forward}");
    assert_eq!(forward["spec"]["document"], v2);
    assert_eq!(forward["status"]["revision"], 4);
    assert_eq!(forward["status"]["history"][3]["restored_from"], 2);

    // A change apply cannot make leaves the stack failed with the reason.
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/stacks/{id}"),
        Some(json!({"spec": {"name": "web", "document": "kind: network\nname: app\nspec:\n  kind: vlan\n"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, failed) = call(&app, "POST", &format!("/stacks/{id}/apply"), None).await;
    assert!(status.is_client_error(), "{status} {failed}");
    let (status, stored) = call(&app, "GET", &format!("/stacks/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored["status"]["phase"], "failed", "{stored}");
    assert!(stored["status"]["message"]
        .as_str()
        .unwrap()
        .starts_with("apply failed:"));
    assert_eq!(stored["status"]["revision"], 4);
}

fn admin_caller() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

#[tokio::test]
async fn missing_caller_is_unauthorized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = router(dir.path());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/stacks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    assert_eq!(body["error"]["message"], "no credentials");
}
