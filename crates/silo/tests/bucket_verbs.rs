// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 CreateBucket / DeleteBucket / HeadBucket on the path-style surface,
//! driven with SigV4-signed requests as an S3 client sends them.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use chrono::Utc;
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Meta};
use sha2::{Digest, Sha256};
use silo::http;
use silo::sigv4::{self, EMPTY_PAYLOAD_SHA256};
use silo::{Client, DEFAULT_REGION};
use tower::ServiceExt;

struct World {
    _dir: tempfile::TempDir,
    app: Router,
    store: Store,
    /// Alice's project and member key.
    project: Id,
    access_key_id: String,
    secret: Vec<u8>,
    region: String,
}

fn project(store: &Store, name: &str) -> Id {
    store
        .collection::<gate::Project>()
        .create(gate::Project {
            meta: Meta::new(kind::PROJECT, name, None),
            spec: Default::default(),
            status: Default::default(),
        })
        .unwrap()
        .meta
        .id
}

fn principal(store: &Store, name: &str) -> Id {
    store
        .collection::<gate::Principal>()
        .create(gate::Principal {
            meta: Meta::new(kind::PRINCIPAL, name, None),
            spec: Default::default(),
            status: Default::default(),
        })
        .unwrap()
        .meta
        .id
}

fn bind(store: &Store, who: &Id, role: &str, project: &Id) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    store
        .collection::<gate::RoleBinding>()
        .create(gate::RoleBinding {
            meta: Meta::new(
                gate::KIND_BINDING,
                format!("{role}-{n}"),
                Some(project.clone()),
            ),
            spec: gate::RoleBindingSpec {
                principal: who.clone(),
                role: role.into(),
                scope: gate::Scope::Project {
                    project: project.clone(),
                },
            },
            status: Default::default(),
        })
        .unwrap();
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn extract_error_code(xml: &[u8]) -> String {
    let xml = String::from_utf8_lossy(xml);
    let start = xml.find("<Code>").map(|i| i + 6).unwrap_or(0);
    let end = xml.find("</Code>").unwrap_or(xml.len());
    xml[start..end].to_owned()
}

fn amz_now() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// A project-scoped caller used to create a bucket through the native API,
/// so the bucket lands in a chosen project before the S3 surface is used.
fn member_caller(store: &Store, name: &str, project_name: &str) -> Caller {
    let project = project(store, project_name);
    let who = principal(store, name);
    bind(store, &who, "member", &project);
    Caller {
        principal: who,
        principal_name: name.into(),
        project: Some(project),
        roles: vec!["member".into()],
        system: false,
    }
}

async fn world() -> World {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("store");
    let keep = keep::Client::open(&store).expect("keep");
    let region = DEFAULT_REGION.to_owned();

    let alice = member_caller(&store, "bucket-alice", "alice-proj");
    let alice_key = keep
        .create_access_key(alice.project.as_ref().unwrap(), &alice.principal, "alice")
        .unwrap();

    let client =
        Client::new(store.clone(), dir.path().join("silo")).with_max_object_bytes(1024 * 1024);
    let app = http::router_with(client, Some(keep), region.clone());

    World {
        _dir: dir,
        app,
        store,
        project: alice.project.clone().unwrap(),
        access_key_id: alice_key.access_key.access_key_id,
        secret: alice_key.secret_access_key.into_bytes(),
        region,
    }
}

impl World {
    /// A handle to the silo client over the same store, for direct checks.
    fn client(&self) -> Client {
        Client::new(self.store.clone(), self._dir.path().join("silo-unused"))
    }

    /// Create a bucket in `caller`'s project through the native JSON API.
    async fn native_create(&self, caller: &Caller, name: &str) {
        let body = serde_json::json!({ "name": name }).to_string();
        let resp = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/buckets")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::CONTENT_LENGTH, body.len().to_string())
                    .extension(caller.clone())
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "native create {name}");
    }
}

#[allow(clippy::too_many_arguments)]
async fn signed_call(
    app: &Router,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
    access_key_id: &str,
    secret: &[u8],
    region: &str,
    payload_hash: &str,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let amz_date = amz_now();
    let host = "silo.test";
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), payload_hash.into()),
        ("x-amz-date".into(), amz_date.clone()),
    ];
    if method == "PUT" || method == "POST" {
        headers.push(("content-length".into(), body.len().to_string()));
    }
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    let signed_names: Vec<String> = headers.iter().map(|(k, _)| k.clone()).collect();
    let sig = sigv4::sign_request(
        method,
        path,
        query,
        &headers,
        payload_hash,
        access_key_id,
        secret,
        region,
        &amz_date,
    );
    let date_stamp: String = amz_date[..8].to_owned();
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{date_stamp}/{region}/s3/aws4_request, SignedHeaders={}, Signature={sig}",
        signed_names.join(";")
    );
    let uri = if query.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{query}")
    };
    let mut builder = Request::builder().method(method).uri(&uri);
    for (k, v) in &headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder = builder.header(header::AUTHORIZATION, auth);
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let hdrs = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes, hdrs)
}

async fn create_bucket(w: &World, name: &str) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    signed_call(
        &w.app,
        "PUT",
        &format!("/{name}"),
        "",
        b"",
        &w.access_key_id,
        &w.secret,
        &w.region,
        EMPTY_PAYLOAD_SHA256,
    )
    .await
}

async fn create_bucket_with(
    w: &World,
    name: &str,
    key: &str,
    secret: &[u8],
    body: &[u8],
    payload_hash: &str,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    signed_call(
        &w.app,
        "PUT",
        &format!("/{name}"),
        "",
        body,
        key,
        secret,
        &w.region,
        payload_hash,
    )
    .await
}

async fn delete_bucket(w: &World, name: &str) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    signed_call(
        &w.app,
        "DELETE",
        &format!("/{name}"),
        "",
        b"",
        &w.access_key_id,
        &w.secret,
        &w.region,
        EMPTY_PAYLOAD_SHA256,
    )
    .await
}

async fn head_bucket(w: &World, name: &str) -> StatusCode {
    let (status, _, _) = signed_call(
        &w.app,
        "HEAD",
        &format!("/{name}"),
        "",
        b"",
        &w.access_key_id,
        &w.secret,
        &w.region,
        EMPTY_PAYLOAD_SHA256,
    )
    .await;
    status
}

async fn put_object(w: &World, name: &str, key: &str, bytes: &[u8]) -> StatusCode {
    let (status, body, _) = signed_call(
        &w.app,
        "PUT",
        &format!("/{name}/{key}"),
        "",
        bytes,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &sha256_hex(bytes),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "put object: {}",
        String::from_utf8_lossy(&body)
    );
    status
}

async fn start_upload(w: &World, name: &str, key: &str) {
    let (status, body, _) = signed_call(
        &w.app,
        "POST",
        &format!("/{name}/{key}"),
        "uploads",
        b"",
        &w.access_key_id,
        &w.secret,
        &w.region,
        EMPTY_PAYLOAD_SHA256,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create mpu: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn create_bucket_returns_ok_with_location() {
    let w = world().await;
    let (status, body, headers) = create_bucket(&w, "brand-new").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        headers.get(header::LOCATION).map(|v| v.to_str().unwrap()),
        Some("/brand-new")
    );
    // The record really exists, scoped to the caller's project.
    let listed = w.client().list_buckets(Some(&w.project)).unwrap();
    assert!(listed.iter().any(|b| b.meta.name == "brand-new"));
}

#[tokio::test]
async fn create_bucket_body_is_accepted_and_ignored() {
    let w = world().await;
    // A real CreateBucketConfiguration body, sent with its own correct payload
    // hash: the body is validated against the hash, then ignored (no region is
    // applied and none is rejected).
    let body = br#"<?xml version="1.0" encoding="UTF-8"?><CreateBucketConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><LocationConstraint>us-west-2</LocationConstraint></CreateBucketConfiguration>"#;
    let (status, resp, headers) = create_bucket_with(
        &w,
        "with-config",
        &w.access_key_id,
        &w.secret,
        body,
        &sha256_hex(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&resp));
    assert_eq!(
        headers.get(header::LOCATION).map(|v| v.to_str().unwrap()),
        Some("/with-config")
    );
    // The body named us-west-2; the bucket landed in the caller's project with
    // the default (unversioned) spec, so the region was not applied.
    let listed = w.client().list_buckets(Some(&w.project)).unwrap();
    let made = listed
        .iter()
        .find(|b| b.meta.name == "with-config")
        .unwrap();
    assert!(!made.spec.versioning);
}

#[tokio::test]
async fn create_bucket_of_own_name_is_ok_again() {
    let w = world().await;
    let (first, _, _) = create_bucket(&w, "twice").await;
    assert_eq!(first, StatusCode::OK);
    let (second, body, _) = create_bucket(&w, "twice").await;
    assert_eq!(second, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
}

#[tokio::test]
async fn create_bucket_taken_by_another_project_is_conflict() {
    let w = world().await;
    // A bucket in Bob's project (created through the native API) collides with
    // the global name index, so Alice's S3 create is refused: shared namespace.
    let bob = member_caller(&w.store, "collide-bob", "bob-proj2");
    w.native_create(&bob, "taken-first").await;
    let (status, body, _) = create_bucket(&w, "taken-first").await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(extract_error_code(&body), "BucketAlreadyExists");
}

#[tokio::test]
async fn create_bucket_bad_name_is_invalid() {
    let w = world().await;
    for name in ["AB", "bad_name", "x", "-lead", "UPPERCASE"] {
        let (status, body, _) = create_bucket(&w, name).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "name {name}");
        assert_eq!(
            extract_error_code(&body),
            "InvalidBucketName",
            "name {name}"
        );
    }
}

#[tokio::test]
async fn delete_empty_bucket_is_no_content() {
    let w = world().await;
    let (created, _, _) = create_bucket(&w, "gone").await;
    assert_eq!(created, StatusCode::OK);
    let (status, body, _) = delete_bucket(&w, "gone").await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    // It is really gone.
    assert_eq!(head_bucket(&w, "gone").await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_missing_bucket_is_not_found() {
    let w = world().await;
    let (status, body, _) = delete_bucket(&w, "never-made").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(extract_error_code(&body), "NoSuchBucket");
}

#[tokio::test]
async fn delete_bucket_with_object_is_not_empty() {
    let w = world().await;
    create_bucket(&w, "full").await;
    put_object(&w, "full", "a.txt", b"hello").await;
    let (status, body, _) = delete_bucket(&w, "full").await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(extract_error_code(&body), "BucketNotEmpty");
}

#[tokio::test]
async fn delete_bucket_with_unfinished_upload_is_not_empty() {
    let w = world().await;
    create_bucket(&w, "mpu").await;
    start_upload(&w, "mpu", "big.bin").await;
    let (status, body, _) = delete_bucket(&w, "mpu").await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(extract_error_code(&body), "BucketNotEmpty");
}

#[tokio::test]
async fn head_bucket_exists_and_missing() {
    let w = world().await;
    create_bucket(&w, "peek").await;
    assert_eq!(head_bucket(&w, "peek").await, StatusCode::OK);
    assert_eq!(head_bucket(&w, "absent").await, StatusCode::NOT_FOUND);
}
