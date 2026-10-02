// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Integration: upload through the router, check hashing, download,
//! content-addressed storage and deletion.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use mold::{Client, LocalDirStore};
use pistack_types::{Caller, Id};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const MIB: usize = 1024 * 1024;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (st, bytes) = raw(
        app,
        method,
        uri,
        body.map(|v| Body::from(v.to_string())),
        None,
    )
    .await;
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json")
    };
    (st, v)
}

async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Body>,
    caller: Option<Caller>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(c) = caller {
        req = req.extension(c);
    }
    let resp = app
        .clone()
        .oneshot(req.body(body.unwrap_or_else(Body::empty)).expect("request"))
        .await
        .expect("response");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 16 * MIB).await.expect("body");
    (status, bytes.to_vec())
}

fn member(project: &Id) -> Caller {
    Caller {
        principal: Id::new("prin"),
        principal_name: "alice".into(),
        project: Some(project.clone()),
        roles: vec!["member".into()],
        system: false,
    }
}

#[tokio::test]
async fn upload_download_delete_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = Client::local(dir.path()).expect("client");
    let app = mold::http::router(c.clone());
    let blobs = LocalDirStore::new(dir.path());

    // metadata
    let (st, img) = call(
        &app,
        "POST",
        "/images",
        Some(json!({ "name": "random-raw", "spec": { "format": "raw", "os": "none", "min_disk_gib": 1 } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{img}");
    let id = img["id"].as_str().expect("id").to_owned();
    assert!(id.starts_with("img-"));
    assert_eq!(img["status"]["phase"], "pending");
    assert_eq!(img["spec"]["arch"], "x86_64");

    // no data yet -> 409
    let (st, _) = raw(&app, "GET", &format!("/images/{id}/data"), None, None).await;
    assert_eq!(st, StatusCode::CONFLICT);

    // 1 MiB of random bytes
    let mut payload = vec![0u8; MIB];
    rand::thread_rng().fill_bytes(&mut payload);
    let want_sha = hex(&Sha256::digest(&payload));

    let (st, body) = raw(
        &app,
        "PUT",
        &format!("/images/{id}/data"),
        Some(Body::from(payload.clone())),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let img: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(img["status"]["phase"], "ready");
    assert_eq!(img["status"]["size_bytes"], MIB as u64);
    assert_eq!(img["status"]["sha256"], want_sha);
    assert_eq!(img["status"]["backing_ref"], format!("sha256:{want_sha}"));

    // content-addressed file exists with the right size
    let path = blobs
        .blob_path(&format!("sha256:{want_sha}"))
        .expect("path");
    assert!(path.ends_with(format!("blobs/sha256/{}/{want_sha}", &want_sha[..2])));
    assert_eq!(
        std::fs::metadata(&path).expect("blob exists").len(),
        MIB as u64
    );
    assert!(
        std::fs::read_dir(dir.path().join("tmp"))
            .expect("tmp")
            .next()
            .is_none(),
        "no temp files left"
    );

    // second upload -> 409
    let (st, _) = raw(
        &app,
        "PUT",
        &format!("/images/{id}/data"),
        Some(Body::from(vec![1u8; 10])),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // download equals upload
    let (st, got) = raw(&app, "GET", &format!("/images/{id}/data"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got, payload);

    // public API: resolve by name + open
    let resolved = c.resolve(None, "random-raw").expect("resolve");
    assert_eq!(resolved.meta.id.to_string(), id);
    let mut stream = c.open(&resolved.meta.id).await.expect("open");
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut buf)
        .await
        .expect("read");
    assert_eq!(buf, payload);

    // import is validated then 501
    let (st, _) = call(
        &app,
        "POST",
        &format!("/images/{id}/import"),
        Some(json!({ "url": "ftp://x/y" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &app,
        "POST",
        &format!("/images/{id}/import"),
        Some(json!({ "url": "https://cloud-images.example/x.img" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "already has data");

    // delete removes record and blob
    let (st, _) = raw(&app, "DELETE", &format!("/images/{id}"), None, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = raw(&app, "GET", &format!("/images/{id}"), None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(!path.exists(), "blob removed with its last image");
}

#[tokio::test]
async fn qcow2_with_backing_file_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = Client::local(dir.path()).expect("client");
    let app = mold::http::router(c.clone());

    let (st, img) = call(
        &app,
        "POST",
        "/images",
        Some(json!({ "name": "evil", "spec": { "format": "qcow2" } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{img}");
    let id = img["id"].as_str().expect("id").to_owned();

    // qcow2 v3 header with a backing file at offset 104
    let mut h = vec![0u8; 65536];
    h[..4].copy_from_slice(b"QFI\xfb");
    h[4..8].copy_from_slice(&3u32.to_be_bytes());
    h[8..16].copy_from_slice(&104u64.to_be_bytes());
    h[16..20].copy_from_slice(&11u32.to_be_bytes());
    h[20..24].copy_from_slice(&16u32.to_be_bytes());
    h[24..32].copy_from_slice(&(1u64 << 30).to_be_bytes());
    h[104..115].copy_from_slice(b"/etc/shadow");

    let (st, body) = raw(
        &app,
        "PUT",
        &format!("/images/{id}/data"),
        Some(Body::from(h)),
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let (_, img) = call(&app, "GET", &format!("/images/{id}"), None).await;
    assert_eq!(img["status"]["phase"], "error");
    // nothing left in the blob store
    let blobs = dir.path().join("blobs");
    let count = walk(&blobs);
    assert_eq!(count, 0, "rejected image must not be kept");

    // a clean qcow2 is accepted and reports its virtual size. With qemu-img
    // on the box make a real one; without it a bare header is enough.
    let h = if mold::inspect::qemu_img_available() {
        let p = dir.path().join("clean.qcow2");
        let ok = std::process::Command::new("qemu-img")
            .args(["create", "-f", "qcow2"])
            .arg(&p)
            .arg("4G")
            .status()
            .expect("qemu-img create")
            .success();
        assert!(ok);
        std::fs::read(&p).expect("read")
    } else {
        let mut h = vec![0u8; 65536];
        h[..4].copy_from_slice(b"QFI\xfb");
        h[4..8].copy_from_slice(&3u32.to_be_bytes());
        h[20..24].copy_from_slice(&16u32.to_be_bytes());
        h[24..32].copy_from_slice(&(4u64 << 30).to_be_bytes());
        h
    };
    let (st, body) = raw(
        &app,
        "PUT",
        &format!("/images/{id}/data"),
        Some(Body::from(h)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let img: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(img["status"]["phase"], "ready");
    assert_eq!(img["status"]["virtual_size_bytes"], 4u64 << 30);

    // raw declared but qcow2 bytes -> 422
    let (_, img2) = call(
        &app,
        "POST",
        "/images",
        Some(json!({ "name": "mislabeled", "spec": { "format": "raw" } })),
    )
    .await;
    let id2 = img2["id"].as_str().expect("id").to_owned();
    let mut h = vec![0u8; 4096];
    h[..4].copy_from_slice(b"QFI\xfb");
    let (st, _) = raw(
        &app,
        "PUT",
        &format!("/images/{id2}/data"),
        Some(Body::from(h)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
}

fn walk(dir: &std::path::Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| {
            if e.path().is_dir() {
                walk(&e.path())
            } else {
                1
            }
        })
        .sum()
}

#[tokio::test]
async fn visibility_members_and_stale_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = Client::local(dir.path()).expect("client");
    let app = mold::http::router(c.clone());
    let owner = Id::new("proj");
    let other = Id::new("proj");

    // admin creates: a public image, and a project-visibility image owned by `owner`
    let (st, pubimg) = call(
        &app,
        "POST",
        "/images",
        Some(json!({ "name": "ubuntu-24.04", "spec": { "format": "qcow2", "visibility": "public" } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{pubimg}");
    let (st, shared) = call(
        &app,
        "POST",
        "/images",
        Some(json!({ "name": "team-base", "project": owner, "spec": { "format": "raw", "visibility": "project" } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{shared}");
    let shared_id = shared["id"].as_str().expect("id").to_owned();
    let (st, private) = call(
        &app,
        "POST",
        "/images",
        Some(json!({ "name": "secret", "project": owner, "spec": { "format": "raw" } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{private}");

    // non-member of `other` sees only the public image
    let (st, body) = raw(&app, "GET", "/images", None, Some(member(&other))).await;
    assert_eq!(st, StatusCode::OK);
    let page: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["name"], "ubuntu-24.04");
    let (st, _) = raw(
        &app,
        "GET",
        &format!("/images/{shared_id}"),
        None,
        Some(member(&other)),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // owner sees all three; a non-admin may not publish
    let (_, body) = raw(&app, "GET", "/images", None, Some(member(&owner))).await;
    let page: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(page["total"], 3);
    let (st, _) = raw(
        &app,
        "POST",
        "/images",
        Some(Body::from(
            json!({ "name": "leak", "spec": { "format": "raw", "visibility": "public" } })
                .to_string(),
        )),
        Some(member(&owner)),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // share with `other`
    let (st, m) = call(
        &app,
        "POST",
        &format!("/images/{shared_id}/members"),
        Some(json!({ "project": other })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{m}");
    let (st, _) = raw(
        &app,
        "GET",
        &format!("/images/{shared_id}"),
        None,
        Some(member(&other)),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, body) = raw(&app, "GET", "/images", None, Some(member(&other))).await;
    let page: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(page["total"], 2);
    // but `other` cannot edit it
    let (st, _) = raw(
        &app,
        "PUT",
        &format!("/images/{shared_id}"),
        Some(Body::from(json!({ "description": "mine now" }).to_string())),
        Some(member(&other)),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // members cannot be added to private images
    let pid = private["id"].as_str().expect("id");
    let (st, _) = call(
        &app,
        "POST",
        &format!("/images/{pid}/members"),
        Some(json!({ "project": other })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // unshare
    let (st, _) = raw(
        &app,
        "DELETE",
        &format!("/images/{shared_id}/members/{other}"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = raw(
        &app,
        "GET",
        &format!("/images/{shared_id}"),
        None,
        Some(member(&other)),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // update with If-Match, then stale -> 409
    let put = |rev: &str| {
        Request::builder()
            .method("PUT")
            .uri(format!("/images/{shared_id}"))
            .header("content-type", "application/json")
            .header("if-match", rev)
            .body(Body::from(json!({ "min_ram_mib": 512 }).to_string()))
            .expect("req")
    };
    let resp = app.clone().oneshot(put("1")).await.expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app.clone().oneshot(put("1")).await.expect("resp");
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // resolve: name in project beats public; public found from another project
    let resolved = c.resolve(Some(&owner), "team-base").expect("resolve");
    assert_eq!(resolved.meta.id.to_string(), shared_id);
    let resolved = c
        .resolve(Some(&other), "ubuntu-24.04")
        .expect("resolve public");
    assert_eq!(resolved.spec.visibility, mold::Visibility::Public);
    assert!(c.resolve(Some(&other), "secret").is_err());
}
