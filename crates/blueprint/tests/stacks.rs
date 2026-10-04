// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Create a stack from a JSON document, apply it, and reject an empty one.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use blueprint::{Blueprint, ACCEPTED_MESSAGE};
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
    let mut req = Request::builder().method(method).uri(path);
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
    assert_eq!(applied["status"]["phase"], "accepted");
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
    assert_eq!(applied["revision"], 2);

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
