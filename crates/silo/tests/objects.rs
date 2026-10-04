// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Open a temp store, create a bucket, put an object, read it back.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use silo::http;
use silo::Client;
use tower::ServiceExt;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn project_caller(project: &Id) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "ada".into(),
        project: Some(project.clone()),
        roles: vec!["member".into()],
        system: false,
    }
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Vec<u8>>,
    content_type: &str,
    caller: Option<&Caller>,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(c) = caller {
        req = req.extension(c.clone());
    }
    let resp = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .expect("body")
        .to_vec();
    (status, bytes, headers)
}

#[tokio::test]
async fn put_and_get_object_on_temp_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("open store");
    let root = dir.path().join("silo");
    let client = Client::new(store, &root);
    let app = http::router(client.clone());

    let project = Id::new(kind::PROJECT);
    let who = project_caller(&project);

    let (status, body, _) = call(
        &app,
        "POST",
        "/buckets",
        Some(
            json!({
                "name": "widgets",
                "spec": { "versioning": false }
            })
            .to_string()
            .into_bytes(),
        ),
        "application/json",
        Some(&who),
    )
    .await;
    let bucket: Value = serde_json::from_slice(&body).expect("bucket json");
    assert_eq!(status, StatusCode::CREATED, "{bucket}");
    let bucket_id = bucket["id"].as_str().expect("id").to_owned();
    assert!(bucket_id.starts_with("bkt-"), "{bucket_id}");
    assert_eq!(bucket["name"], "widgets");
    assert_eq!(bucket["project"], project.to_string());
    assert_eq!(bucket["spec"]["versioning"], false);
    assert_eq!(bucket["status"]["phase"], "ready");

    let payload = b"hello silo".to_vec();
    let want = sha256_hex(&payload);
    let (status, body, _) = call(
        &app,
        "PUT",
        &format!("/buckets/{bucket_id}/objects/hello.txt"),
        Some(payload.clone()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    let obj: Value = serde_json::from_slice(&body).expect("object json");
    assert_eq!(status, StatusCode::CREATED, "{obj}");
    assert!(obj["id"].as_str().unwrap().starts_with("obj-"));
    assert_eq!(obj["spec"]["key"], "hello.txt");
    assert_eq!(obj["spec"]["bucket"], bucket_id);
    assert_eq!(obj["status"]["sha256"], want);
    assert_eq!(obj["status"]["size_bytes"], payload.len() as u64);
    assert_eq!(obj["project"], project.to_string());

    let disk = root.join(&bucket_id).join("hello.txt");
    assert_eq!(std::fs::read(&disk).expect("object file"), payload);

    let (status, got, headers) = call(
        &app,
        "GET",
        &format!("/buckets/{bucket_id}/objects/hello.txt"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, payload);
    assert_eq!(
        headers.get(header::ETAG).and_then(|v| v.to_str().ok()),
        Some(format!("\"{want}\"").as_str())
    );

    let nested = b"nested".to_vec();
    let (status, body, _) = call(
        &app,
        "PUT",
        &format!("/buckets/{bucket_id}/objects/dir/nested.txt"),
        Some(nested.clone()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        std::fs::read(root.join(&bucket_id).join("dir/nested.txt")).unwrap(),
        nested
    );

    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/buckets/{bucket_id}/objects"),
        None,
        "application/json",
        Some(&who),
    )
    .await;
    let list: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 2);
    let keys = list["keys"].as_array().unwrap();
    assert!(keys.iter().any(|k| k == "hello.txt"));
    assert!(keys.iter().any(|k| k == "dir/nested.txt"));

    let id = Id::parse(kind::BUCKET, &bucket_id).expect("bucket id");
    let err = client
        .put_object(&id, "../outside.txt", b"nope", None)
        .await
        .expect_err("dotdot");
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert!(!dir.path().join("outside.txt").exists());

    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/buckets/{bucket_id}/objects/%2e%2e/outside.txt"),
        Some(b"nope".to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!dir.path().join("outside.txt").exists());

    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/buckets/{bucket_id}/objects/hello.txt"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!disk.exists());
    let (status, _, _) = call(
        &app,
        "GET",
        &format!("/buckets/{bucket_id}/objects/hello.txt"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Versioning keeps the previous bytes, addressed by their sha256.
    let (status, body, _) = call(
        &app,
        "POST",
        "/buckets",
        Some(
            json!({"name": "versions", "spec": {"versioning": true}})
                .to_string()
                .into_bytes(),
        ),
        "application/json",
        Some(&who),
    )
    .await;
    let ver: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::CREATED, "{ver}");
    let vid = ver["id"].as_str().unwrap();
    let first = b"one".to_vec();
    let first_sha = sha256_hex(&first);
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/buckets/{vid}/objects/k"),
        Some(first.clone()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let second = b"two".to_vec();
    let (status, body, _) = call(
        &app,
        "PUT",
        &format!("/buckets/{vid}/objects/k"),
        Some(second.clone()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    let obj: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::CREATED, "{obj}");
    assert_eq!(obj["status"]["version"], 2);
    assert_eq!(obj["status"]["sha256"], sha256_hex(&second));
    assert_eq!(std::fs::read(root.join(vid).join("k")).unwrap(), second);
    assert_eq!(
        std::fs::read(root.join(vid).join(".blobs").join(&first_sha)).unwrap(),
        first
    );
    let (status, got, _) = call(
        &app,
        "GET",
        &format!("/buckets/{vid}/objects/k"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, second);
}

#[tokio::test]
async fn missing_caller_is_unauthorized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("open store");
    let app = http::router(Client::new(store, &dir.path().join("silo")));
    let (status, body, _) = call(&app, "GET", "/buckets", None, "application/json", None).await;
    let err: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{err}");
    assert_eq!(err["error"]["code"], "unauthorized");
    assert_eq!(err["error"]["message"], "no credentials");
}
