// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Open a temp store, create a bucket, put an object, read it back.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use md5::Md5;
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

fn md5_hex(bytes: &[u8]) -> String {
    use md5::Digest;
    Md5::digest(bytes)
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
    let payload = body.unwrap_or_default();
    if method == "PUT" || method == "POST" {
        req = req.header(header::CONTENT_LENGTH, payload.len().to_string());
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(payload)).expect("request"))
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

async fn call_with_headers(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Vec<u8>>,
    content_type: &str,
    caller: Option<&Caller>,
    extra: &[(&str, &str)],
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(c) = caller {
        req = req.extension(c.clone());
    }
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let payload = body.unwrap_or_default();
    if method == "PUT" || method == "POST" {
        req = req.header(header::CONTENT_LENGTH, payload.len().to_string());
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(payload)).expect("request"))
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

fn app_with_max(max: usize) -> (tempfile::TempDir, Client, Router, Id, Caller) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("open store");
    let root = dir.path().join("silo");
    let client = Client::new(store, &root).with_max_object_bytes(max);
    let app = http::router(client.clone());
    let project = Id::new(kind::PROJECT);
    let who = project_caller(&project);
    (dir, client, app, project, who)
}

#[tokio::test]
async fn put_and_get_object_on_temp_store() {
    let (dir, client, app, project, who) = app_with_max(silo::MAX_OBJECT_BYTES);
    let root = dir.path().join("silo");

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
    let want_md5 = md5_hex(&payload);
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
    assert_eq!(obj["status"]["md5"], want_md5);
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
        Some(format!("\"{want_md5}\"").as_str())
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
    let app = http::router(Client::new(store, dir.path().join("silo")));
    let (status, body, _) = call(&app, "GET", "/buckets", None, "application/json", None).await;
    let err: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{err}");
    assert_eq!(err["error"]["code"], "unauthorized");
    assert_eq!(err["error"]["message"], "no credentials");
}

#[tokio::test]
async fn configured_max_rejects_oversize_without_writing_gib() {
    // Cap at 64 bytes so the test stays tiny.
    let (_dir, client, app, _project, who) = app_with_max(64);

    let (status, body, _) = call(
        &app,
        "POST",
        "/buckets",
        Some(json!({"name": "tiny"}).to_string().into_bytes()),
        "application/json",
        Some(&who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let bucket: Value = serde_json::from_slice(&body).unwrap();
    let bucket_id = bucket["id"].as_str().unwrap();

    let too_big = vec![b'x'; 65];
    let (status, body, _) = call(
        &app,
        "PUT",
        &format!("/buckets/{bucket_id}/objects/big.bin"),
        Some(too_big.clone()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let err: Value = serde_json::from_slice(&body).unwrap();
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("larger than"),
        "{err}"
    );

    let id = Id::parse(kind::BUCKET, bucket_id).unwrap();
    let err = client
        .put_object(&id, "via-client.bin", &too_big, None)
        .await
        .expect_err("oversize");
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);

    // Exactly at the limit still works.
    let ok = vec![b'y'; 64];
    let (status, body, _) = call(
        &app,
        "PUT",
        &format!("/buckets/{bucket_id}/objects/ok.bin"),
        Some(ok.clone()),
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
    let (status, got, _) = call(
        &app,
        "GET",
        &format!("/buckets/{bucket_id}/objects/ok.bin"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, ok);
}

#[tokio::test]
async fn streaming_put_get_round_trip() {
    let (dir, _client, app, _project, who) = app_with_max(1024 * 1024);
    let root = dir.path().join("silo");

    let (status, body, _) = call(
        &app,
        "POST",
        "/buckets",
        Some(json!({"name": "stream"}).to_string().into_bytes()),
        "application/json",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let bucket: Value = serde_json::from_slice(&body).unwrap();
    let bucket_id = bucket["id"].as_str().unwrap().to_owned();

    // Multi-chunk-sized payload (not huge, but larger than a single write buffer).
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let want_sha = sha256_hex(&payload);
    let want_md5 = md5_hex(&payload);

    let (status, body, _) = call(
        &app,
        "PUT",
        &format!("/buckets/{bucket_id}/objects/chunky.bin"),
        Some(payload.clone()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    let obj: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::CREATED, "{obj}");
    assert_eq!(obj["status"]["sha256"], want_sha);
    assert_eq!(obj["status"]["md5"], want_md5);
    assert_eq!(obj["status"]["size_bytes"], payload.len() as u64);
    assert_eq!(
        std::fs::read(root.join(&bucket_id).join("chunky.bin")).unwrap(),
        payload
    );

    let (status, got, headers) = call(
        &app,
        "GET",
        &format!("/buckets/{bucket_id}/objects/chunky.bin"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, payload);
    assert_eq!(
        headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()),
        Some(payload.len().to_string().as_str())
    );
}

#[tokio::test]
async fn s3_put_get_list_prefix_and_pagination() {
    let (_dir, _client, app, _project, who) = app_with_max(1024 * 1024);

    let (status, body, _) = call(
        &app,
        "POST",
        "/buckets",
        Some(json!({"name": "photos"}).to_string().into_bytes()),
        "application/json",
        Some(&who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let a = b"alpha".to_vec();
    let b = b"bravo".to_vec();
    let c = b"charlie".to_vec();
    for (key, payload) in [("docs/a.txt", &a), ("docs/b.txt", &b), ("other/c.txt", &c)] {
        let (status, body, headers) = call(
            &app,
            "PUT",
            &format!("/photos/{key}"),
            Some(payload.clone()),
            "text/plain",
            Some(&who),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} {}",
            key,
            String::from_utf8_lossy(&body)
        );
        let etag = headers
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .unwrap();
        assert_eq!(etag, format!("\"{}\"", md5_hex(payload)));
    }

    let (status, got, headers) = call(
        &app,
        "GET",
        "/photos/docs/a.txt",
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, a);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/plain")
    );

    let (status, _, headers) = call(
        &app,
        "HEAD",
        "/photos/docs/a.txt",
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()),
        Some("5")
    );

    let (status, body, headers) = call(
        &app,
        "GET",
        "/photos?list-type=2&prefix=docs/",
        None,
        "application/xml",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/xml")
    );
    let xml = String::from_utf8_lossy(&body);
    assert!(xml.contains("<Key>docs/a.txt</Key>"), "{xml}");
    assert!(xml.contains("<Key>docs/b.txt</Key>"), "{xml}");
    assert!(!xml.contains("other/c.txt"), "{xml}");
    assert!(xml.contains("<IsTruncated>false</IsTruncated>"), "{xml}");

    let (status, body, _) = call(
        &app,
        "GET",
        "/photos?list-type=2&max-keys=1",
        None,
        "application/xml",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let xml = String::from_utf8_lossy(&body);
    assert!(xml.contains("<IsTruncated>true</IsTruncated>"), "{xml}");
    assert!(xml.contains("<NextContinuationToken>"), "{xml}");
    // First key lexicographically among the three.
    assert!(xml.contains("<Key>docs/a.txt</Key>"), "{xml}");
    assert!(!xml.contains("docs/b.txt"), "{xml}");

    // Continue from the token.
    let token = xml
        .split("<NextContinuationToken>")
        .nth(1)
        .unwrap()
        .split("</NextContinuationToken>")
        .next()
        .unwrap();
    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/photos?list-type=2&max-keys=1&continuation-token={token}"),
        None,
        "application/xml",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let xml = String::from_utf8_lossy(&body);
    assert!(xml.contains("<Key>docs/b.txt</Key>"), "{xml}");

    let (status, _, _) = call(
        &app,
        "DELETE",
        "/photos/docs/a.txt",
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body, _) = call(
        &app,
        "GET",
        "/photos/docs/a.txt",
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        String::from_utf8_lossy(&body).contains("NoSuchKey")
            || String::from_utf8_lossy(&body).contains("not found")
    );
}

fn app_multipart(max: usize, min_part: usize) -> (tempfile::TempDir, Client, Router, Id, Caller) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("open store");
    let root = dir.path().join("silo");
    let client = Client::new(store, &root)
        .with_max_object_bytes(max)
        .with_min_part_bytes(min_part);
    let app = http::router(client.clone());
    let project = Id::new(kind::PROJECT);
    let who = project_caller(&project);
    (dir, client, app, project, who)
}

async fn ensure_bucket(app: &Router, who: &Caller, name: &str) -> String {
    let (status, body, _) = call(
        app,
        "POST",
        "/buckets",
        Some(
            json!({
                "name": name,
                "spec": { "versioning": false }
            })
            .to_string()
            .into_bytes(),
        ),
        "application/json",
        Some(who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let bucket: Value = serde_json::from_slice(&body).expect("bucket json");
    bucket["name"].as_str().unwrap().to_owned()
}

fn extract_upload_id(xml: &str) -> String {
    xml.split("<UploadId>")
        .nth(1)
        .expect("UploadId")
        .split("</UploadId>")
        .next()
        .unwrap()
        .to_owned()
}

fn extract_error_code(xml: &str) -> String {
    xml.split("<Code>")
        .nth(1)
        .unwrap_or("")
        .split("</Code>")
        .next()
        .unwrap_or("")
        .to_owned()
}

fn complete_xml(parts: &[(u32, &str)]) -> Vec<u8> {
    let mut xml =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><CompleteMultipartUpload>");
    for (n, etag) in parts {
        xml.push_str(&format!(
            "<Part><PartNumber>{n}</PartNumber><ETag>\"{etag}\"</ETag></Part>"
        ));
    }
    xml.push_str("</CompleteMultipartUpload>");
    xml.into_bytes()
}

#[tokio::test]
async fn multipart_create_upload_complete_round_trip() {
    // Small min part (8 bytes) and max (64) so tests stay tiny.
    let (dir, _client, app, _project, who) = app_multipart(64, 8);
    let bucket = ensure_bucket(&app, &who, "mpu").await;
    let key = "big.bin";
    let part1 = b"abcdefgh"; // 8
    let part2 = b"ijklmnop"; // 8
    let expected: Vec<u8> = part1.iter().chain(part2.iter()).copied().collect();

    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let xml = String::from_utf8_lossy(&body);
    assert!(xml.contains("InitiateMultipartUploadResult"), "{xml}");
    let upload_id = extract_upload_id(&xml);

    let (status, _, headers) = call(
        &app,
        "PUT",
        &format!("/{bucket}/{key}?partNumber=1&uploadId={upload_id}"),
        Some(part1.to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag1 = headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .trim_matches('"')
        .to_owned();
    assert_eq!(etag1, md5_hex(part1));

    let (status, _, headers) = call(
        &app,
        "PUT",
        &format!("/{bucket}/{key}?partNumber=2&uploadId={upload_id}"),
        Some(part2.to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag2 = headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .trim_matches('"')
        .to_owned();

    let (status, body, headers) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploadId={upload_id}"),
        Some(complete_xml(&[(1, &etag1), (2, &etag2)])),
        "application/xml",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let xml = String::from_utf8_lossy(&body);
    assert!(xml.contains("CompleteMultipartUploadResult"), "{xml}");
    let etag = headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert!(etag.contains('-'), "{etag}");
    assert!(etag.ends_with("-2\"") || etag.ends_with("-2"), "{etag}");

    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/{bucket}/{key}"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, expected);

    // Staging cleaned.
    let uploads = dir.path().join("silo");
    let found = std::fs::read_dir(&uploads).unwrap().any(|e| {
        let p = e.unwrap().path();
        p.join(".uploads").exists()
            && std::fs::read_dir(p.join(".uploads"))
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
    });
    assert!(!found, "staging dir should be empty after complete");
}

#[tokio::test]
async fn multipart_wrong_etag_is_invalid_part() {
    let (_dir, _client, app, _project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "bad-etag").await;
    let key = "x.bin";
    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));

    let part = b"data";
    let (status, _, headers) = call(
        &app,
        "PUT",
        &format!("/{bucket}/{key}?partNumber=1&uploadId={upload_id}"),
        Some(part.to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _real = headers.get(header::ETAG).unwrap();

    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploadId={upload_id}"),
        Some(complete_xml(&[(1, "00000000000000000000000000000000")])),
        "application/xml",
        Some(&who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "InvalidPart"
    );
}

#[tokio::test]
async fn multipart_out_of_order_is_invalid_part_order() {
    let (_dir, _client, app, _project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "order").await;
    let key = "y.bin";
    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));
    assert_eq!(status, StatusCode::OK);

    let mut etags = Vec::new();
    for (n, bytes) in [(1u32, b"aaaa" as &[u8]), (2, b"bbbb")] {
        let (status, _, headers) = call(
            &app,
            "PUT",
            &format!("/{bucket}/{key}?partNumber={n}&uploadId={upload_id}"),
            Some(bytes.to_vec()),
            "application/octet-stream",
            Some(&who),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        etags.push(
            headers
                .get(header::ETAG)
                .unwrap()
                .to_str()
                .unwrap()
                .trim_matches('"')
                .to_owned(),
        );
    }

    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploadId={upload_id}"),
        Some(complete_xml(&[(2, &etags[1]), (1, &etags[0])])),
        "application/xml",
        Some(&who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "InvalidPartOrder"
    );
}

#[tokio::test]
async fn multipart_abort_cleans_staging() {
    let (dir, client, app, _project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "abort").await;
    let key = "z.bin";
    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/{bucket}/{key}?partNumber=1&uploadId={upload_id}"),
        Some(b"part".to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Staging exists before abort.
    let bkt = client
        .list_buckets(Some(&who.project.clone().unwrap()))
        .unwrap()
        .into_iter()
        .find(|b| b.meta.name == bucket)
        .unwrap();
    let staging = dir
        .path()
        .join("silo")
        .join(bkt.meta.id.to_string())
        .join(".uploads")
        .join(&upload_id);
    assert!(staging.exists(), "{}", staging.display());

    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/{bucket}/{key}?uploadId={upload_id}"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!staging.exists(), "staging should be gone after abort");
}

#[tokio::test]
async fn multipart_oversize_total_rejected() {
    // max 12 bytes; two 8-byte parts = 16 > 12.
    let (_dir, _client, app, _project, who) = app_multipart(12, 4);
    let bucket = ensure_bucket(&app, &who, "oversize").await;
    let key = "too-big.bin";
    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));
    assert_eq!(status, StatusCode::OK);

    let mut etags = Vec::new();
    for (n, bytes) in [(1u32, b"12345678" as &[u8]), (2, b"abcdefgh")] {
        let (status, _, headers) = call(
            &app,
            "PUT",
            &format!("/{bucket}/{key}?partNumber={n}&uploadId={upload_id}"),
            Some(bytes.to_vec()),
            "application/octet-stream",
            Some(&who),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "part {n}");
        etags.push(
            headers
                .get(header::ETAG)
                .unwrap()
                .to_str()
                .unwrap()
                .trim_matches('"')
                .to_owned(),
        );
    }

    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/{key}?uploadId={upload_id}"),
        Some(complete_xml(&[(1, &etags[0]), (2, &etags[1])])),
        "application/xml",
        Some(&who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "EntityTooLarge"
    );
}

// ---- ListMultipartUploads ------------------------------------------------------

fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_owned())
}

/// `(key, upload_id)` for every `<Upload>` in a ListMultipartUploadsResult.
fn listed_uploads(xml: &str) -> Vec<(String, String)> {
    xml.split("<Upload>")
        .skip(1)
        .map(|block| {
            let block = block.split("</Upload>").next().unwrap();
            (
                xml_tag(block, "Key").expect("Key"),
                xml_tag(block, "UploadId").expect("UploadId"),
            )
        })
        .collect()
}

async fn create_upload(app: &Router, who: &Caller, bucket: &str, key: &str) -> String {
    let (status, body, _) = call(
        app,
        "POST",
        &format!("/{bucket}/{key}?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    extract_upload_id(&String::from_utf8_lossy(&body))
}

async fn list_uploads(app: &Router, who: &Caller, uri: &str) -> String {
    let (status, body, headers) = call(app, "GET", uri, None, "", Some(who)).await;
    let xml = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(status, StatusCode::OK, "{xml}");
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/xml"
    );
    assert!(xml.contains("<ListMultipartUploadsResult"), "{xml}");
    xml
}

#[tokio::test]
async fn list_multipart_uploads_prefix_and_pagination() {
    let (_dir, _client, app, _project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "uploads").await;

    let a1 = create_upload(&app, &who, &bucket, "logs/a.bin").await;
    let a2 = create_upload(&app, &who, &bucket, "logs/a.bin").await;
    let b = create_upload(&app, &who, &bucket, "logs/b.bin").await;
    let other = create_upload(&app, &who, &bucket, "other.bin").await;
    // Within a key, uploads are listed in upload-id order.
    let mut a_ids = [a1, a2];
    a_ids.sort();

    // Everything, ordered by key then upload id.
    let xml = list_uploads(&app, &who, &format!("/{bucket}?uploads")).await;
    assert_eq!(xml_tag(&xml, "Bucket").as_deref(), Some(bucket.as_str()));
    assert_eq!(xml_tag(&xml, "MaxUploads").as_deref(), Some("1000"));
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));
    assert!(xml.contains("<Initiated>"), "{xml}");
    assert_eq!(
        listed_uploads(&xml),
        vec![
            ("logs/a.bin".to_owned(), a_ids[0].clone()),
            ("logs/a.bin".to_owned(), a_ids[1].clone()),
            ("logs/b.bin".to_owned(), b.clone()),
            ("other.bin".to_owned(), other.clone()),
        ]
    );

    // Prefix filter.
    let xml = list_uploads(&app, &who, &format!("/{bucket}?uploads&prefix=logs/")).await;
    assert_eq!(xml_tag(&xml, "Prefix").as_deref(), Some("logs/"));
    let keys: Vec<String> = listed_uploads(&xml).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, vec!["logs/a.bin", "logs/a.bin", "logs/b.bin"]);

    // Page 1: two of the three under the prefix.
    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&prefix=logs/&max-uploads=2"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("true"));
    assert_eq!(xml_tag(&xml, "MaxUploads").as_deref(), Some("2"));
    let page1 = listed_uploads(&xml);
    assert_eq!(
        page1,
        vec![
            ("logs/a.bin".to_owned(), a_ids[0].clone()),
            ("logs/a.bin".to_owned(), a_ids[1].clone()),
        ]
    );
    let next_key = xml_tag(&xml, "NextKeyMarker").expect("NextKeyMarker");
    let next_id = xml_tag(&xml, "NextUploadIdMarker").expect("NextUploadIdMarker");
    assert_eq!(next_key, "logs/a.bin");
    assert_eq!(next_id, a_ids[1]);

    // Page 2 resumes from the markers.
    let xml = list_uploads(
        &app,
        &who,
        &format!(
            "/{bucket}?uploads&prefix=logs/&max-uploads=2&key-marker={next_key}&upload-id-marker={next_id}"
        ),
    )
    .await;
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));
    assert_eq!(xml_tag(&xml, "KeyMarker").as_deref(), Some("logs/a.bin"));
    assert_eq!(xml_tag(&xml, "UploadIdMarker"), Some(next_id.clone()));
    assert!(xml_tag(&xml, "NextKeyMarker").is_none(), "{xml}");
    assert_eq!(
        listed_uploads(&xml),
        vec![("logs/b.bin".to_owned(), b.clone())]
    );

    // A marker that splits one key's uploads keeps the later one.
    let xml = list_uploads(
        &app,
        &who,
        &format!(
            "/{bucket}?uploads&max-uploads=1&key-marker=logs/a.bin&upload-id-marker={}",
            a_ids[0]
        ),
    )
    .await;
    assert_eq!(
        listed_uploads(&xml),
        vec![("logs/a.bin".to_owned(), a_ids[1].clone())]
    );
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("true"));

    // key-marker alone skips every upload for that key.
    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&key-marker=logs/a.bin"),
    )
    .await;
    let keys: Vec<String> = listed_uploads(&xml).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, vec!["logs/b.bin", "other.bin"]);

    // upload-id-marker without key-marker is ignored.
    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&upload-id-marker={}", a_ids[1]),
    )
    .await;
    assert_eq!(listed_uploads(&xml).len(), 4);

    // Aborted uploads drop out of the listing.
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/{bucket}/other.bin?uploadId={other}"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let xml = list_uploads(&app, &who, &format!("/{bucket}?uploads")).await;
    assert!(!xml.contains(&other), "{xml}");
    assert_eq!(listed_uploads(&xml).len(), 3);
}

#[tokio::test]
async fn list_multipart_uploads_auth_and_bad_args() {
    let (_dir, _client, app, _project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "private").await;
    create_upload(&app, &who, &bucket, "k.bin").await;

    // No caller: rejected by the router's gate.
    let (status, _, _) = call(&app, "GET", &format!("/{bucket}?uploads"), None, "", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A caller from another project cannot see the bucket.
    let stranger = project_caller(&Id::new(kind::PROJECT));
    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/{bucket}?uploads"),
        None,
        "",
        Some(&stranger),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "NoSuchBucket"
    );

    for query in ["max-uploads=abc", "max-uploads=-1"] {
        let (status, body, _) = call(
            &app,
            "GET",
            &format!("/{bucket}?uploads&{query}"),
            None,
            "",
            Some(&who),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{query}: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

// ---- Abandoned-upload sweep -----------------------------------------------------

const DAY: u64 = 24 * 60 * 60;

fn gc_fixture() -> (tempfile::TempDir, Client, Id) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("open store");
    let client = Client::new(store, dir.path().join("silo"))
        .with_max_object_bytes(64)
        .with_min_part_bytes(4);
    let project = Id::new(kind::PROJECT);
    let bucket = client
        .create_bucket(
            pistack_types::Meta::new(kind::BUCKET, "gc", Some(project)),
            silo::BucketSpec::default(),
        )
        .expect("bucket");
    (dir, client, bucket.meta.id)
}

async fn staged_upload(client: &Client, bucket: &Id, key: &str) -> String {
    let id = client
        .create_multipart_upload(bucket, key, None)
        .await
        .expect("create upload");
    let body = futures::stream::once(async { Ok::<_, std::io::Error>(bytes::Bytes::from("part")) });
    client
        .upload_part(bucket, key, &id, 1, body, Some(4), None)
        .await
        .expect("upload part");
    id
}

fn staging_dir(client: &Client, bucket: &Id, upload_id: &str) -> std::path::PathBuf {
    client
        .root()
        .join(bucket.to_string())
        .join(".uploads")
        .join(upload_id)
}

/// Rewrite `meta.json` so the upload looks initiated at `at`.
fn backdate_meta(dir: &std::path::Path, at: chrono::DateTime<chrono::Utc>) {
    let path = dir.join("meta.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    meta["initiated"] = json!(at);
    std::fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();
}

#[tokio::test]
async fn sweep_removes_expired_uploads_and_keeps_fresh() {
    let (_dir, client, bucket) = gc_fixture();
    let ttl = std::time::Duration::from_secs(7 * DAY);
    let now = chrono::Utc::now();
    let eight_days_ago = now - chrono::Duration::days(8);

    let expired = staged_upload(&client, &bucket, "old.bin").await;
    backdate_meta(&staging_dir(&client, &bucket, &expired), eight_days_ago);

    // Staged before `initiated` existed: age comes from the meta.json mtime.
    let legacy = staged_upload(&client, &bucket, "legacy.bin").await;
    let legacy_dir = staging_dir(&client, &bucket, &legacy);
    let meta_path = legacy_dir.join("meta.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    meta.as_object_mut().unwrap().remove("initiated");
    std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&meta_path)
        .unwrap()
        .set_modified(eight_days_ago.into())
        .unwrap();

    // A create that crashed before meta.json: age comes from the dir mtime.
    let orphan_dir = client
        .root()
        .join(bucket.to_string())
        .join(".uploads")
        .join("orphan");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    std::fs::File::open(&orphan_dir)
        .unwrap()
        .set_modified(eight_days_ago.into())
        .unwrap();

    let fresh = staged_upload(&client, &bucket, "fresh.bin").await;
    let almost = staged_upload(&client, &bucket, "almost.bin").await;
    backdate_meta(
        &staging_dir(&client, &bucket, &almost),
        now - chrono::Duration::days(6),
    );

    let report = silo::gc::sweep_expired_uploads(&client, now, ttl).await;
    assert_eq!(
        report,
        silo::gc::SweepReport {
            scanned: 5,
            removed: 3,
            failed: 0,
        }
    );
    assert!(!staging_dir(&client, &bucket, &expired).exists());
    assert!(!legacy_dir.exists());
    assert!(!orphan_dir.exists());
    assert!(staging_dir(&client, &bucket, &fresh).exists());
    assert!(staging_dir(&client, &bucket, &almost).exists());

    let (listed, truncated) = client
        .list_multipart_uploads(&bucket, None, None, None, 1000, None)
        .await
        .unwrap();
    assert!(!truncated);
    let mut ids: Vec<String> = listed
        .into_iter()
        .filter_map(|e| match e {
            silo::multipart::ListUploadEntry::Upload(u) => Some(u.upload_id),
            silo::multipart::ListUploadEntry::CommonPrefix(_) => None,
        })
        .collect();
    ids.sort();
    let mut want = [fresh.clone(), almost.clone()];
    want.sort();
    assert_eq!(ids, want);

    // A swept upload is gone for clients too.
    let err = client
        .list_parts(&bucket, "old.bin", &expired)
        .await
        .unwrap_err();
    assert_eq!(silo::multipart::s3_code_of(&err), Some("NoSuchUpload"));

    // Staging never became an object, before or after the sweep.
    assert!(client.list_objects(&bucket).unwrap().is_empty());

    // The surviving upload still completes.
    let (_, parts) = client
        .list_parts(&bucket, "fresh.bin", &fresh)
        .await
        .unwrap();
    let completed: Vec<_> = parts
        .iter()
        .map(|p| silo::multipart::CompletedPart {
            part_number: p.part_number,
            etag: p.etag.clone(),
        })
        .collect();
    let (obj, _) = client
        .complete_multipart_upload(&bucket, "fresh.bin", &fresh, &completed)
        .await
        .unwrap();
    assert_eq!(obj.status.size_bytes, 4);

    // Advance the injected clock past the TTL: the remaining upload goes.
    let later = now + chrono::Duration::days(2);
    let report = silo::gc::sweep_expired_uploads(&client, later, ttl).await;
    assert_eq!(report.removed, 1);
    assert!(!staging_dir(&client, &bucket, &almost).exists());
    let (listed, _) = client
        .list_multipart_uploads(&bucket, None, None, None, 1000, None)
        .await
        .unwrap();
    assert!(listed.is_empty());
    // The completed object is untouched by the sweep.
    assert_eq!(client.list_objects(&bucket).unwrap().len(), 1);
}

#[tokio::test]
async fn sweep_on_missing_data_dir_is_a_no_op() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("open store");
    let client = Client::new(store, dir.path().join("never-created"));
    let report = silo::gc::sweep_expired_uploads(
        &client,
        chrono::Utc::now(),
        std::time::Duration::from_secs(1),
    )
    .await;
    assert_eq!(report, silo::gc::SweepReport::default());
}

#[tokio::test]
async fn gc_loop_sweeps_on_start_and_stops_on_cancel() {
    let (_dir, client, bucket) = gc_fixture();
    let expired = staged_upload(&client, &bucket, "old.bin").await;
    let expired_dir = staging_dir(&client, &bucket, &expired);
    backdate_meta(
        &expired_dir,
        chrono::Utc::now() - chrono::Duration::days(30),
    );
    let fresh = staged_upload(&client, &bucket, "fresh.bin").await;

    let active = pistack_types::CancellationToken::new();
    let task = tokio::spawn(silo::gc::run(
        client.clone(),
        std::time::Duration::from_secs(7 * DAY),
        active.clone(),
    ));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while expired_dir.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "startup sweep did not run"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(staging_dir(&client, &bucket, &fresh).exists());

    active.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("gc loop exits on cancel")
        .expect("gc loop task");
}

fn extract_copy_part_etag(xml: &str) -> String {
    xml.split("<ETag>")
        .nth(1)
        .expect("ETag")
        .split("</ETag>")
        .next()
        .unwrap()
        .replace("&quot;", "")
        .trim_matches('"')
        .to_owned()
}

#[tokio::test]
async fn upload_part_copy_full_and_ranged_and_mixed_complete() {
    let (_dir, _client, app, _project, who) = app_multipart(64, 4);
    let src_bucket = ensure_bucket(&app, &who, "src").await;
    let dst_bucket = ensure_bucket(&app, &who, "dst").await;
    let source = b"ABCDEFGHIJKLMNOP"; // 16 bytes
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/{src_bucket}/source.bin"),
        Some(source.to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{dst_bucket}/out.bin?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));

    // Full copy as part 1.
    let (status, body, headers) = call_with_headers(
        &app,
        "PUT",
        &format!("/{dst_bucket}/out.bin?partNumber=1&uploadId={upload_id}"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{src_bucket}/source.bin"))],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let xml = String::from_utf8_lossy(&body);
    assert!(xml.contains("CopyPartResult"), "{xml}");
    assert!(xml.contains("LastModified"), "{xml}");
    let etag_full = extract_copy_part_etag(&xml);
    assert_eq!(etag_full, md5_hex(source));
    assert_eq!(
        headers
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .unwrap()
            .trim_matches('"'),
        etag_full
    );

    // Abort and start fresh for ranged + mixed.
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/{dst_bucket}/out.bin?uploadId={upload_id}"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{dst_bucket}/mixed.bin?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));

    // Ranged copy bytes=0-7 as part 1.
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{dst_bucket}/mixed.bin?partNumber=1&uploadId={upload_id}"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[
            ("x-amz-copy-source", &format!("/{src_bucket}/source.bin")),
            ("x-amz-copy-source-range", "bytes=0-7"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let etag1 = extract_copy_part_etag(&String::from_utf8_lossy(&body));
    assert_eq!(etag1, md5_hex(&source[0..8]));

    // Uploaded part 2.
    let part2 = b"wxyzWXYZ"; // 8
    let (status, _, headers) = call(
        &app,
        "PUT",
        &format!("/{dst_bucket}/mixed.bin?partNumber=2&uploadId={upload_id}"),
        Some(part2.to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag2 = headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .trim_matches('"')
        .to_owned();

    // Ranged copy bytes=8-15 as part 3.
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{dst_bucket}/mixed.bin?partNumber=3&uploadId={upload_id}"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[
            ("x-amz-copy-source", &format!("/{src_bucket}/source.bin")),
            ("x-amz-copy-source-range", "bytes=8-15"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let etag3 = extract_copy_part_etag(&String::from_utf8_lossy(&body));
    assert_eq!(etag3, md5_hex(&source[8..16]));

    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{dst_bucket}/mixed.bin?uploadId={upload_id}"),
        Some(complete_xml(&[(1, &etag1), (2, &etag2), (3, &etag3)])),
        "application/xml",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/{dst_bucket}/mixed.bin"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut expected = source[0..8].to_vec();
    expected.extend_from_slice(part2);
    expected.extend_from_slice(&source[8..16]);
    assert_eq!(body, expected);
}

#[tokio::test]
async fn upload_part_copy_errors() {
    let (_dir, _client, app, _project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "upc-err").await;
    let source = b"0123456789abcdef";
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/{bucket}/src.bin"),
        Some(source.to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{bucket}/dst.bin?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));

    // Bad range.
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/dst.bin?partNumber=1&uploadId={upload_id}"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[
            ("x-amz-copy-source", &format!("/{bucket}/src.bin")),
            ("x-amz-copy-source-range", "bytes=0-99"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "InvalidRange"
    );

    // Missing source key.
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/dst.bin?partNumber=1&uploadId={upload_id}"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{bucket}/nope.bin"))],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "NoSuchKey"
    );

    // Missing upload.
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/dst.bin?partNumber=1&uploadId=does-not-exist"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{bucket}/src.bin"))],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "NoSuchUpload"
    );

    // Body + copy-source rejected.
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/dst.bin?partNumber=1&uploadId={upload_id}"),
        Some(b"nope".to_vec()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{bucket}/src.bin"))],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "InvalidRequest"
    );

    // CopyObject (no multipart query) succeeds.
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/copied.bin"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{bucket}/src.bin"))],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert!(
        String::from_utf8_lossy(&body).contains("CopyObjectResult"),
        "{}",
        String::from_utf8_lossy(&body)
    );

    // versionId → NotImplemented.
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/dst.bin?partNumber=1&uploadId={upload_id}"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[(
            "x-amz-copy-source",
            &format!("/{bucket}/src.bin?versionId=v1"),
        )],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "NotImplemented"
    );
}

#[tokio::test]
async fn upload_part_copy_cross_project_access_denied() {
    // Cross-project copy-source must be AccessDenied (not not-found).
    let (_dir, client, app, _project, who) = app_multipart(64, 4);
    let other = Id::new(kind::PROJECT);
    let stranger = project_caller(&other);
    let src_bucket = ensure_bucket(&app, &stranger, "secret-src").await;
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/{src_bucket}/secret.bin"),
        Some(b"secret-bytes-here".to_vec()),
        "application/octet-stream",
        Some(&stranger),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let dst_bucket = ensure_bucket(&app, &who, "dst-vis").await;
    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{dst_bucket}/out.bin?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));

    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{dst_bucket}/out.bin?partNumber=1&uploadId={upload_id}"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{src_bucket}/secret.bin"))],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "AccessDenied"
    );

    // No part was written; staging has no part files or leftover temps.
    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/{dst_bucket}/out.bin?uploadId={upload_id}"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert!(
        !String::from_utf8_lossy(&body).contains("<Part>"),
        "ListParts must be empty: {}",
        String::from_utf8_lossy(&body)
    );
    let dst = client
        .list_buckets(Some(&who.project.clone().unwrap()))
        .unwrap()
        .into_iter()
        .find(|b| b.meta.name == dst_bucket)
        .unwrap();
    let staging = staging_dir(&client, &dst.meta.id, &upload_id);
    assert!(staging.exists(), "upload staging should still exist");
    let leftovers: Vec<_> = std::fs::read_dir(&staging)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".tmp-") || n.starts_with("part-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no part or .tmp-* files after AccessDenied: {leftovers:?}"
    );
}

#[tokio::test]
async fn upload_part_copy_nonexistent_source_bucket_access_denied() {
    let (_dir, client, app, _project, who) = app_multipart(64, 4);
    let dst_bucket = ensure_bucket(&app, &who, "dst-missing-src").await;
    let (status, body, _) = call(
        &app,
        "POST",
        &format!("/{dst_bucket}/out.bin?uploads"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upload_id = extract_upload_id(&String::from_utf8_lossy(&body));

    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{dst_bucket}/out.bin?partNumber=1&uploadId={upload_id}"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", "/no-such-bucket-xyz/any-key")],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "AccessDenied"
    );

    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/{dst_bucket}/out.bin?uploadId={upload_id}"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert!(!String::from_utf8_lossy(&body).contains("<Part>"));
    let dst = client
        .list_buckets(Some(&who.project.clone().unwrap()))
        .unwrap()
        .into_iter()
        .find(|b| b.meta.name == dst_bucket)
        .unwrap();
    let staging = staging_dir(&client, &dst.meta.id, &upload_id);
    let leftovers: Vec<_> = std::fs::read_dir(&staging)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".tmp-") || n.starts_with("part-"))
        .collect();
    assert!(leftovers.is_empty(), "leftovers: {leftovers:?}");
}

#[tokio::test]
async fn copy_object_basic_and_cross_bucket_and_metadata() {
    let (_dir, _client, app, _project, who) = app_with_max(1024);
    let src_bucket = ensure_bucket(&app, &who, "copy-src").await;
    let dst_bucket = ensure_bucket(&app, &who, "copy-dst").await;
    let payload = b"hello-copy-object";
    let (status, _, headers) = call(
        &app,
        "PUT",
        &format!("/{src_bucket}/orig.txt"),
        Some(payload.to_vec()),
        "text/plain",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let src_etag = headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .trim_matches('"')
        .to_owned();

    let (status, body, headers) = call_with_headers(
        &app,
        "PUT",
        &format!("/{src_bucket}/copy.txt"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{src_bucket}/orig.txt"))],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert!(String::from_utf8_lossy(&body).contains("CopyObjectResult"));
    assert!(String::from_utf8_lossy(&body).contains("LastModified"));
    let copy_etag = headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .trim_matches('"')
        .to_owned();
    assert_eq!(copy_etag, src_etag);
    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/{src_bucket}/copy.txt"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, payload);

    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{dst_bucket}/from-src.txt"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[
            ("x-amz-copy-source", &format!("/{src_bucket}/orig.txt")),
            ("x-amz-metadata-directive", "COPY"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let (status, _, headers) = call(
        &app,
        "HEAD",
        &format!("/{dst_bucket}/from-src.txt"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap(),
        "text/plain"
    );

    let (status, _, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{dst_bucket}/replaced.txt"),
        Some(Vec::new()),
        "application/json",
        Some(&who),
        &[
            ("x-amz-copy-source", &format!("/{src_bucket}/orig.txt")),
            ("x-amz-metadata-directive", "REPLACE"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, headers) = call(
        &app,
        "HEAD",
        &format!("/{dst_bucket}/replaced.txt"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn copy_object_self_copy_copy_vs_replace() {
    let (_dir, _client, app, _project, who) = app_with_max(1024);
    let bucket = ensure_bucket(&app, &who, "self-copy").await;
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/{bucket}/me.txt"),
        Some(b"self".to_vec()),
        "text/plain",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/me.txt"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{bucket}/me.txt"))],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "InvalidRequest"
    );

    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/me.txt"),
        Some(Vec::new()),
        "application/json",
        Some(&who),
        &[
            ("x-amz-copy-source", &format!("/{bucket}/me.txt")),
            ("x-amz-metadata-directive", "REPLACE"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let (status, body, _) = call(
        &app,
        "GET",
        &format!("/{bucket}/me.txt"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"self");
    let (status, _, headers) = call(
        &app,
        "HEAD",
        &format!("/{bucket}/me.txt"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn copy_object_access_denied_and_errors() {
    let (_dir, client, app, _project, who) = app_with_max(64);
    let bucket = ensure_bucket(&app, &who, "co-err").await;
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/{bucket}/small.bin"),
        Some(b"0123456789abcdef".to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/out.bin"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{bucket}/missing.bin"))],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "NoSuchKey"
    );

    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/ranged.bin"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[
            ("x-amz-copy-source", &format!("/{bucket}/small.bin")),
            ("x-amz-copy-source-range", "bytes=0-3"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "InvalidArgument"
    );

    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/from-ghost.bin"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", "/no-such-bkt/key")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "AccessDenied"
    );
    let (status, _, _) = call(
        &app,
        "GET",
        &format!("/{bucket}/from-ghost.bin"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let other = Id::new(kind::PROJECT);
    let stranger = project_caller(&other);
    let secret = ensure_bucket(&app, &stranger, "secret-co").await;
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/{secret}/hidden.bin"),
        Some(b"secret".to_vec()),
        "application/octet-stream",
        Some(&stranger),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body, _) = call_with_headers(
        &app,
        "PUT",
        &format!("/{bucket}/stolen.bin"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{secret}/hidden.bin"))],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "AccessDenied"
    );
    let (status, _, _) = call(
        &app,
        "GET",
        &format!("/{bucket}/stolen.bin"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let bkt = client
        .list_buckets(Some(&who.project.clone().unwrap()))
        .unwrap()
        .into_iter()
        .find(|b| b.meta.name == bucket)
        .unwrap();
    let bucket_dir = client.root().join(bkt.meta.id.to_string());
    let tmps = walkdir_tmp(&bucket_dir);
    assert!(tmps.is_empty(), "leftover temps: {tmps:?}");
}

#[tokio::test]
async fn copy_object_oversize_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("open store");
    let root = dir.path().join("silo");
    let big = Client::new(store.clone(), &root).with_max_object_bytes(64);
    let small = Client::new(store, &root).with_max_object_bytes(8);
    let app_big = http::router(big);
    let app_small = http::router(small);
    let project = Id::new(kind::PROJECT);
    let who = project_caller(&project);
    let bucket = ensure_bucket(&app_big, &who, "oversize").await;
    let payload = b"0123456789abcdef"; // 16 > 8
    let (status, _, _) = call(
        &app_big,
        "PUT",
        &format!("/{bucket}/big.bin"),
        Some(payload.to_vec()),
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body, _) = call_with_headers(
        &app_small,
        "PUT",
        &format!("/{bucket}/out.bin"),
        Some(Vec::new()),
        "application/octet-stream",
        Some(&who),
        &[("x-amz-copy-source", &format!("/{bucket}/big.bin"))],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "EntityTooLarge"
    );
    let (status, _, _) = call(
        &app_small,
        "GET",
        &format!("/{bucket}/out.bin"),
        None,
        "application/octet-stream",
        Some(&who),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn walkdir_tmp(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.is_dir() {
            out.extend(walkdir_tmp(&path));
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(".tmp-"))
        {
            out.push(path);
        }
    }
    out
}

fn listed_common_prefixes(xml: &str) -> Vec<String> {
    xml.split("<CommonPrefixes>")
        .skip(1)
        .filter_map(|block| {
            let block = block.split("</CommonPrefixes>").next()?;
            xml_tag(block, "Prefix")
        })
        .collect()
}

#[tokio::test]
async fn list_multipart_uploads_delimiter_grouping() {
    let (_dir, _client, app, _project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "delim").await;

    let a1 = create_upload(&app, &who, &bucket, "logs/a.bin").await;
    let a2 = create_upload(&app, &who, &bucket, "logs/a.bin").await;
    let _nested1 = create_upload(&app, &who, &bucket, "logs/2024/x.bin").await;
    let _nested2 = create_upload(&app, &who, &bucket, "logs/2024/y.bin").await;
    let flat = create_upload(&app, &who, &bucket, "readme.txt").await;
    let _other = create_upload(&app, &who, &bucket, "other/z.bin").await;
    let mut a_ids = [a1, a2];
    a_ids.sort();

    // delimiter=/ with no prefix: common prefixes + flat upload.
    let xml = list_uploads(&app, &who, &format!("/{bucket}?uploads&delimiter=/")).await;
    assert_eq!(xml_tag(&xml, "Delimiter").as_deref(), Some("/"));
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));
    assert_eq!(
        listed_common_prefixes(&xml),
        vec!["logs/".to_owned(), "other/".to_owned()]
    );
    assert_eq!(
        listed_uploads(&xml),
        vec![("readme.txt".to_owned(), flat.clone())]
    );

    // prefix=logs/ + delimiter=/: uploads under logs/ without further '/', plus nested CP.
    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&prefix=logs/&delimiter=/"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "Prefix").as_deref(), Some("logs/"));
    assert_eq!(listed_common_prefixes(&xml), vec!["logs/2024/".to_owned()]);
    assert_eq!(
        listed_uploads(&xml),
        vec![
            ("logs/a.bin".to_owned(), a_ids[0].clone()),
            ("logs/a.bin".to_owned(), a_ids[1].clone()),
        ]
    );

    // Empty delimiter = no grouping (same as omitted).
    let xml = list_uploads(&app, &who, &format!("/{bucket}?uploads&delimiter=")).await;
    assert!(xml_tag(&xml, "Delimiter").is_none());
    assert!(listed_common_prefixes(&xml).is_empty());
    assert!(listed_uploads(&xml).len() >= 6);

    // Non-'/' delimiter (keys must not look like Windows drive paths to validate_key).
    let bucket2 = ensure_bucket(&app, &who, "delim2").await;
    create_upload(&app, &who, &bucket2, "foo::bar::c").await;
    create_upload(&app, &who, &bucket2, "foo::bar::d").await;
    create_upload(&app, &who, &bucket2, "plain").await;
    let xml = list_uploads(&app, &who, &format!("/{bucket2}?uploads&delimiter=::")).await;
    assert_eq!(xml_tag(&xml, "Delimiter").as_deref(), Some("::"));
    assert_eq!(listed_common_prefixes(&xml), vec!["foo::".to_owned()]);
    assert_eq!(listed_uploads(&xml).len(), 1);
    assert_eq!(listed_uploads(&xml)[0].0, "plain");
}

#[tokio::test]
async fn list_multipart_uploads_delimiter_pagination() {
    let (_dir, _client, app, _project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "delim-page").await;
    create_upload(&app, &who, &bucket, "a/1").await;
    create_upload(&app, &who, &bucket, "a/2").await;
    let b = create_upload(&app, &who, &bucket, "b.bin").await;
    create_upload(&app, &who, &bucket, "c/x").await;

    // max-uploads=1: first entry is CommonPrefix a/
    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&delimiter=/&max-uploads=1"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("true"));
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert!(listed_uploads(&xml).is_empty());
    assert_eq!(xml_tag(&xml, "NextKeyMarker").as_deref(), Some("a/"));
    assert_eq!(xml_tag(&xml, "NextUploadIdMarker").as_deref(), Some(""));

    // Continue after common prefix.
    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&delimiter=/&max-uploads=1&key-marker=a/"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("true"));
    assert_eq!(listed_uploads(&xml), vec![("b.bin".to_owned(), b.clone())]);
    assert!(listed_common_prefixes(&xml).is_empty());

    // max-uploads=2 from start: a/ then b.bin
    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&delimiter=/&max-uploads=2"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("true"));
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert_eq!(listed_uploads(&xml).len(), 1);
    assert_eq!(listed_uploads(&xml)[0].0, "b.bin");
    assert_eq!(xml_tag(&xml, "NextKeyMarker").as_deref(), Some("b.bin"));
}

// ---- ListObjectsV2 delimiter ---------------------------------------------------

fn listed_contents_keys(xml: &str) -> Vec<String> {
    xml.split("<Contents>")
        .skip(1)
        .filter_map(|block| {
            let block = block.split("</Contents>").next()?;
            xml_tag(block, "Key")
        })
        .collect()
}

async fn list_objects_v2(app: &Router, who: &Caller, uri: &str) -> String {
    let (status, body, headers) = call(app, "GET", uri, None, "", Some(who)).await;
    let xml = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(status, StatusCode::OK, "{xml}");
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/xml"
    );
    assert!(xml.contains("<ListBucketResult"), "{xml}");
    xml
}

async fn put_key(app: &Router, who: &Caller, bucket: &str, key: &str, body: &[u8]) {
    let (status, resp, _) = call(
        app,
        "PUT",
        &format!("/{bucket}/{key}"),
        Some(body.to_vec()),
        "application/octet-stream",
        Some(who),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{key}: {}",
        String::from_utf8_lossy(&resp)
    );
}

#[tokio::test]
async fn list_objects_v2_delimiter_grouping() {
    let (_dir, _client, app, _project, who) = app_with_max(64 * 1024);
    let bucket = ensure_bucket(&app, &who, "ls-delim").await;

    put_key(&app, &who, &bucket, "logs/a.bin", b"a").await;
    put_key(&app, &who, &bucket, "logs/2024/x.bin", b"x").await;
    put_key(&app, &who, &bucket, "logs/2024/y.bin", b"y").await;
    put_key(&app, &who, &bucket, "readme.txt", b"r").await;
    put_key(&app, &who, &bucket, "other/z.bin", b"z").await;

    let xml = list_objects_v2(&app, &who, &format!("/{bucket}?list-type=2&delimiter=/")).await;
    assert_eq!(xml_tag(&xml, "Delimiter").as_deref(), Some("/"));
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));
    assert_eq!(xml_tag(&xml, "KeyCount").as_deref(), Some("3"));
    assert_eq!(
        listed_common_prefixes(&xml),
        vec!["logs/".to_owned(), "other/".to_owned()]
    );
    assert_eq!(listed_contents_keys(&xml), vec!["readme.txt".to_owned()]);

    let xml = list_objects_v2(
        &app,
        &who,
        &format!("/{bucket}?list-type=2&prefix=logs/&delimiter=/"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "Prefix").as_deref(), Some("logs/"));
    assert_eq!(listed_common_prefixes(&xml), vec!["logs/2024/".to_owned()]);
    assert_eq!(listed_contents_keys(&xml), vec!["logs/a.bin".to_owned()]);

    // Empty delimiter = no grouping.
    let xml = list_objects_v2(&app, &who, &format!("/{bucket}?list-type=2&delimiter=")).await;
    assert!(xml_tag(&xml, "Delimiter").is_none());
    assert!(listed_common_prefixes(&xml).is_empty());
    assert_eq!(listed_contents_keys(&xml).len(), 5);

    // Non-'/' delimiter.
    let bucket2 = ensure_bucket(&app, &who, "ls-delim2").await;
    put_key(&app, &who, &bucket2, "foo::bar::c", b"1").await;
    put_key(&app, &who, &bucket2, "foo::bar::d", b"2").await;
    put_key(&app, &who, &bucket2, "plain", b"3").await;
    let xml = list_objects_v2(&app, &who, &format!("/{bucket2}?list-type=2&delimiter=::")).await;
    assert_eq!(xml_tag(&xml, "Delimiter").as_deref(), Some("::"));
    assert_eq!(listed_common_prefixes(&xml), vec!["foo::".to_owned()]);
    assert_eq!(listed_contents_keys(&xml), vec!["plain".to_owned()]);
}

#[tokio::test]
async fn list_objects_v2_delimiter_pagination() {
    let (_dir, _client, app, _project, who) = app_with_max(64 * 1024);
    let bucket = ensure_bucket(&app, &who, "ls-page").await;
    put_key(&app, &who, &bucket, "a/1", b"1").await;
    put_key(&app, &who, &bucket, "a/2", b"2").await;
    put_key(&app, &who, &bucket, "b.bin", b"b").await;
    put_key(&app, &who, &bucket, "c/x", b"c").await;

    let xml = list_objects_v2(
        &app,
        &who,
        &format!("/{bucket}?list-type=2&delimiter=/&max-keys=1"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("true"));
    assert_eq!(xml_tag(&xml, "KeyCount").as_deref(), Some("1"));
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert!(listed_contents_keys(&xml).is_empty());
    assert_eq!(
        xml_tag(&xml, "NextContinuationToken").as_deref(),
        Some("a/")
    );

    let xml = list_objects_v2(
        &app,
        &who,
        &format!("/{bucket}?list-type=2&delimiter=/&max-keys=1&continuation-token=a/"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("true"));
    assert_eq!(listed_contents_keys(&xml), vec!["b.bin".to_owned()]);
    assert!(listed_common_prefixes(&xml).is_empty());

    // start-after across the same boundary.
    let xml = list_objects_v2(
        &app,
        &who,
        &format!("/{bucket}?list-type=2&delimiter=/&max-keys=2&start-after=a/"),
    )
    .await;
    assert_eq!(listed_contents_keys(&xml), vec!["b.bin".to_owned()]);
    assert_eq!(listed_common_prefixes(&xml), vec!["c/".to_owned()]);
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));

    let xml = list_objects_v2(
        &app,
        &who,
        &format!("/{bucket}?list-type=2&delimiter=/&max-keys=2"),
    )
    .await;
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("true"));
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert_eq!(listed_contents_keys(&xml), vec!["b.bin".to_owned()]);
    assert_eq!(
        xml_tag(&xml, "NextContinuationToken").as_deref(),
        Some("b.bin")
    );
}

/// Identity edge case from #72: key equal to the common prefix (`a/`) is
/// grouped into that CommonPrefix; paging with max=1/2 never duplicates or
/// skips it.
#[tokio::test]
async fn list_objects_v2_key_equal_common_prefix_edge() {
    let (_dir, client, app, project, who) = app_with_max(64 * 1024);
    let bucket = ensure_bucket(&app, &who, "ls-edge").await;
    // Trailing-slash key via Client (HTTP path normalization is finicky).
    let bucket_id = client
        .list_buckets(Some(&project))
        .unwrap()
        .into_iter()
        .find(|b| b.meta.name == bucket)
        .unwrap()
        .meta
        .id;
    client
        .put_object(&bucket_id, "a/", b"dir", None)
        .await
        .expect("put a/");
    put_key(&app, &who, &bucket, "a/1", b"1").await;
    put_key(&app, &who, &bucket, "b", b"b").await;

    let xml = list_objects_v2(&app, &who, &format!("/{bucket}?list-type=2&delimiter=/")).await;
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert_eq!(listed_contents_keys(&xml), vec!["b".to_owned()]);
    // a/ must not appear as Contents.
    assert!(!listed_contents_keys(&xml).iter().any(|k| k == "a/"));

    let xml = list_objects_v2(
        &app,
        &who,
        &format!("/{bucket}?list-type=2&delimiter=/&max-keys=1"),
    )
    .await;
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert_eq!(
        xml_tag(&xml, "NextContinuationToken").as_deref(),
        Some("a/")
    );

    let xml = list_objects_v2(
        &app,
        &who,
        &format!("/{bucket}?list-type=2&delimiter=/&max-keys=1&continuation-token=a/"),
    )
    .await;
    assert_eq!(listed_contents_keys(&xml), vec!["b".to_owned()]);
    assert!(listed_common_prefixes(&xml).is_empty());
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));

    let xml = list_objects_v2(
        &app,
        &who,
        &format!("/{bucket}?list-type=2&delimiter=/&max-keys=2"),
    )
    .await;
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert_eq!(listed_contents_keys(&xml), vec!["b".to_owned()]);
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));
    // Only one CommonPrefix a/ in the full page.
    assert_eq!(listed_common_prefixes(&xml).len(), 1);
}

#[tokio::test]
async fn list_multipart_uploads_key_equal_common_prefix_edge() {
    let (_dir, client, app, project, who) = app_multipart(64, 4);
    let bucket = ensure_bucket(&app, &who, "mpu-edge").await;
    let bucket_id = client
        .list_buckets(Some(&project))
        .unwrap()
        .into_iter()
        .find(|b| b.meta.name == bucket)
        .unwrap()
        .meta
        .id;
    // Trailing-slash key via Client (HTTP path normalization is finicky).
    client
        .create_multipart_upload(&bucket_id, "a/", None)
        .await
        .expect("upload a/");
    create_upload(&app, &who, &bucket, "a/1").await;
    let b = create_upload(&app, &who, &bucket, "b").await;

    let xml = list_uploads(&app, &who, &format!("/{bucket}?uploads&delimiter=/")).await;
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert_eq!(listed_uploads(&xml), vec![("b".to_owned(), b.clone())]);

    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&delimiter=/&max-uploads=1"),
    )
    .await;
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert_eq!(xml_tag(&xml, "NextKeyMarker").as_deref(), Some("a/"));
    assert!(listed_uploads(&xml).is_empty());

    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&delimiter=/&max-uploads=1&key-marker=a/"),
    )
    .await;
    assert_eq!(listed_uploads(&xml), vec![("b".to_owned(), b.clone())]);
    assert!(listed_common_prefixes(&xml).is_empty());
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));

    let xml = list_uploads(
        &app,
        &who,
        &format!("/{bucket}?uploads&delimiter=/&max-uploads=2"),
    )
    .await;
    assert_eq!(listed_common_prefixes(&xml), vec!["a/".to_owned()]);
    assert_eq!(listed_uploads(&xml).len(), 1);
    assert_eq!(listed_uploads(&xml)[0].0, "b");
    assert_eq!(xml_tag(&xml, "IsTruncated").as_deref(), Some("false"));
}
