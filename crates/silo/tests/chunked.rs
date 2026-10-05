// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end aws-chunked PutObject / UploadPart via keep SigV4.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use chrono::Utc;
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Meta};
use silo::chunked::{
    chunk_signature, frame_signed_chunks, frame_signed_trailer, frame_unsigned_trailer,
    TrailerChecksum, STREAMING_PAYLOAD, STREAMING_SIGNED_TRAILER, STREAMING_UNSIGNED_TRAILER,
};
use silo::http;
use silo::sigv4::{self, derive_signing_key, EMPTY_PAYLOAD_SHA256};
use silo::{Client, DEFAULT_REGION};
use tower::ServiceExt;

struct World {
    _dir: tempfile::TempDir,
    app: Router,
    bucket: String,
    access_key_id: String,
    secret: Vec<u8>,
    region: String,
    silo_root: std::path::PathBuf,
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
    let project = project(&store, "chunked-proj");
    let alice = principal(&store, "chunked-alice");
    bind(&store, &alice, "member", &project);
    let created = keep
        .create_access_key(&project, &alice, "chunked-test")
        .expect("access key");
    let region = DEFAULT_REGION.to_owned();
    let silo_root = dir.path().join("silo");
    let client = Client::new(store.clone(), &silo_root).with_max_object_bytes(8 * 1024 * 1024);
    let app = http::router_with(client, Some(keep.clone()), region.clone());

    let who = Caller {
        principal: alice.clone(),
        principal_name: "chunked-alice".into(),
        project: Some(project.clone()),
        roles: vec!["member".into()],
        system: false,
    };
    let body = serde_json::json!({
        "name": "chunks",
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
        bucket: "chunks".into(),
        access_key_id: created.access_key.access_key_id,
        secret: created.secret_access_key.into_bytes(),
        region,
        silo_root,
    }
}

fn amz_now() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

#[allow(clippy::too_many_arguments)]
async fn signed_put(
    app: &Router,
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
) -> (StatusCode, Vec<u8>) {
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), payload_hash.into()),
        ("x-amz-date".into(), amz_date.into()),
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), body.len().to_string()),
    ];
    for (k, v) in extra_headers {
        headers.push(((*k).into(), (*v).into()));
    }
    headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let signed_names: Vec<String> = headers.iter().map(|(k, _)| k.clone()).collect();
    let sig = sigv4::sign_request(
        "PUT",
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
    let mut builder = Request::builder().method("PUT").uri(&uri);
    for (k, v) in &headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder = builder.header(header::AUTHORIZATION, &auth);
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn signed_get(
    app: &Router,
    path: &str,
    access_key_id: &str,
    secret: &[u8],
    region: &str,
) -> (StatusCode, Vec<u8>) {
    let amz_date = amz_now();
    let host = "silo.test";
    let headers = vec![
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), EMPTY_PAYLOAD_SHA256.into()),
        ("x-amz-date".into(), amz_date.clone()),
    ];
    let sig = sigv4::sign_request(
        "GET",
        path,
        "",
        &headers,
        EMPTY_PAYLOAD_SHA256,
        access_key_id,
        secret,
        region,
        &amz_date,
    );
    let date_stamp: String = amz_date.chars().take(8).collect();
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{date_stamp}/{region}/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={sig}"
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("host", host)
                .header("x-amz-content-sha256", EMPTY_PAYLOAD_SHA256)
                .header("x-amz-date", &amz_date)
                .header(header::AUTHORIZATION, auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

fn scope(amz_date: &str, region: &str) -> String {
    format!("{}/{}/s3/aws4_request", &amz_date[..8], region)
}

#[tokio::test]
async fn signed_chunked_put_round_trip() {
    let w = world().await;
    let payload = b"hello aws-chunked streaming put";
    let amz_date = amz_now();
    let path = format!("/{}/chunked.txt", w.bucket);
    let date_stamp = &amz_date[..8];
    let signing_key = derive_signing_key(&w.secret, date_stamp, &w.region, "s3");
    let scope = scope(&amz_date, &w.region);

    // Seed signature must match Authorization; frame after we know the seed.
    // Compute seed by signing headers first with a temporary body length, then
    // build the framed body and re-sign — same pattern as SDKs (headers known
    // before framing). We sign once with the final Content-Length.
    let host = "silo.test";
    let decoded = payload.len().to_string();
    // Trial: build framed body using a placeholder seed, then we need real seed.
    // Approach: sign_request for headers → that signature IS the seed → frame with it.
    let mut trial_headers = vec![
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), "0".into()), // replaced below
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), STREAMING_PAYLOAD.into()),
        ("x-amz-date".into(), amz_date.clone()),
        ("x-amz-decoded-content-length".into(), decoded.clone()),
    ];
    // Build body with a throwaway seed, measure length, then re-frame with real seed.
    let (tmp, _) = frame_signed_chunks(
        payload,
        8,
        &signing_key,
        &amz_date,
        &scope,
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    trial_headers[1].1 = tmp.len().to_string();
    trial_headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let seed = sigv4::sign_request(
        "PUT",
        &path,
        "",
        &trial_headers,
        STREAMING_PAYLOAD,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
    );
    let (framed, _) = frame_signed_chunks(payload, 8, &signing_key, &amz_date, &scope, &seed);
    assert_eq!(
        framed.len(),
        tmp.len(),
        "framing length independent of seed"
    );

    let (status, body) = signed_put(
        &w.app,
        &path,
        "",
        &framed,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
        STREAMING_PAYLOAD,
        host,
        &[("x-amz-decoded-content-length", &decoded)],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "chunked put: {}",
        String::from_utf8_lossy(&body)
    );

    let (status, got) = signed_get(&w.app, &path, &w.access_key_id, &w.secret, &w.region).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&got[..], payload);
}

#[tokio::test]
async fn tampered_chunk_leaves_no_object() {
    let w = world().await;
    let payload = b"do-not-store-me";
    let amz_date = amz_now();
    let path = format!("/{}/tampered.txt", w.bucket);
    let date_stamp = &amz_date[..8];
    let signing_key = derive_signing_key(&w.secret, date_stamp, &w.region, "s3");
    let scope = scope(&amz_date, &w.region);
    let host = "silo.test";
    let decoded = payload.len().to_string();

    let (tmp, _) = frame_signed_chunks(
        payload,
        64,
        &signing_key,
        &amz_date,
        &scope,
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let mut headers = vec![
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), tmp.len().to_string()),
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), STREAMING_PAYLOAD.into()),
        ("x-amz-date".into(), amz_date.clone()),
        ("x-amz-decoded-content-length".into(), decoded.clone()),
    ];
    headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let seed = sigv4::sign_request(
        "PUT",
        &path,
        "",
        &headers,
        STREAMING_PAYLOAD,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
    );
    let (mut framed, _) = frame_signed_chunks(payload, 64, &signing_key, &amz_date, &scope, &seed);
    // Flip a payload byte inside the first chunk (after the header line).
    if let Some(pos) = framed.windows(2).position(|w| w == b"\r\n") {
        let data_start = pos + 2;
        if data_start < framed.len() {
            framed[data_start] ^= 0xff;
        }
    }

    let (status, body) = signed_put(
        &w.app,
        &path,
        "",
        &framed,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
        STREAMING_PAYLOAD,
        host,
        &[("x-amz-decoded-content-length", &decoded)],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "SignatureDoesNotMatch"
    );

    let (status, _) = signed_get(&w.app, &path, &w.access_key_id, &w.secret, &w.region).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No leftover object or staging temp files.
    let mut found_object = false;
    let mut found_tmp = false;
    if w.silo_root.exists() {
        for entry in walkdir_files(&w.silo_root) {
            if entry.contains(".tmp-") {
                found_tmp = true;
            }
            if entry.contains("tampered") && !entry.contains(".tmp-") {
                found_object = true;
            }
        }
    }
    assert!(!found_object, "tampered upload must not leave an object");
    assert!(
        !found_tmp,
        "tampered upload must not leave .tmp-* staging files"
    );
}

fn walkdir_files(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.display().to_string());
            }
        }
    }
    out
}

#[tokio::test]
async fn wrong_decoded_length_rejected() {
    let w = world().await;
    let payload = b"length-check";
    let amz_date = amz_now();
    let path = format!("/{}/bad-len.txt", w.bucket);
    let date_stamp = &amz_date[..8];
    let signing_key = derive_signing_key(&w.secret, date_stamp, &w.region, "s3");
    let scope = scope(&amz_date, &w.region);
    let host = "silo.test";
    // Declare too many decoded bytes.
    let decoded = (payload.len() + 10).to_string();

    let (tmp, _) = frame_signed_chunks(
        payload,
        64,
        &signing_key,
        &amz_date,
        &scope,
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let mut headers = vec![
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), tmp.len().to_string()),
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), STREAMING_PAYLOAD.into()),
        ("x-amz-date".into(), amz_date.clone()),
        ("x-amz-decoded-content-length".into(), decoded.clone()),
    ];
    headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let seed = sigv4::sign_request(
        "PUT",
        &path,
        "",
        &headers,
        STREAMING_PAYLOAD,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
    );
    let (framed, _) = frame_signed_chunks(payload, 64, &signing_key, &amz_date, &scope, &seed);
    let (status, body) = signed_put(
        &w.app,
        &path,
        "",
        &framed,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
        STREAMING_PAYLOAD,
        host,
        &[("x-amz-decoded-content-length", &decoded)],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "IncompleteBody"
    );
}

#[tokio::test]
async fn unsigned_trailer_crc32_and_sha256() {
    let w = world().await;
    for (algo, key) in [
        (TrailerChecksum::Crc32, "trailer-crc32.bin"),
        (TrailerChecksum::Sha256, "trailer-sha256.bin"),
    ] {
        let payload = b"unsigned trailer payload bytes";
        let amz_date = amz_now();
        let path = format!("/{}/{}", w.bucket, key);
        let host = "silo.test";
        let decoded = payload.len().to_string();
        let framed = frame_unsigned_trailer(payload, 7, algo);
        let trailer_name = algo.header_name();

        let (status, body) = signed_put(
            &w.app,
            &path,
            "",
            &framed,
            &w.access_key_id,
            &w.secret,
            &w.region,
            &amz_date,
            STREAMING_UNSIGNED_TRAILER,
            host,
            &[
                ("x-amz-decoded-content-length", &decoded),
                ("x-amz-trailer", trailer_name),
            ],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{key}: {}",
            String::from_utf8_lossy(&body)
        );
        let (status, got) = signed_get(&w.app, &path, &w.access_key_id, &w.secret, &w.region).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&got[..], payload);

        // Bad checksum
        let mut bad = frame_unsigned_trailer(payload, 7, algo);
        // Corrupt the base64 value slightly.
        if let Some(i) = bad.iter().rposition(|&b| b == b'=') {
            bad[i - 1] = if bad[i - 1] == b'A' { b'B' } else { b'A' };
        }
        let path_bad = format!("/{}/bad-{key}", w.bucket);
        let (status, body) = signed_put(
            &w.app,
            &path_bad,
            "",
            &bad,
            &w.access_key_id,
            &w.secret,
            &w.region,
            &amz_date,
            STREAMING_UNSIGNED_TRAILER,
            host,
            &[
                ("x-amz-decoded-content-length", &decoded),
                ("x-amz-trailer", trailer_name),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            extract_error_code(&String::from_utf8_lossy(&body)),
            "XAmzContentChecksumMismatch"
        );
    }
}

#[tokio::test]
async fn signed_trailer_put_round_trip() {
    let w = world().await;
    let payload = b"signed trailer object";
    let amz_date = amz_now();
    let path = format!("/{}/signed-trailer.txt", w.bucket);
    let date_stamp = &amz_date[..8];
    let signing_key = derive_signing_key(&w.secret, date_stamp, &w.region, "s3");
    let scope = scope(&amz_date, &w.region);
    let host = "silo.test";
    let decoded = payload.len().to_string();
    let algo = TrailerChecksum::Crc32c;

    let tmp = frame_signed_trailer(
        payload,
        9,
        &signing_key,
        &amz_date,
        &scope,
        "0000000000000000000000000000000000000000000000000000000000000000",
        algo,
    );
    let mut headers = vec![
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), tmp.len().to_string()),
        ("host".into(), host.into()),
        (
            "x-amz-content-sha256".into(),
            STREAMING_SIGNED_TRAILER.into(),
        ),
        ("x-amz-date".into(), amz_date.clone()),
        ("x-amz-decoded-content-length".into(), decoded.clone()),
        ("x-amz-trailer".into(), algo.header_name().into()),
    ];
    headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let seed = sigv4::sign_request(
        "PUT",
        &path,
        "",
        &headers,
        STREAMING_SIGNED_TRAILER,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
    );
    let framed = frame_signed_trailer(payload, 9, &signing_key, &amz_date, &scope, &seed, algo);
    assert_eq!(framed.len(), tmp.len());

    let (status, body) = signed_put(
        &w.app,
        &path,
        "",
        &framed,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
        STREAMING_SIGNED_TRAILER,
        host,
        &[
            ("x-amz-decoded-content-length", &decoded),
            ("x-amz-trailer", algo.header_name()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let (status, got) = signed_get(&w.app, &path, &w.access_key_id, &w.secret, &w.region).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&got[..], payload);
}

#[tokio::test]
async fn chunked_upload_part_round_trip() {
    let w = world().await;
    let amz_date = amz_now();
    let key = "mpu-chunked.bin";
    let path = format!("/{}/{}", w.bucket, key);
    let host = "silo.test";

    // CreateMultipartUpload with UNSIGNED-PAYLOAD
    let (status, body) = {
        let payload_hash = "UNSIGNED-PAYLOAD";
        let headers = vec![
            ("host".into(), host.into()),
            ("x-amz-content-sha256".into(), payload_hash.into()),
            ("x-amz-date".into(), amz_date.clone()),
            ("content-length".into(), "0".into()),
        ];
        let sig = sigv4::sign_request(
            "POST",
            &path,
            "uploads",
            &headers,
            payload_hash,
            &w.access_key_id,
            &w.secret,
            &w.region,
            &amz_date,
        );
        let date_stamp: String = amz_date.chars().take(8).collect();
        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{date_stamp}/{}/s3/aws4_request, SignedHeaders=content-length;host;x-amz-content-sha256;x-amz-date, Signature={sig}",
            w.access_key_id, w.region
        );
        let resp = w
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{path}?uploads"))
                    .header("host", host)
                    .header("x-amz-content-sha256", payload_hash)
                    .header("x-amz-date", &amz_date)
                    .header("content-length", "0")
                    .header(header::AUTHORIZATION, auth)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        (status, bytes)
    };
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let xml = String::from_utf8_lossy(&body);
    let upload_id = {
        let s = xml.find("<UploadId>").unwrap() + 10;
        let e = xml.find("</UploadId>").unwrap();
        xml[s..e].to_owned()
    };

    let part = vec![b'p'; 100];
    let date_stamp = &amz_date[..8];
    let signing_key = derive_signing_key(&w.secret, date_stamp, &w.region, "s3");
    let scope = scope(&amz_date, &w.region);
    let decoded = part.len().to_string();
    let query = format!("partNumber=1&uploadId={upload_id}");

    let (tmp, _) = frame_signed_chunks(
        &part,
        32,
        &signing_key,
        &amz_date,
        &scope,
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let mut headers = vec![
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), tmp.len().to_string()),
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), STREAMING_PAYLOAD.into()),
        ("x-amz-date".into(), amz_date.clone()),
        ("x-amz-decoded-content-length".into(), decoded.clone()),
    ];
    headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let seed = sigv4::sign_request(
        "PUT",
        &path,
        &query,
        &headers,
        STREAMING_PAYLOAD,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
    );
    let (framed, _) = frame_signed_chunks(&part, 32, &signing_key, &amz_date, &scope, &seed);

    let (status, body) = signed_put(
        &w.app,
        &path,
        &query,
        &framed,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
        STREAMING_PAYLOAD,
        host,
        &[("x-amz-decoded-content-length", &decoded)],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "upload part: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn streamed_body_pieces_and_split_trailer() {
    use bytes::Bytes as B;
    use futures::stream;
    let w = world().await;
    let payload = b"piecewise-trailer-body";
    let amz_date = amz_now();
    let path = format!("/{}/pieces.bin", w.bucket);
    let host = "silo.test";
    let decoded = payload.len().to_string();
    let framed = frame_unsigned_trailer(payload, 5, TrailerChecksum::Crc32);
    let trailer_name = TrailerChecksum::Crc32.header_name();

    // Sign with full framed length.
    let mut headers = vec![
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), framed.len().to_string()),
        ("host".into(), host.into()),
        (
            "x-amz-content-sha256".into(),
            STREAMING_UNSIGNED_TRAILER.into(),
        ),
        ("x-amz-date".into(), amz_date.clone()),
        ("x-amz-decoded-content-length".into(), decoded.clone()),
        ("x-amz-trailer".into(), trailer_name.into()),
    ];
    headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let sig = sigv4::sign_request(
        "PUT",
        &path,
        "",
        &headers,
        STREAMING_UNSIGNED_TRAILER,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
    );
    let date_stamp: String = amz_date.chars().take(8).collect();
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={}/{date_stamp}/{}/s3/aws4_request, SignedHeaders={}, Signature={sig}",
        w.access_key_id,
        w.region,
        headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";")
    );

    let pieces = framed
        .chunks(3)
        .map(|c| Ok::<_, std::io::Error>(B::copy_from_slice(c)))
        .collect::<Vec<_>>();
    let body = Body::from_stream(stream::iter(pieces));

    let mut builder = Request::builder().method("PUT").uri(&path);
    for (k, v) in &headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder = builder.header(header::AUTHORIZATION, auth);
    let resp = w
        .app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "streamed trailer put");

    let (status, got) = signed_get(&w.app, &path, &w.access_key_id, &w.secret, &w.region).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&got[..], payload);
}

#[tokio::test]
async fn bad_chunk_after_good_chunk_http() {
    let w = world().await;
    let amz_date = amz_now();
    let path = format!("/{}/after-good.txt", w.bucket);
    let date_stamp = &amz_date[..8];
    let signing_key = derive_signing_key(&w.secret, date_stamp, &w.region, "s3");
    let scope = scope(&amz_date, &w.region);
    let host = "silo.test";
    let c1 = b"first-ok-chunk!!!!";
    let c2 = b"second";
    let decoded = (c1.len() + c2.len()).to_string();

    // Build body with placeholder seed length first.
    let seed0 = "0".repeat(64);
    let sig1 = chunk_signature(&signing_key, &amz_date, &scope, &seed0, c1);
    let mut trial = Vec::new();
    trial.extend_from_slice(format!("{:x};chunk-signature={sig1}\r\n", c1.len()).as_bytes());
    trial.extend_from_slice(c1);
    trial.extend_from_slice(b"\r\n");
    trial.extend_from_slice(
        format!(
            "{:x};chunk-signature={}\r\n",
            c2.len(),
            "1111111111111111111111111111111111111111111111111111111111111111"
        )
        .as_bytes(),
    );
    trial.extend_from_slice(c2);
    trial.extend_from_slice(b"\r\n");

    let mut headers = vec![
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), trial.len().to_string()),
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), STREAMING_PAYLOAD.into()),
        ("x-amz-date".into(), amz_date.clone()),
        ("x-amz-decoded-content-length".into(), decoded.clone()),
    ];
    headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let seed = sigv4::sign_request(
        "PUT",
        &path,
        "",
        &headers,
        STREAMING_PAYLOAD,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
    );
    let sig1 = chunk_signature(&signing_key, &amz_date, &scope, &seed, c1);
    let mut framed = Vec::new();
    framed.extend_from_slice(format!("{:x};chunk-signature={sig1}\r\n", c1.len()).as_bytes());
    framed.extend_from_slice(c1);
    framed.extend_from_slice(b"\r\n");
    framed.extend_from_slice(
        format!(
            "{:x};chunk-signature={}\r\n",
            c2.len(),
            "1111111111111111111111111111111111111111111111111111111111111111"
        )
        .as_bytes(),
    );
    framed.extend_from_slice(c2);
    framed.extend_from_slice(b"\r\n");
    assert_eq!(framed.len(), trial.len());

    let (status, body) = signed_put(
        &w.app,
        &path,
        "",
        &framed,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
        STREAMING_PAYLOAD,
        host,
        &[("x-amz-decoded-content-length", &decoded)],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "SignatureDoesNotMatch"
    );
    let mut found_tmp = false;
    for entry in walkdir_files(&w.silo_root) {
        if entry.contains(".tmp-") {
            found_tmp = true;
        }
    }
    assert!(!found_tmp, "no .tmp-* after bad chunk signature");
}

#[tokio::test]
async fn truncated_chunked_body_http() {
    let w = world().await;
    let amz_date = amz_now();
    let path = format!("/{}/trunc.txt", w.bucket);
    let date_stamp = &amz_date[..8];
    let signing_key = derive_signing_key(&w.secret, date_stamp, &w.region, "s3");
    let scope = scope(&amz_date, &w.region);
    let host = "silo.test";
    // Two logical chunks; we send only the first (complete) and omit the rest + zero chunk.
    let c1 = b"good-chunk-only!!";
    let c2 = b"never-sent";
    let decoded = (c1.len() + c2.len()).to_string();

    let seed0 = "0".repeat(64);
    let sig1 = chunk_signature(&signing_key, &amz_date, &scope, &seed0, c1);
    let mut trial = Vec::new();
    trial.extend_from_slice(format!("{:x};chunk-signature={sig1}\r\n", c1.len()).as_bytes());
    trial.extend_from_slice(c1);
    trial.extend_from_slice(b"\r\n");
    // truncated: stop here (no second chunk, no final zero chunk)

    let mut headers = vec![
        ("content-encoding".into(), "aws-chunked".into()),
        ("content-length".into(), trial.len().to_string()),
        ("host".into(), host.into()),
        ("x-amz-content-sha256".into(), STREAMING_PAYLOAD.into()),
        ("x-amz-date".into(), amz_date.clone()),
        ("x-amz-decoded-content-length".into(), decoded.clone()),
    ];
    headers.sort_by(|a: &(String, String), b| a.0.cmp(&b.0));
    let seed = sigv4::sign_request(
        "PUT",
        &path,
        "",
        &headers,
        STREAMING_PAYLOAD,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
    );
    let sig1 = chunk_signature(&signing_key, &amz_date, &scope, &seed, c1);
    let mut truncated = Vec::new();
    truncated.extend_from_slice(format!("{:x};chunk-signature={sig1}\r\n", c1.len()).as_bytes());
    truncated.extend_from_slice(c1);
    truncated.extend_from_slice(b"\r\n");
    assert_eq!(truncated.len(), trial.len());

    let (status, body) = signed_put(
        &w.app,
        &path,
        "",
        &truncated,
        &w.access_key_id,
        &w.secret,
        &w.region,
        &amz_date,
        STREAMING_PAYLOAD,
        host,
        &[("x-amz-decoded-content-length", &decoded)],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        extract_error_code(&String::from_utf8_lossy(&body)),
        "IncompleteBody"
    );

    let (get_status, _) = signed_get(&w.app, &path, &w.access_key_id, &w.secret, &w.region).await;
    assert_eq!(
        get_status,
        StatusCode::NOT_FOUND,
        "no object after truncate"
    );

    let mut found_tmp = false;
    let mut found_object = false;
    for entry in walkdir_files(&w.silo_root) {
        if entry.contains(".tmp-") {
            found_tmp = true;
        }
        if entry.contains("trunc.txt") && !entry.contains(".tmp-") {
            found_object = true;
        }
    }
    assert!(!found_object, "truncated upload must not leave an object");
    assert!(!found_tmp, "truncated upload must not leave .tmp-* files");
}
