// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 versioning on the path-style surface, driven with SigV4-signed requests
//! as an S3 client sends them: PutBucketVersioning, GetBucketVersioning,
//! `x-amz-version-id` on PutObject, GetObject/HeadObject by `versionId`,
//! ListObjectVersions, and DeleteObject by `versionId`.
//!
//! A `versionId` here is the object record's generation number in decimal, so
//! the expected ids in these tests are `1`, `2`, `3` — not opaque strings.

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

fn md5_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let mut h = md5::Md5::new();
    sha2::Digest::update(&mut h, bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn body_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn error_code(xml: &[u8]) -> String {
    let xml = body_text(xml);
    let start = xml.find("<Code>").map(|i| i + 6).unwrap_or(0);
    let end = xml.find("</Code>").unwrap_or(xml.len());
    xml[start..end].to_owned()
}

/// The text between two tags of the first element named `tag`.
fn tag_value(xml: &[u8], tag: &str) -> Option<String> {
    let xml = body_text(xml);
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_owned())
}

/// The contents of each `<Version>` element, in the order listed.
fn versions(xml: &[u8]) -> Vec<String> {
    let xml = body_text(xml);
    let mut out = Vec::new();
    let mut rest = xml.as_str();
    while let Some(start) = rest.find("<Version>") {
        rest = &rest[start + "<Version>".len()..];
        match rest.find("</Version>") {
            Some(end) => {
                out.push(rest[..end].to_owned());
                rest = &rest[end..];
            }
            None => break,
        }
    }
    out
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
    payload_hash: &str,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let amz_date = amz_now();
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), "silo.test".into()),
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

/// One signed call with Alice's key against the world's bucket.
impl World {
    async fn call(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
    ) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
        signed_call(
            &self.app,
            method,
            path,
            query,
            body,
            &self.access_key_id,
            &self.secret,
            &self.region,
            &sha256_hex(body),
        )
        .await
    }

    async fn create_bucket(&self, name: &str) {
        let (status, body, _) = self.call("PUT", &format!("/{name}"), "", b"").await;
        assert_eq!(
            status,
            StatusCode::OK,
            "create {name}: {}",
            body_text(&body)
        );
    }

    /// `PUT /{bucket}?versioning` with a VersioningConfiguration body.
    async fn put_versioning(&self, bucket: &str, status_value: &str) -> StatusCode {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
<Status>{status_value}</Status></VersioningConfiguration>"
        );
        self.call("PUT", &format!("/{bucket}"), "versioning", body.as_bytes())
            .await
            .0
    }

    async fn get_versioning(&self, bucket: &str) -> (StatusCode, Vec<u8>) {
        let (status, body, _) = self
            .call("GET", &format!("/{bucket}"), "versioning", b"")
            .await;
        (status, body)
    }

    /// PutObject; answers the `x-amz-version-id` header, which is absent when
    /// the bucket does not keep generations.
    async fn put(&self, bucket: &str, key: &str, bytes: &[u8]) -> Option<String> {
        let (status, body, headers) = self
            .call("PUT", &format!("/{bucket}/{key}"), "", bytes)
            .await;
        assert_eq!(status, StatusCode::OK, "put {key}: {}", body_text(&body));
        headers
            .get("x-amz-version-id")
            .map(|v| v.to_str().unwrap().to_owned())
    }

    async fn get(&self, bucket: &str, key: &str, version: Option<&str>) -> (StatusCode, Vec<u8>) {
        let query = match version {
            Some(v) => format!("versionId={v}"),
            None => String::new(),
        };
        let (status, body, _) = self
            .call("GET", &format!("/{bucket}/{key}"), &query, b"")
            .await;
        (status, body)
    }

    async fn head(
        &self,
        bucket: &str,
        key: &str,
        version: Option<&str>,
    ) -> (
        StatusCode,
        axum::http::HeaderMap,
        Option<String>,
        Option<String>,
    ) {
        let query = match version {
            Some(v) => format!("versionId={v}"),
            None => String::new(),
        };
        let (status, _, headers) = self
            .call("HEAD", &format!("/{bucket}/{key}"), &query, b"")
            .await;
        let version_id = headers
            .get("x-amz-version-id")
            .map(|v| v.to_str().unwrap().to_owned());
        let etag = headers
            .get(header::ETAG)
            .map(|v| v.to_str().unwrap().to_owned());
        (status, headers, version_id, etag)
    }

    async fn delete(&self, bucket: &str, key: &str, version: Option<&str>) -> StatusCode {
        let query = match version {
            Some(v) => format!("versionId={v}"),
            None => String::new(),
        };
        let (status, body, _) = self
            .call("DELETE", &format!("/{bucket}/{key}"), &query, b"")
            .await;
        assert!(
            status == StatusCode::NO_CONTENT || status == StatusCode::NOT_FOUND,
            "delete {key}: {} {}",
            status,
            body_text(&body)
        );
        status
    }

    async fn list_versions(&self, bucket: &str, query: &str) -> (StatusCode, Vec<u8>) {
        let (status, body, _) = self
            .call(
                "GET",
                &format!("/{bucket}"),
                &format!("versions&{query}"),
                b"",
            )
            .await;
        (status, body)
    }
}

/// One world, one fresh bucket, versioning left as the caller set it.
async fn world(bucket: &str) -> World {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("store");
    let keep = keep::Client::open(&store).expect("keep");
    let region = DEFAULT_REGION.to_owned();

    let proj = project(&store, "versioning-proj");
    let alice = principal(&store, "versioning-alice");
    bind(&store, &alice, "member", &proj);
    let key = keep
        .create_access_key(&proj, &alice, "versioning")
        .expect("access key");

    let caller = Caller {
        principal: alice.clone(),
        principal_name: "versioning-alice".into(),
        project: Some(proj),
        roles: vec!["member".into()],
        system: false,
    };
    let client = Client::new(store.clone(), dir.path().join("silo"));
    let app = http::router_with(client, Some(keep), region.clone());

    // The bucket is made through the S3 surface itself, with a signed call.
    let world = World {
        _dir: dir,
        app,
        access_key_id: key.access_key.access_key_id,
        secret: key.secret_access_key.into_bytes(),
        region,
    };
    world.create_bucket(bucket).await;
    // Keep the caller alive for the store's lifetime; it is not needed again.
    drop(caller);
    world
}

// ---- PutBucketVersioning / GetBucketVersioning -------------------------------

#[tokio::test]
async fn get_versioning_has_no_status_until_it_is_set() {
    let w = world("v-never").await;
    let (status, body) = w.get_versioning("v-never").await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    let xml = body_text(&body);
    assert!(
        xml.contains("VersioningConfiguration"),
        "not a versioning document: {xml}"
    );
    assert!(!xml.contains("<Status>"), "expected no Status: {xml}");
}

#[tokio::test]
async fn put_versioning_enabled_is_answered_by_get() {
    let w = world("v-enable").await;
    assert_eq!(
        w.put_versioning("v-enable", "Enabled").await,
        StatusCode::OK
    );
    let (status, body) = w.get_versioning("v-enable").await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    assert_eq!(tag_value(&body, "Status").as_deref(), Some("Enabled"));
}

#[tokio::test]
async fn put_versioning_suspended_answers_no_status_again() {
    let w = world("v-suspend").await;
    assert_eq!(
        w.put_versioning("v-suspend", "Enabled").await,
        StatusCode::OK
    );
    assert_eq!(
        w.put_versioning("v-suspend", "Suspended").await,
        StatusCode::OK
    );
    let (status, body) = w.get_versioning("v-suspend").await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    // The store has one flag, so off is answered as never set — no Status.
    assert_eq!(tag_value(&body, "Status"), None);
}

#[tokio::test]
async fn put_versioning_bad_status_is_malformed() {
    let w = world("v-bad").await;
    let body = br#"<?xml version="1.0" encoding="UTF-8"?><VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>YesPlease</Status></VersioningConfiguration>"#;
    let (status, resp, _) = w.call("PUT", "/v-bad", "versioning", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&resp));
    assert_eq!(error_code(&resp), "MalformedXML");
    // The bucket was not changed by the refused call.
    let (_, got) = w.get_versioning("v-bad").await;
    assert_eq!(tag_value(&got, "Status"), None);
}

#[tokio::test]
async fn versioning_routes_are_visible_only_to_the_owning_project() {
    let w = world("v-private").await;
    // A bucket that is not there is NoSuchBucket on both subresources.
    let (status, body, _) = w.call("GET", "/v-nope", "versioning", b"").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));
    assert_eq!(error_code(&body), "NoSuchBucket");
    let (status, body, _) = w.call("GET", "/v-nope", "versions", b"").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));
    assert_eq!(error_code(&body), "NoSuchBucket");
}

#[tokio::test]
async fn an_unsigned_versioning_call_is_refused() {
    let w = world("v-anon").await;
    let resp = w
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v-anon?versioning")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // The same rule as every other silo route: no caller, no answer.
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---- PutObject answers x-amz-version-id -------------------------------------

#[tokio::test]
async fn put_on_a_plain_bucket_answers_no_version_id() {
    let w = world("v-plain").await;
    assert_eq!(w.put("v-plain", "k", b"one").await, None);
}

#[tokio::test]
async fn put_on_a_versioned_bucket_numbers_each_generation() {
    let w = world("v-put").await;
    assert_eq!(w.put_versioning("v-put", "Enabled").await, StatusCode::OK);
    assert_eq!(w.put("v-put", "k", b"one").await.as_deref(), Some("1"));
    assert_eq!(w.put("v-put", "k", b"two").await.as_deref(), Some("2"));
    assert_eq!(w.put("v-put", "k", b"three").await.as_deref(), Some("3"));
}

// ---- GetObject / HeadObject by versionId ------------------------------------

#[tokio::test]
async fn get_version_answers_that_generation() {
    let w = world("v-get").await;
    w.put_versioning("v-get", "Enabled").await;
    w.put("v-get", "k", b"one").await;
    w.put("v-get", "k", b"two").await;
    // The plain key path serves the newest bytes either way.
    let (status, body) = w.get("v-get", "k", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"two");
    for (version, want) in [("1", &b"one"[..]), ("2", &b"two"[..])] {
        let (status, body) = w.get("v-get", "k", Some(version)).await;
        assert_eq!(status, StatusCode::OK, "version {version}");
        assert_eq!(body, want);
    }
}

#[tokio::test]
async fn get_carries_x_amz_version_id() {
    let w = world("v-gethdr").await;
    w.put_versioning("v-gethdr", "Enabled").await;
    w.put("v-gethdr", "k", b"one").await;
    w.put("v-gethdr", "k", b"two").await;
    let query = "versionId=1";
    let (status, _, headers) = w.call("GET", "/v-gethdr/k", query, b"").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("x-amz-version-id").map(|v| v.to_str().unwrap()),
        Some("1")
    );
    // And on the plain read, naming the generation actually served.
    let (status, _, headers) = w.call("GET", "/v-gethdr/k", "", b"").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("x-amz-version-id").map(|v| v.to_str().unwrap()),
        Some("2")
    );
}

#[tokio::test]
async fn get_a_generation_that_was_never_written_is_no_such_version() {
    let w = world("v-missing").await;
    w.put_versioning("v-missing", "Enabled").await;
    w.put("v-missing", "k", b"one").await;
    let (status, body) = w.get("v-missing", "k", Some("7")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));
    assert_eq!(error_code(&body), "NoSuchVersion");
    // A versionId that is not a generation number is the same answer.
    let (status, body) = w.get("v-missing", "k", Some("abc")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "NoSuchVersion");
    // A key that is not there at all is still NoSuchKey, not NoSuchVersion.
    let (status, body) = w.get("v-missing", "never", Some("1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));
    assert_eq!(error_code(&body), "NoSuchVersion");
}

#[tokio::test]
async fn head_version_answers_that_generation_with_its_etag() {
    let w = world("v-head").await;
    w.put_versioning("v-head", "Enabled").await;
    w.put("v-head", "k", b"one").await;
    w.put("v-head", "k", b"two").await;
    let (status, headers, version_id, etag) = w.head("v-head", "k", Some("1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(version_id.as_deref(), Some("1"));
    assert_eq!(
        etag.as_deref(),
        Some(format!("\"{}\"", md5_hex(b"one")).as_str())
    );
    assert_eq!(
        headers
            .get(header::CONTENT_LENGTH)
            .map(|v| v.to_str().unwrap()),
        Some("3")
    );
    let (status, _, version_id, etag) = w.head("v-head", "k", Some("2")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(version_id.as_deref(), Some("2"));
    assert_eq!(
        etag.as_deref(),
        Some(format!("\"{}\"", md5_hex(b"two")).as_str())
    );
    let (status, _) = w.get("v-head", "k", Some("9")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _, _) = w.head("v-head", "k", Some("9")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_version_of_a_plain_bucket_is_still_readable() {
    // Generation 1 exists whether or not the bucket keeps generations, so
    // `?versionId=1` reads a plain bucket's only record.
    let w = world("v-plainver").await;
    w.put("v-plainver", "k", b"only").await;
    let (status, body) = w.get("v-plainver", "k", Some("1")).await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    assert_eq!(body, b"only");
    // A second put replaced it, so generation 1 is gone.
    w.put("v-plainver", "k", b"next").await;
    let (status, _) = w.get("v-plainver", "k", Some("1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---- ListObjectVersions -----------------------------------------------------

#[tokio::test]
async fn list_versions_shows_every_generation_newest_first() {
    let w = world("v-list").await;
    w.put_versioning("v-list", "Enabled").await;
    w.put("v-list", "a", b"one").await;
    w.put("v-list", "a", b"two").await;
    w.put("v-list", "b", b"bee").await;
    let (status, body) = w.list_versions("v-list", "").await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    let xml = body_text(&body);
    assert!(xml.contains("ListVersionsResult"), "{xml}");
    assert_eq!(tag_value(&body, "Name").as_deref(), Some("v-list"));
    let rows = versions(&body);
    assert_eq!(rows.len(), 3, "{xml}");
    // Key order first, newest generation first within the key.
    let expect: [(&str, &str, &str); 3] =
        [("a", "2", "true"), ("a", "1", "false"), ("b", "1", "true")];
    for (row, (key, vid, latest)) in rows.iter().zip(expect) {
        assert_eq!(tag_value(row.as_bytes(), "Key").as_deref(), Some(key));
        assert_eq!(tag_value(row.as_bytes(), "VersionId").as_deref(), Some(vid));
        assert_eq!(
            tag_value(row.as_bytes(), "IsLatest").as_deref(),
            Some(latest)
        );
        assert!(
            tag_value(row.as_bytes(), "LastModified").is_some_and(|v| v.ends_with('Z')),
            "{row}"
        );
        assert!(tag_value(row.as_bytes(), "ETag").is_some(), "{row}");
        assert_eq!(tag_value(row.as_bytes(), "Size").as_deref(), Some("3"));
    }
    assert_eq!(tag_value(&body, "IsTruncated").as_deref(), Some("false"));
}

#[tokio::test]
async fn list_versions_honours_prefix() {
    let w = world("v-prefix").await;
    w.put_versioning("v-prefix", "Enabled").await;
    for key in ["docs/a", "docs/b", "other/c"] {
        w.put("v-prefix", key, b"x").await;
    }
    let (_, body) = w.list_versions("v-prefix", "prefix=docs/").await;
    let keys: Vec<String> = versions(&body)
        .iter()
        .map(|v| tag_value(v.as_bytes(), "Key").unwrap_or_default())
        .collect();
    assert_eq!(keys, ["docs/a", "docs/b"]);
    assert_eq!(tag_value(&body, "Prefix").as_deref(), Some("docs/"));
}

#[tokio::test]
async fn list_versions_pages_with_max_keys_and_markers() {
    let w = world("v-page").await;
    w.put_versioning("v-page", "Enabled").await;
    // Three generations at a, one at b: a page of two stops inside key a.
    w.put("v-page", "a", b"1").await;
    w.put("v-page", "a", b"2").await;
    w.put("v-page", "a", b"3").await;
    w.put("v-page", "b", b"4").await;

    let (status, first) = w.list_versions("v-page", "max-keys=2").await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&first));
    assert_eq!(versions(&first).len(), 2);
    assert_eq!(
        tag_value(&first, "IsTruncated").as_deref(),
        Some("true"),
        "{}",
        body_text(&first)
    );
    assert_eq!(tag_value(&first, "MaxKeys").as_deref(), Some("2"));
    // The answer says where to resume: after the last row of this page.
    assert_eq!(tag_value(&first, "NextKeyMarker").as_deref(), Some("a"));
    assert_eq!(
        tag_value(&first, "NextVersionIdMarker").as_deref(),
        Some("2")
    );

    let (_, second) = w
        .list_versions("v-page", "max-keys=2&key-marker=a&version-id-marker=2")
        .await;
    let rows = versions(&second);
    let got: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                tag_value(r.as_bytes(), "Key").unwrap_or_default(),
                tag_value(r.as_bytes(), "VersionId").unwrap_or_default(),
            )
        })
        .collect();
    let got: Vec<(&str, &str)> = got.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(got, [("a", "1"), ("b", "1")], "{}", body_text(&second));
    assert_eq!(
        tag_value(&second, "IsTruncated").as_deref(),
        Some("false"),
        "{}",
        body_text(&second)
    );
    // The markers read back as sent.
    assert_eq!(tag_value(&second, "KeyMarker").as_deref(), Some("a"));
    assert_eq!(tag_value(&second, "VersionIdMarker").as_deref(), Some("2"));
}

#[tokio::test]
async fn list_versions_key_marker_alone_skips_the_whole_key() {
    let w = world("v-marker").await;
    w.put_versioning("v-marker", "Enabled").await;
    w.put("v-marker", "a", b"1").await;
    w.put("v-marker", "a", b"2").await;
    w.put("v-marker", "b", b"3").await;
    // Without a version-id marker the key marker resumes after key `a`.
    let (_, body) = w.list_versions("v-marker", "key-marker=a").await;
    let rows = versions(&body);
    assert_eq!(rows.len(), 1, "{}", body_text(&body));
    assert_eq!(tag_value(rows[0].as_bytes(), "Key").as_deref(), Some("b"));
}

#[tokio::test]
async fn list_versions_of_an_empty_bucket_is_an_empty_page() {
    let w = world("v-empty").await;
    let (status, body) = w.list_versions("v-empty", "").await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    assert!(versions(&body).is_empty());
    assert_eq!(tag_value(&body, "IsTruncated").as_deref(), Some("false"));
    // No delete marker exists in the store, so none is ever listed.
    assert!(!body_text(&body).contains("DeleteMarker"));
}

// ---- DeleteObject with and without versionId --------------------------------

#[tokio::test]
async fn delete_version_removes_exactly_that_generation() {
    let w = world("v-del").await;
    w.put_versioning("v-del", "Enabled").await;
    w.put("v-del", "k", b"one").await;
    w.put("v-del", "k", b"two").await;
    assert_eq!(
        w.delete("v-del", "k", Some("1")).await,
        StatusCode::NO_CONTENT
    );
    // Generation 1 is gone; the current one is untouched.
    let (status, _) = w.get("v-del", "k", Some("1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = w.get("v-del", "k", Some("2")).await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    assert_eq!(body, b"two");
    let (_, listed) = w.list_versions("v-del", "").await;
    assert_eq!(versions(&listed).len(), 1);
}

#[tokio::test]
async fn delete_the_current_version_leaves_the_newest_survivor_current() {
    let w = world("v-delcur").await;
    w.put_versioning("v-delcur", "Enabled").await;
    w.put("v-delcur", "k", b"one").await;
    w.put("v-delcur", "k", b"two").await;
    w.put("v-delcur", "k", b"three").await;
    assert_eq!(
        w.delete("v-delcur", "k", Some("3")).await,
        StatusCode::NO_CONTENT
    );
    // The key path now serves generation 2, and says so.
    let (status, _, headers) = w.call("GET", "/v-delcur/k", "", b"").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("x-amz-version-id").map(|v| v.to_str().unwrap()),
        Some("2")
    );
    let (status, body) = w.get("v-delcur", "k", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"two");
    let (_, listed) = w.list_versions("v-delcur", "").await;
    let latest: Vec<String> = versions(&listed)
        .iter()
        .filter(|r| tag_value(r.as_bytes(), "IsLatest").as_deref() == Some("true"))
        .map(|r| tag_value(r.as_bytes(), "VersionId").unwrap_or_default())
        .collect();
    assert_eq!(latest, ["2"]);
}

#[tokio::test]
async fn delete_every_version_takes_the_key_with_it() {
    let w = world("v-delall").await;
    w.put_versioning("v-delall", "Enabled").await;
    w.put("v-delall", "k", b"one").await;
    w.put("v-delall", "k", b"two").await;
    assert_eq!(
        w.delete("v-delall", "k", Some("2")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        w.delete("v-delall", "k", Some("1")).await,
        StatusCode::NO_CONTENT
    );
    // The key is gone from the plain path, from listing, and from disk.
    let (status, _) = w.get("v-delall", "k", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, listed) = w.list_versions("v-delall", "").await;
    assert!(versions(&listed).is_empty(), "{}", body_text(&listed));
    // Nothing is left to delete, so the plain delete says the key is not there.
    let (status, body, _) = w.call("DELETE", "/v-delall/k", "", b"").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));
    assert_eq!(error_code(&body), "NoSuchKey");
}

#[tokio::test]
async fn delete_a_version_that_is_not_there_is_no_such_version() {
    let w = world("v-delmiss").await;
    w.put_versioning("v-delmiss", "Enabled").await;
    w.put("v-delmiss", "k", b"one").await;
    let (status, body, _) = w.call("DELETE", "/v-delmiss/k", "versionId=42", b"").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));
    assert_eq!(error_code(&body), "NoSuchVersion");
    // The one generation it does have is still there.
    let (status, body) = w.get("v-delmiss", "k", Some("1")).await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
}

#[tokio::test]
async fn plain_delete_on_a_versioned_bucket_removes_the_key_as_before() {
    // The store keeps no delete marker, so a plain delete is unchanged: the
    // whole key goes and no marker is listed in its place.
    let w = world("v-plaindel").await;
    w.put_versioning("v-plaindel", "Enabled").await;
    w.put("v-plaindel", "k", b"one").await;
    w.put("v-plaindel", "k", b"two").await;
    assert_eq!(
        w.delete("v-plaindel", "k", None).await,
        StatusCode::NO_CONTENT
    );
    let (status, _) = w.get("v-plaindel", "k", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = w.get("v-plaindel", "k", Some("1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, listed) = w.list_versions("v-plaindel", "").await;
    assert!(versions(&listed).is_empty(), "{}", body_text(&listed));
    assert!(!body_text(&listed).contains("DeleteMarker"));
}

#[tokio::test]
async fn delete_version_leaves_other_keys_alone() {
    let w = world("v-neighbours").await;
    w.put_versioning("v-neighbours", "Enabled").await;
    w.put("v-neighbours", "a", b"1").await;
    w.put("v-neighbours", "a", b"2").await;
    w.put("v-neighbours", "b", b"keep me").await;
    assert_eq!(
        w.delete("v-neighbours", "a", Some("2")).await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = w.get("v-neighbours", "b", None).await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    assert_eq!(body, b"keep me");
    let (status, body) = w.get("v-neighbours", "a", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"1");
}

#[tokio::test]
async fn delete_version_answers_the_generation_it_took() {
    let w = world("v-delhdr").await;
    w.put_versioning("v-delhdr", "Enabled").await;
    w.put("v-delhdr", "k", b"one").await;
    w.put("v-delhdr", "k", b"two").await;
    // The header names the generation the call removed. What is left at the key
    // is the older generation, and the key path now serves it.
    let (status, _, headers) = w.call("DELETE", "/v-delhdr/k", "versionId=2", b"").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        headers.get("x-amz-version-id").map(|v| v.to_str().unwrap()),
        Some("2")
    );
    let (status, body) = w.get("v-delhdr", "k", None).await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    assert_eq!(body, b"one");
    let (status, _, headers) = w.call("DELETE", "/v-delhdr/k", "versionId=1", b"").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        headers.get("x-amz-version-id").map(|v| v.to_str().unwrap()),
        Some("1")
    );
    let (status, _) = w.get("v-delhdr", "k", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_reader_key_may_read_versions_but_not_delete_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).expect("store");
    let keep = keep::Client::open(&store).expect("keep");
    let region = DEFAULT_REGION.to_owned();
    let proj = project(&store, "versioning-read-proj");
    let writer = principal(&store, "versioning-writer");
    bind(&store, &writer, "member", &proj);
    let writer_key = keep
        .create_access_key(&proj, &writer, "writer")
        .expect("writer key");
    let reader = principal(&store, "versioning-reader");
    bind(&store, &reader, "reader", &proj);
    let reader_key = keep
        .create_access_key(&proj, &reader, "reader")
        .expect("reader key");
    let reader_secret = reader_key.secret_access_key.clone().into_bytes();
    let client = Client::new(store.clone(), dir.path().join("silo"));
    let app = http::router_with(client, Some(keep), region.clone());
    let member = World {
        _dir: dir,
        app: app.clone(),
        access_key_id: writer_key.access_key.access_key_id,
        secret: writer_key.secret_access_key.into_bytes(),
        region: region.clone(),
    };
    member.create_bucket("v-reader").await;
    member.put_versioning("v-reader", "Enabled").await;
    member.put("v-reader", "k", b"one").await;

    let (status, body, _) = signed_call(
        &app,
        "GET",
        "/v-reader/k",
        "versionId=1",
        b"",
        &reader_key.access_key.access_key_id,
        &reader_secret,
        &region,
        EMPTY_PAYLOAD_SHA256,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));
    assert_eq!(body, b"one");
    let (status, body, _) = signed_call(
        &app,
        "DELETE",
        "/v-reader/k",
        "versionId=1",
        b"",
        &reader_key.access_key.access_key_id,
        &reader_secret,
        &region,
        EMPTY_PAYLOAD_SHA256,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{}", body_text(&body));
    assert_eq!(error_code(&body), "AccessDenied");
}
