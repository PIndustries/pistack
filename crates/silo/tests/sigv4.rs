// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end SigV4 against a keep-issued access key.

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
use silo::sigv4::{self, EMPTY_PAYLOAD_SHA256, UNSIGNED_PAYLOAD};
use silo::{Client, DEFAULT_REGION};
use tower::ServiceExt;

struct World {
    _dir: tempfile::TempDir,
    app: Router,
    bucket: String,
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

fn extract_error_code(xml: &str) -> String {
    let start = xml.find("<Code>").map(|i| i + 6).unwrap_or(0);
    let end = xml.find("</Code>").unwrap_or(xml.len());
    xml[start..end].to_owned()
}

async fn world() -> World {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("store");
    let keep = keep::Client::open(&store).expect("keep");
    let project = project(&store, "sigv4-proj");
    let alice = principal(&store, "sigv4-alice");
    bind(&store, &alice, "member", &project);
    let created = keep
        .create_access_key(&project, &alice, "sigv4-test")
        .expect("access key");
    let region = DEFAULT_REGION.to_owned();
    let silo_root = dir.path().join("silo");
    let client = Client::new(store.clone(), &silo_root).with_max_object_bytes(1024 * 1024);
    let app = http::router_with(client, Some(keep.clone()), region.clone());

    // Create bucket with a project-scoped caller (native API).
    let who = Caller {
        principal: alice.clone(),
        principal_name: "sigv4-alice".into(),
        project: Some(project.clone()),
        roles: vec!["member".into()],
        system: false,
    };
    let body = serde_json::json!({
        "name": "widgets",
        "spec": { "versioning": false }
    })
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/buckets")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len().to_string())
        .extension(who)
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create bucket");

    World {
        _dir: dir,
        app,
        bucket: "widgets".into(),
        access_key_id: created.access_key.access_key_id,
        secret: created.secret_access_key.into_bytes(),
        region,
    }
}

fn amz_now() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
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
    amz_date: &str,
    payload_hash: &str,
    host: &str,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), payload_hash.into()),
        ("x-amz-date".into(), amz_date.into()),
    ];
    for (k, v) in extra_headers {
        headers.push(((*k).into(), (*v).into()));
    }
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
        amz_date,
    );
    let date_stamp: String = amz_date.chars().take(8).collect();
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

#[tokio::test]
async fn signed_put_get_round_trip() {
    let w = world().await;
    let payload = b"hello sigv4";
    let hash = sha256_hex(payload);
    let date = amz_now();
    let path = format!("/{}/greeting.txt", w.bucket);
    let (status, _, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        payload,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &date,
        &hash,
        "silo.test",
        &[("content-type", "text/plain")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "signed put");

    let (status, body, _) = signed_call(
        &w.app,
        "GET",
        &path,
        "",
        b"",
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_now(),
        EMPTY_PAYLOAD_SHA256,
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "signed get");
    assert_eq!(body, payload);
}

#[tokio::test]
async fn bad_signature_rejected() {
    let w = world().await;
    let date = amz_now();
    let path = format!("/{}/x.txt", w.bucket);
    let (status, body, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        b"x",
        &w.access_key_id,
        b"wrong-secret-wrong-secret-wrong-secret!!",
        &w.region,
        &date,
        &sha256_hex(b"x"),
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "SignatureDoesNotMatch"
    );
}

#[tokio::test]
async fn unknown_key_rejected() {
    let w = world().await;
    let date = amz_now();
    let path = format!("/{}/x.txt", w.bucket);
    let (status, body, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        b"x",
        "PIAKAAAAAAAAAAAAAAAA",
        &w.secret,
        &w.region,
        &date,
        &sha256_hex(b"x"),
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "InvalidAccessKeyId"
    );
}

#[tokio::test]
async fn skewed_clock_rejected() {
    let w = world().await;
    let date = (Utc::now() - chrono::Duration::hours(1))
        .format("%Y%m%dT%H%M%SZ")
        .to_string();
    let path = format!("/{}/x.txt", w.bucket);
    let (status, body, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        b"x",
        &w.access_key_id,
        &w.secret,
        &w.region,
        &date,
        &sha256_hex(b"x"),
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "RequestTimeTooSkewed"
    );
}

#[tokio::test]
async fn wrong_region_rejected() {
    let w = world().await;
    let date = amz_now();
    let path = format!("/{}/x.txt", w.bucket);
    let (status, body, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        b"x",
        &w.access_key_id,
        &w.secret,
        "us-east-1",
        &date,
        &sha256_hex(b"x"),
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "AuthorizationHeaderMalformed"
    );
}

#[tokio::test]
async fn payload_hash_mismatch_discards() {
    let w = world().await;
    let date = amz_now();
    let path = format!("/{}/badhash.txt", w.bucket);
    let real = b"actual-bytes";
    let lie = sha256_hex(b"other-bytes");
    let (status, body, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        real,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &date,
        &lie,
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "XAmzContentSHA256Mismatch"
    );
    // Object must not exist for a subsequent signed GET.
    let (status, _, _) = signed_call(
        &w.app,
        "GET",
        &path,
        "",
        b"",
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_now(),
        EMPTY_PAYLOAD_SHA256,
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn presigned_get() {
    let w = world().await;
    // Seed object with a signed PUT.
    let payload = b"presigned-body";
    let path = format!("/{}/pre.txt", w.bucket);
    let (status, _, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        payload,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_now(),
        &sha256_hex(payload),
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let date = amz_now();
    let qs = sigv4::presign_query(
        "GET",
        &path,
        "silo.test",
        &w.access_key_id,
        &w.secret,
        &w.region,
        &date,
        3600,
        "",
    );
    let uri = format!("{path}?{qs}");
    let req = Request::builder()
        .method("GET")
        .uri(&uri)
        .header("host", "silo.test")
        .body(Body::empty())
        .unwrap();
    let resp = w.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(&body[..], payload);
}

#[tokio::test]
async fn streaming_payload_not_implemented() {
    let w = world().await;
    let date = amz_now();
    let path = format!("/{}/chunked.txt", w.bucket);
    let (status, body, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        b"",
        &w.access_key_id,
        &w.secret,
        &w.region,
        &date,
        "STREAMING-AWS4-HMAC-SHA256-PAYLOAD",
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "NotImplemented"
    );
}

#[tokio::test]
async fn unsigned_payload_put_ok() {
    let w = world().await;
    let payload = b"unsigned";
    let path = format!("/{}/unsigned.txt", w.bucket);
    let (status, _, _) = signed_call(
        &w.app,
        "PUT",
        &path,
        "",
        payload,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_now(),
        UNSIGNED_PAYLOAD,
        "silo.test",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
