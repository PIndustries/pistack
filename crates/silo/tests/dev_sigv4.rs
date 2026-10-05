// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Dev-mode stack: `attach_caller` then `dev_admin` must leave `/silo` SigV4
//! alone so silo can authenticate the access key (no bearer+SigV4 conflict).

use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use chrono::Utc;
use gate::Keys;
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Meta};
use sha2::{Digest, Sha256};
use silo::http;
use silo::sigv4::{self, EMPTY_PAYLOAD_SHA256};
use silo::{Client, DEFAULT_REGION};
use tower::ServiceExt;

/// Mirror of gateway `dev_admin`, using the shared gate helper.
async fn dev_admin(mut req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none()
        && !gate::silo_sigv4_skips_bearer(req.uri().path(), req.headers(), req.uri().query())
    {
        req.extensions_mut().insert(Caller {
            principal: Id::new(kind::PRINCIPAL),
            principal_name: "dev-admin".into(),
            project: None,
            roles: vec!["admin".into()],
            system: true,
        });
    }
    next.run(req).await
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
    store
        .collection::<gate::RoleBinding>()
        .create(gate::RoleBinding {
            meta: Meta::new(
                gate::KIND_BINDING,
                format!("{role}-dev"),
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

#[tokio::test]
async fn dev_stack_sigv4_put_get_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (events, _) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).unwrap();
    let keys = Keys::load_or_create(&store).unwrap();
    let keep = keep::Client::open(&store).unwrap();
    let project = project(&store, "p");
    let alice = principal(&store, "alice");
    bind(&store, &alice, "member", &project);
    let created = keep.create_access_key(&project, &alice, "dev").unwrap();

    let silo =
        Client::new(store.clone(), dir.path().join("silo")).with_max_object_bytes(1024 * 1024);
    let region = DEFAULT_REGION.to_owned();
    let silo_router = http::router_with(silo, Some(keep), region.clone());

    // Create bucket with a real project caller (native API), before layering auth.
    let who = Caller {
        principal: alice.clone(),
        principal_name: "alice".into(),
        project: Some(project.clone()),
        roles: vec!["member".into()],
        system: false,
    };
    let body = serde_json::json!({"name": "widgets", "spec": {"versioning": false}}).to_string();
    let resp = silo_router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/buckets")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_LENGTH, body.len().to_string())
                .extension(who)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Gateway order: attach_caller (outer) thendev_admin (inner) then /silo nest.
    let app = Router::new()
        .nest("/silo", silo_router)
        .layer(middleware::from_fn(dev_admin))
        .layer(middleware::from_fn_with_state(
            keys.verifier(),
            gate::attach_caller,
        ));

    let payload = b"hello-dev-sigv4";
    let hash = sha256_hex(payload);
    let date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let path = "/silo/widgets/obj.txt";
    let headers = vec![
        ("host".into(), "silo.test".into()),
        ("x-amz-content-sha256".into(), hash.clone()),
        ("x-amz-date".into(), date.clone()),
        ("content-length".into(), payload.len().to_string()),
    ];
    let sig = sigv4::sign_request(
        "PUT",
        path,
        "",
        &headers,
        &hash,
        &created.access_key.access_key_id,
        created.secret_access_key.as_bytes(),
        &region,
        &date,
    );
    let date_stamp: String = date.chars().take(8).collect();
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={}/{date_stamp}/{region}/s3/aws4_request, SignedHeaders=content-length;host;x-amz-content-sha256;x-amz-date, Signature={sig}",
        created.access_key.access_key_id
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("host", "silo.test")
                .header("x-amz-content-sha256", &hash)
                .header("x-amz-date", &date)
                .header(header::CONTENT_LENGTH, payload.len().to_string())
                .header(header::AUTHORIZATION, auth)
                .body(Body::from(payload.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&to_bytes(resp.into_body(), 4096).await.unwrap())
    );

    // Presigned GET also works under thedev stack (no synthetic caller).
    let qs = sigv4::presign_query(
        "GET",
        path,
        "silo.test",
        &created.access_key.access_key_id,
        created.secret_access_key.as_bytes(),
        &region,
        &Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
        3600,
        "",
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("{path}?{qs}"))
                .header("host", "silo.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let got = to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(&got[..], payload);
}

#[tokio::test]
async fn without_skip_dev_admin_would_conflict() {
    // Sanity: ifdev_admin injects over SigV4, silo returns InvalidRequest.
    let dir = tempfile::tempdir().unwrap();
    let (events, _) = tokio::sync::broadcast::channel(16);
    let store = Store::open(dir.path().join("state.redb"), events).unwrap();
    let keep = keep::Client::open(&store).unwrap();
    let project = project(&store, "p2");
    let alice = principal(&store, "alice2");
    bind(&store, &alice, "member", &project);
    let created = keep.create_access_key(&project, &alice, "").unwrap();
    let silo = Client::new(store, dir.path().join("silo")).with_max_object_bytes(1024);
    let region = DEFAULT_REGION.to_owned();
    let app = http::router_with(silo, Some(keep), region.clone());

    let date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let path = "/widgets/x.txt";
    let hash = EMPTY_PAYLOAD_SHA256;
    // Need a bucket — create with extension, then conflict on SigV4+Caller.
    let who = Caller {
        principal: alice,
        principal_name: "alice2".into(),
        project: Some(project),
        roles: vec!["member".into()],
        system: false,
    };
    let body = serde_json::json!({"name": "widgets", "spec": {}}).to_string();
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/buckets")
                .header(header::CONTENT_TYPE, "application/json")
                .extension(who.clone())
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    let headers = vec![
        ("host".into(), "silo.test".into()),
        ("x-amz-content-sha256".into(), hash.to_owned()),
        ("x-amz-date".into(), date.clone()),
    ];
    let sig = sigv4::sign_request(
        "GET",
        path,
        "",
        &headers,
        hash,
        &created.access_key.access_key_id,
        created.secret_access_key.as_bytes(),
        &region,
        &date,
    );
    let date_stamp: String = date.chars().take(8).collect();
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={}/{date_stamp}/{region}/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={sig}",
        created.access_key.access_key_id
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("host", "silo.test")
                .header("x-amz-content-sha256", hash)
                .header("x-amz-date", &date)
                .header(header::AUTHORIZATION, auth)
                .extension(Caller {
                    principal: Id::new(kind::PRINCIPAL),
                    principal_name: "dev-admin".into(),
                    project: None,
                    roles: vec!["admin".into()],
                    system: true,
                })
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("InvalidRequest"),
        "{}",
        String::from_utf8_lossy(&body)
    );
}
