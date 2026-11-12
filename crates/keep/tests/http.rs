// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end through the router: boot on an in-memory store with a fake
//! auth layer, then walk secrets, certificates, CAs and join tokens.

use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use keep::{redeem_join_token, Keep};
use pistack_store::Store;
use pistack_types::module::CancellationToken;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// Fake gateway auth: `X-Test-Caller: admin|member:<proj>|reader:<proj>|member-other:<proj>`,
/// or `as:<prin>:<role>:<proj>` for a fixed principal.
async fn fake_auth(mut req: Request, next: Next) -> Response {
    let hdr = req
        .headers()
        .get("x-test-caller")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if let Some(rest) = hdr.as_deref().and_then(|h| h.strip_prefix("as:")) {
        let mut it = rest.splitn(3, ':');
        let (prin, role, proj) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        req.extensions_mut().insert(Caller {
            principal: prin.parse_id(),
            principal_name: role.into(),
            project: Some(proj.parse_id()),
            roles: vec![role.into()],
            system: false,
        });
        return next.run(req).await;
    }
    if let Some(h) = hdr {
        let (role, proj) = h.split_once(':').unwrap_or((h.as_str(), ""));
        let project = (!proj.is_empty()).then(|| proj.parse_id());
        let caller = match role {
            "admin" => Caller {
                principal: Id::new(kind::PRINCIPAL),
                principal_name: "admin".into(),
                project: None,
                roles: vec!["admin".into()],
                system: true,
            },
            "padmin" => Caller {
                principal: Id::new(kind::PRINCIPAL),
                principal_name: "padmin".into(),
                project,
                roles: vec!["admin".into()],
                system: false,
            },
            "member" => Caller {
                principal: Id::new(kind::PRINCIPAL),
                principal_name: "bob".into(),
                project,
                roles: vec!["member".into()],
                system: false,
            },
            _ => Caller {
                principal: Id::new(kind::PRINCIPAL),
                principal_name: "eve".into(),
                project,
                roles: vec!["reader".into()],
                system: false,
            },
        };
        req.extensions_mut().insert(caller);
    }
    next.run(req).await
}

trait ParseId {
    fn parse_id(&self) -> Id;
}
impl ParseId for str {
    fn parse_id(&self) -> Id {
        serde_json::from_value(Value::String(self.to_owned())).unwrap()
    }
}

async fn boot() -> (Router, Store) {
    let store = Store::in_memory().unwrap();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let keep = Keep::default();
    keep.start(ctx.clone()).await.unwrap();
    assert_eq!(keep.info().prefix, "secrets");
    assert!(keep.client().is_some());
    let app = keep.router(ctx).layer(middleware::from_fn(fake_auth));
    (app, store)
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    who: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call_h(app, method, path, who, body, &[]).await
}

async fn call_h(
    app: &Router,
    method: &str,
    path: &str,
    who: Option<&str>,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(w) = who {
        req = req.header("x-test-caller", w);
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else if ct.contains("json") {
        serde_json::from_slice(&bytes).unwrap()
    } else {
        Value::String(String::from_utf8(bytes.to_vec()).unwrap())
    };
    (status, json)
}

#[tokio::test]
async fn secret_lifecycle_over_http() {
    let (app, _store) = boot().await;
    let proj = Id::new(kind::PROJECT).to_string();
    let member = format!("member:{proj}");
    let reader = format!("reader:{proj}");

    // No credentials: 401.
    let (st, _) = call(&app, "GET", "/secrets", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Reader cannot create.
    let (st, _) = call(
        &app,
        "POST",
        "/secrets",
        Some(&reader),
        Some(json!({"name": "db", "payload": "x"})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Member creates.
    let (st, s) = call(
        &app,
        "POST",
        "/secrets",
        Some(&member),
        Some(json!({
            "name": "db-password", "kind": "password", "payload": "hunter2",
            "labels": {"tier": "db"}, "description": "prod db"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{s}");
    assert_eq!(s["status"]["version"], 1);
    assert_eq!(s["status"]["phase"], "ready");
    assert_eq!(s["spec"]["content_type"], "text/plain");
    assert_eq!(s["project"], proj);
    assert!(s.get("payload").is_none());
    let id = s["id"].as_str().unwrap().to_owned();
    let rev = s["revision"].as_u64().unwrap();

    // Payload hidden in list and get.
    let (st, page) = call(&app, "GET", "/secrets", Some(&member), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page["total"], 1);
    let text = page.to_string();
    assert!(!text.contains("hunter2"));
    assert!(!text.contains("ciphertext"));
    assert!(!text.contains("sealed"));
    let (st, got) = call(&app, "GET", "/secrets/db-password", Some(&reader), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!got.to_string().contains("hunter2"));

    // Reader cannot fetch the payload; member can.
    let (st, _) = call(
        &app,
        "GET",
        &format!("/secrets/{id}/payload"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, p) = call(
        &app,
        "GET",
        &format!("/secrets/{id}/payload"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p["encoding"], "base64");
    assert_eq!(p["version"], 1);
    let decoded = base64_decode(p["payload"].as_str().unwrap());
    assert_eq!(decoded, b"hunter2");

    // Another project sees nothing.
    let other = format!("member:{}", Id::new(kind::PROJECT));
    let (st, page) = call(&app, "GET", "/secrets", Some(&other), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page["total"], 0);
    let (st, _) = call(&app, "GET", &format!("/secrets/{id}"), Some(&other), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Rotate with a stale If-Match: conflict. Then rotate properly: version 2.
    let (st, _) = call_h(
        &app,
        "PUT",
        &format!("/secrets/{id}/payload"),
        Some(&member),
        Some(json!({"payload": "aHVudGVyMw==", "encoding": "base64"})),
        &[("if-match", "99")],
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, s2) = call_h(
        &app,
        "PUT",
        &format!("/secrets/{id}/payload"),
        Some(&member),
        Some(json!({"payload": "aHVudGVyMw==", "encoding": "base64"})),
        &[("if-match", &rev.to_string())],
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{s2}");
    assert_eq!(s2["status"]["version"], 2);
    assert!(s2["status"]["last_rotated"].is_string());
    let (_, p) = call(
        &app,
        "GET",
        &format!("/secrets/{id}/payload"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(base64_decode(p["payload"].as_str().unwrap()), b"hunter3");
    assert_eq!(p["version"], 2);

    // Metadata update keeps payload and version.
    let (st, s3) = call(
        &app,
        "PUT",
        &format!("/secrets/{id}"),
        Some(&member),
        Some(json!({"description": "renamed", "rotate_after_days": 30})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s3["description"], "renamed");
    assert_eq!(s3["spec"]["rotate_after_days"], 30);
    assert_eq!(s3["status"]["version"], 2);
    let (_, p) = call(
        &app,
        "GET",
        &format!("/secrets/{id}/payload"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(base64_decode(p["payload"].as_str().unwrap()), b"hunter3");

    // Admin sees it in the cloud-wide list (plus nothing else: CA keys are
    // cloud-scoped secrets and are listed for admins).
    let (st, page) = call(&app, "GET", "/secrets?labels=tier=db", Some("admin"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page["total"], 1);

    // Delete.
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/secrets/{id}"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/secrets/{id}"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&app, "GET", &format!("/secrets/{id}"), Some(&member), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Bad names and empty payloads are rejected.
    let (st, _) = call(
        &app,
        "POST",
        "/secrets",
        Some(&member),
        Some(json!({"name": "bad name!", "payload": "x"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &app,
        "POST",
        "/secrets",
        Some(&member),
        Some(json!({"name": "empty", "payload": ""})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn certificates_and_cas_over_http() {
    let (app, _store) = boot().await;
    let proj = Id::new(kind::PROJECT).to_string();
    let member = format!("member:{proj}");
    let padmin = format!("padmin:{proj}");

    // CAs exist and are admin only.
    let (st, _) = call(&app, "GET", "/cas", Some(&member), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, cas) = call(&app, "GET", "/cas", Some("admin"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cas["total"], 2);
    let names: Vec<&str> = cas["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"pistack-root") && names.contains(&"pistack-issuing"));
    // Nothing in the CA listing leaks a key.
    assert!(!cas.to_string().contains("PRIVATE KEY"));

    // Anyone authenticated can fetch the CA certificate and CRL.
    let (st, pem) = call(
        &app,
        "GET",
        "/cas/pistack-issuing/certificate",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(pem
        .as_str()
        .unwrap()
        .starts_with("-----BEGIN CERTIFICATE-----"));
    let (st, crl) = call(&app, "GET", "/cas/pistack-issuing/crl", Some(&member), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(crl["serials"], json!([]));

    // Issue via the API.
    let (st, cert) = call(
        &app,
        "POST",
        "/certificates",
        Some(&member),
        Some(json!({
            "name": "web", "common_name": "web.example", "sans": ["web.example", "10.0.0.5"],
            "usage": "both", "ttl_days": 90
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{cert}");
    assert_eq!(cert["status"]["phase"], "ready");
    assert!(cert["status"]["cert_pem"]
        .as_str()
        .unwrap()
        .contains("BEGIN CERTIFICATE"));
    assert_eq!(
        cert["status"]["fingerprint_sha256"].as_str().unwrap().len(),
        64
    );
    assert!(cert["status"]["serial"].as_u64().unwrap() >= 3);
    assert!(cert["status"]["key_secret"].is_string());
    assert!(!cert.to_string().contains("PRIVATE KEY"));
    let id = cert["id"].as_str().unwrap().to_owned();
    let serial = cert["status"]["serial"].as_u64().unwrap();
    let key_secret = cert["status"]["key_secret"].as_str().unwrap().to_owned();

    // Members may not choose the issuer, nor read the key; project admins may.
    let (st, _) = call(
        &app,
        "POST",
        "/certificates",
        Some(&member),
        Some(json!({"name": "x", "common_name": "x", "issuer": cas["items"][0]["id"]})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(
        &app,
        "GET",
        &format!("/certificates/{id}/key"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, key) = call(&app, "GET", "/certificates/web/key", Some(&padmin), None).await;
    assert_eq!(st, StatusCode::OK, "{key}");
    // Admin in one project is not admin of the cloud.
    let (st, _) = call(&app, "GET", "/cas", Some(&padmin), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(
        &app,
        "POST",
        "/join-tokens",
        Some(&padmin),
        Some(json!({"name": "rogue"})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(key["key_pem"].as_str().unwrap().contains("PRIVATE KEY"));
    assert_eq!(key["cert_pem"], cert["status"]["cert_pem"]);
    assert_eq!(
        key["ca_pem"]
            .as_str()
            .unwrap()
            .matches("BEGIN CERTIFICATE")
            .count(),
        2
    );

    // The key secret is visible as metadata but cannot be deleted directly.
    let (st, s) = call(
        &app,
        "GET",
        &format!("/secrets/{key_secret}"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s["spec"]["kind"], "certificate");
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/secrets/{key_secret}"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Listing is project scoped.
    let (st, page) = call(&app, "GET", "/certificates", Some(&member), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page["total"], 1);
    let other = format!("member:{}", Id::new(kind::PROJECT));
    let (_, page) = call(&app, "GET", "/certificates", Some(&other), None).await;
    assert_eq!(page["total"], 0);

    // Revoke: shows on the CRL and on the resource.
    let (st, revoked) = call(
        &app,
        "POST",
        &format!("/certificates/{id}/revoke"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(revoked["status"]["revoked"], true);
    let (_, crl) = call(&app, "GET", "/cas/pistack-issuing/crl", Some(&member), None).await;
    assert_eq!(crl["serials"], json!([serial]));

    // Revoke by serial on the CA (admin), idempotent.
    let (st, _) = call(
        &app,
        "POST",
        "/cas/pistack-issuing/revoke",
        Some(&member),
        Some(json!({"serial": serial})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, ca) = call(
        &app,
        "POST",
        "/cas/pistack-issuing/revoke",
        Some("admin"),
        Some(json!({"serial": serial})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ca["status"]["revoked"].as_array().unwrap().len(), 1);

    // Delete takes the key secret with it.
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/certificates/{id}"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(
        &app,
        "GET",
        &format!("/secrets/{key_secret}"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn join_tokens_over_http() {
    let (app, store) = boot().await;
    let member = format!("member:{}", Id::new(kind::PROJECT));

    let (st, _) = call(
        &app,
        "POST",
        "/join-tokens",
        Some(&member),
        Some(json!({"name": "rack-a"})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, t) = call(
        &app,
        "POST",
        "/join-tokens",
        Some("admin"),
        Some(json!({"name": "rack-a"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{t}");
    let clear = t["token"].as_str().unwrap().to_owned();
    assert!(clear.starts_with("pij_"));
    assert_eq!(t["join_token"]["spec"]["multi_use"], false);
    assert_eq!(t["join_token"]["spec"]["created_by"], "admin");
    assert_ne!(t["join_token"]["spec"]["hash"], clear);

    let (st, page) = call(&app, "GET", "/join-tokens", Some("admin"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page["total"], 1);
    assert!(!page.to_string().contains(&clear));

    // Redeem over HTTP without credentials (that is the point), then again: refused.
    let (st, grant) = call(
        &app,
        "POST",
        "/join-tokens/redeem",
        None,
        Some(json!({"token": clear})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{grant}");
    assert_eq!(grant["token_name"], "rack-a");
    assert!(!grant.to_string().contains(&clear));
    let (st, got) = call(&app, "GET", "/join-tokens/rack-a", Some("admin"), None).await;
    assert_eq!(st, StatusCode::OK, "{got}");
    assert_eq!(got["status"]["uses"], 1);
    assert!(!got.to_string().contains(&clear));
    let (st, denied) = call(
        &app,
        "POST",
        "/join-tokens/redeem",
        None,
        Some(json!({"token": clear})),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(denied["error"]["message"], "join token already used");
    assert!(!denied.to_string().contains(&clear));
    let (st, got) = call(&app, "GET", "/join-tokens/rack-a", Some("admin"), None).await;
    assert_eq!(st, StatusCode::OK, "{got}");
    assert_eq!(got["status"]["uses"], 1);

    // Multi-use token through the library entry point, with a short ttl.
    let (st, t) = call(
        &app,
        "POST",
        "/join-tokens",
        Some("admin"),
        Some(json!({"name": "fleet", "multi_use": true, "ttl_secs": 600})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let clear = t["token"].as_str().unwrap();
    redeem_join_token(&store, clear).unwrap();
    redeem_join_token(&store, clear).unwrap();
    let (st, got) = call(&app, "GET", "/join-tokens/fleet", Some("admin"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["status"]["uses"], 2);
    let (st, _) = call(&app, "DELETE", "/join-tokens/fleet", Some("admin"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(redeem_join_token(&store, clear).is_err());

    // Too long a ttl is rejected.
    let (st, _) = call(
        &app,
        "POST",
        "/join-tokens",
        Some("admin"),
        Some(json!({"name": "long", "ttl_secs": 999999})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

#[tokio::test]
async fn limits_over_http() {
    let (app, store) = boot().await;
    // A gate project whose quota allows one secret and one certificate.
    let mut spec = gate::ProjectSpec::default();
    spec.quotas.insert("secrets".into(), 1);
    spec.quotas.insert("certificates".into(), 1);
    let proj = store
        .collection::<gate::Project>()
        .create(gate::Project {
            meta: pistack_types::Meta::new(kind::PROJECT, "tight", None),
            spec,
            status: Default::default(),
        })
        .unwrap()
        .meta
        .id
        .to_string();
    let member = format!("member:{proj}");

    let secret = |name: &str| json!({"name": name, "payload": "x"});
    let (st, _) = call(&app, "POST", "/secrets", Some(&member), Some(secret("a"))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, e) = call(&app, "POST", "/secrets", Some(&member), Some(secret("b"))).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(e["error"]["code"], "quota_exceeded", "{e}");
    assert_eq!(e["error"]["details"]["resource"], "secrets", "{e}");
    assert_eq!(e["error"]["details"]["limit"], 1, "{e}");

    let cert = |name: &str| json!({"name": name, "common_name": name});
    let (st, _) = call(
        &app,
        "POST",
        "/certificates",
        Some(&member),
        Some(cert("w1")),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, e) = call(
        &app,
        "POST",
        "/certificates",
        Some(&member),
        Some(cert("w2")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(e["error"]["details"]["resource"], "certificates", "{e}");
    // Usage is visible on the project.
    let p = store
        .collection::<gate::Project>()
        .get(&serde_json::from_value(Value::String(proj.clone())).unwrap())
        .unwrap();
    assert_eq!(p.status.usage.get("secrets"), Some(&1));
    assert_eq!(p.status.usage.get("certificates"), Some(&1));

    // Certificate metadata has the secret limits.
    let (st, e) = call(
        &app,
        "POST",
        "/certificates",
        Some("admin"),
        Some(json!({"name": "big", "common_name": "big", "description": "d".repeat(4097)})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");

    // A multi-use join token above the cap is refused; at the cap it is not.
    let cap = keep::KeepConfig::configured().max_join_token_uses;
    let (st, e) = call(
        &app,
        "POST",
        "/join-tokens",
        Some("admin"),
        Some(json!({"name": "over", "multi_use": true, "max_uses": cap + 1})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");
    let (st, t) = call(
        &app,
        "POST",
        "/join-tokens",
        Some("admin"),
        Some(json!({"name": "at-cap", "multi_use": true, "max_uses": cap})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{t}");
    assert_eq!(t["join_token"]["spec"]["max_uses"], cap);
}

fn gate_principal(store: &Store, name: &str, project: &Id, role: &str) -> Id {
    let id = store
        .collection::<gate::Principal>()
        .create(gate::Principal {
            meta: pistack_types::Meta::new(kind::PRINCIPAL, name, None),
            spec: Default::default(),
            status: Default::default(),
        })
        .unwrap()
        .meta
        .id;
    store
        .collection::<gate::RoleBinding>()
        .create(gate::RoleBinding {
            meta: pistack_types::Meta::new(
                gate::KIND_BINDING,
                format!("{name}-{role}"),
                Some(project.clone()),
            ),
            spec: gate::RoleBindingSpec {
                principal: id.clone(),
                role: role.into(),
                scope: gate::Scope::Project {
                    project: project.clone(),
                },
            },
            status: Default::default(),
        })
        .unwrap();
    id
}

fn gate_project(store: &Store, name: &str) -> Id {
    store
        .collection::<gate::Project>()
        .create(gate::Project {
            meta: pistack_types::Meta::new(kind::PROJECT, name, None),
            spec: Default::default(),
            status: Default::default(),
        })
        .unwrap()
        .meta
        .id
}

#[tokio::test]
async fn access_keys_over_http() {
    let (app, store) = boot().await;
    let proj = gate_project(&store, "p1");
    let other = gate_project(&store, "p2");
    let alice = gate_principal(&store, "alice", &proj, "member");
    let bob = gate_principal(&store, "bob", &proj, "member");
    let carol = gate_principal(&store, "carol", &other, "member");
    let eve = gate_principal(&store, "eve", &proj, "reader");
    let as_ = |p: &Id, role: &str, pr: &Id| format!("as:{p}:{role}:{pr}");
    let alice_h = as_(&alice, "member", &proj);
    let carol_h = as_(&carol, "member", &other);

    // Create: the secret is in this response only.
    let (st, k) = call(
        &app,
        "POST",
        "/access-keys",
        Some(&alice_h),
        Some(json!({"description": "ci"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{k}");
    let akid = k["access_key_id"].as_str().unwrap().to_owned();
    let id = k["id"].as_str().unwrap().to_owned();
    let secret = k["secret_access_key"].as_str().unwrap().to_owned();
    assert!(akid.starts_with("PIAK") && akid.len() == 20, "{k}");
    assert_eq!(secret.len(), 40);
    assert_eq!(k["project"], proj.to_string());
    assert_eq!(k["principal"], alice.to_string());

    // List and get (by sec- id and by PIAK id) carry no secret.
    let (st, list) = call(&app, "GET", "/access-keys", Some(&alice_h), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert!(!list.to_string().contains(&secret));
    for key in [&id, &akid] {
        let (st, g) = call(
            &app,
            "GET",
            &format!("/access-keys/{key}"),
            Some(&alice_h),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{g}");
        assert_eq!(g["access_key_id"], akid.as_str());
        assert!(g.get("secret_access_key").is_none(), "{g}");
    }
    // The generic secret routes show it but never its payload.
    let (st, _) = call(&app, "GET", &format!("/secrets/{id}"), Some(&alice_h), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, e) = call(
        &app,
        "GET",
        &format!("/secrets/{id}/payload"),
        Some(&alice_h),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{e}");
    let (st, e) = call(
        &app,
        "PUT",
        &format!("/secrets/{id}/payload"),
        Some(&alice_h),
        Some(json!({"payload": "mine"})),
    )
    .await;
    assert!(st.is_client_error(), "{st} {e}");
    let (st, e) = call(
        &app,
        "DELETE",
        &format!("/secrets/{id}"),
        Some(&alice_h),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{e}");
    let (st, e) = call(
        &app,
        "PUT",
        &format!("/secrets/{id}"),
        Some(&alice_h),
        Some(json!({"labels": {}})),
    )
    .await;
    assert!(st.is_client_error(), "{st} {e}");
    let (st, e) = call(
        &app,
        "POST",
        "/secrets",
        Some(&alice_h),
        Some(json!({"name": "fake", "payload": "x",
                    "labels": {"keep.pistack.io/role": "access-key"}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");

    // Another project sees nothing and cannot revoke.
    let (_, list) = call(&app, "GET", "/access-keys", Some(&carol_h), None).await;
    assert!(list["items"].as_array().unwrap().is_empty(), "{list}");
    for key in [&id, &akid] {
        let path = format!("/access-keys/{key}");
        let (st, _) = call(&app, "GET", &path, Some(&carol_h), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = call(&app, "DELETE", &path, Some(&carol_h), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    // Nor create a key in it.
    let (st, _) = call(
        &app,
        "POST",
        "/access-keys",
        Some(&carol_h),
        Some(json!({"project": proj.to_string()})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // A reader cannot create or revoke; a member cannot mint for someone else.
    let eve_h = as_(&eve, "reader", &proj);
    let (st, _) = call(&app, "POST", "/access-keys", Some(&eve_h), Some(json!({}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/access-keys/{id}"),
        Some(&eve_h),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(
        &app,
        "POST",
        "/access-keys",
        Some(&alice_h),
        Some(json!({"principal": bob.to_string()})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // A project admin can, for a principal with a role in the project.
    let padmin = as_(&Id::new(kind::PRINCIPAL), "admin", &proj);
    let (st, b) = call(
        &app,
        "POST",
        "/access-keys",
        Some(&padmin),
        Some(json!({"principal": bob.to_string()})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    assert_eq!(b["principal"], bob.to_string());
    let (st, e) = call(
        &app,
        "POST",
        "/access-keys",
        Some(&padmin),
        Some(json!({"principal": carol.to_string()})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");
    // A cloud admin must say which project.
    let (st, _) = call(&app, "POST", "/access-keys", Some("admin"), Some(json!({}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, all) = call(&app, "GET", "/access-keys", Some("admin"), None).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 2, "{all}");

    // Revoke by PIAK id: gone everywhere, and lookup refuses it.
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/access-keys/{akid}"),
        Some(&alice_h),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(
        &app,
        "GET",
        &format!("/access-keys/{id}"),
        Some(&alice_h),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(
        &app,
        "GET",
        &format!("/access-keys/{akid}"),
        Some(&alice_h),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let client = keep::Client::open(&store).unwrap();
    assert!(matches!(
        client.lookup_access_key(&akid),
        Err(keep::KeepError::Unauthorized(_))
    ));
    let bob_key = client
        .lookup_access_key(b["access_key_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(bob_key.caller.principal, bob);
    assert_eq!(bob_key.caller.project, Some(proj));
}

#[tokio::test]
async fn start_sweeps_dead_join_tokens_on_the_leader() {
    let store = Store::in_memory().unwrap();
    let (dead, _) = keep::create_join_token(&store, "dead", "admin", false, None).unwrap();
    let (live, _) = keep::create_join_token(&store, "live", "admin", false, None).unwrap();
    let col = store.collection::<keep::JoinToken>();
    col.modify(&dead.meta.id, |t| {
        t.spec.expires_at = chrono::Utc::now() - chrono::Duration::days(30);
        Ok(())
    })
    .unwrap();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    Keep::default().start(ctx.clone()).await.unwrap();
    let mut gone = false;
    for _ in 0..100 {
        if col.try_get(&dead.meta.id).unwrap().is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        gone,
        "the leader's sweep should delete a token dead for 30 days"
    );
    assert!(col.try_get(&live.meta.id).unwrap().is_some());
    ctx.shutdown.cancel();
}
