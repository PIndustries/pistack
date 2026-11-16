// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Key pairs through the router: import, generate, refuse, project
//! isolation, and what a machine created with one ends up holding.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use forge::{DriverRegistry, Forge, Keypair, Machine};
use http_body_util::BodyExt;
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Meta, Module, ModuleContext, Page};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct Harness {
    app: Router,
    store: Store,
    shutdown: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// A control plane with sizes and no hosts. Requests need a caller.
async fn harness() -> Harness {
    let store = Store::in_memory().unwrap();
    let shutdown = CancellationToken::new();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: shutdown.clone(),
        public_url: "http://test".into(),
        node_name: "control".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let forge = Forge::with_registry(Arc::new(DriverRegistry::new()));
    let app = forge.router(ctx.clone());
    Forge::seed_sizes(&store).unwrap();
    Harness {
        app,
        store,
        shutdown,
    }
}

/// A project row, so a machine create has something to count against.
fn project(store: &Store, name: &str) -> Id {
    let p = store
        .collection::<gate::Project>()
        .create(gate::Project {
            meta: Meta::new(kind::PROJECT, name, None),
            spec: gate::ProjectSpec::default(),
            status: gate::ProjectStatus::default(),
        })
        .unwrap();
    p.meta.id
}

fn caller(project: Id, role: &str) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "user".into(),
        project: Some(project),
        roles: vec![role.into()],
        system: false,
    }
}

async fn raw(
    app: &Router,
    caller: &Caller,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let mut req = req.body(body).unwrap();
    req.extensions_mut().insert(caller.clone());
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{method} {path}: {status} bad body {e}"))
    };
    (status, value)
}

/// The body as `T`, with the status, panicking with the message the server
/// sent when the two do not fit.
async fn call<T: DeserializeOwned>(
    app: &Router,
    caller: &Caller,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, T) {
    let (status, value) = raw(app, caller, method, path, body).await;
    let v: T = match serde_json::from_value(value.clone()) {
        Ok(v) => v,
        Err(e) => panic!(
            "{method} {path}: {status} is not a {}: {e}: {value}",
            std::any::type_name::<T>()
        ),
    };
    (status, v)
}

/// `PUT` with the current revision, as `pi apply` sends it.
async fn put(
    app: &Router,
    caller: &Caller,
    path: &str,
    revision: u64,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("PUT")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::IF_MATCH, revision.to_string())
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let mut req = req;
    req.extensions_mut().insert(caller.clone());
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        },
    )
}

/// An `ed25519` key `ssh-keygen` would print, with a comment.
const ED25519: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG6NiWNsbUCDY1DqQxFpj7Y1p+Ph+ovvRShuoREjxj/7 demo@box";
/// The same key without its comment.
const ED25519_NO_COMMENT: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG6NiWNsbUCDY1DqQxFpj7Y1p+Ph+ovvRShuoREjxj/7";
/// An `ssh-rsa` key of 2048 bits.
const RSA_2048: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDbH3es99nfYiFAEl5Y1/ELuDLikLHwsbhK66TT6G8Aoni+4muZExBBqu8h5FqzwY1ukR83nDb8gw2dvQRAg9R54D2VdWfNN3EEOzTxs76PfT0zODwCIB/8IrZ0ZHnvn/T4C8UMBgiw2VeVBTI3qBspzu5uaCquBDdxNXNPBKmFV6qVGHxMiVkEGRKYPYNBJUTvnK8EGC+GDkR1uNYszmJvXTPGujwRCY/qxHEzcIEg7y8A5keNO5v6dwDJp34M5eoF58OhNVr67BeCcTlVbIw2P5sXthBemymtghWcNrHEg696g6vYZ2QJ/ecUGyMULBUUPt8zaNhuRnsAioyR7zE7 rsa@box";
/// An `ssh-rsa` key of 1024 bits, which is too small.
const RSA_1024: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAgADAAQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyAhIiMkJSYnKCkqKywtLi8wMTIzNDU2Nzg5Ojs8PT4/QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl9gYWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXp7fH1+ small@box";

#[tokio::test]
async fn import_reports_the_fingerprint_and_type() {
    let h = harness().await;
    let me = caller(Id::new(kind::PROJECT), "member");

    let (st, kp) = call::<Keypair>(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "laptop", "public_key": ED25519})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(kp.spec.key_type, "ssh-ed25519");
    assert_eq!(
        kp.spec.fingerprint,
        "SHA256:kZaHsQEOwFK+9RD5E9L3VAB8FaS9MZYfNAMVbxg0f1I"
    );
    // The line is kept as one line, comment and all.
    assert_eq!(kp.spec.public_key, ED25519);
    // Nothing is generated for an import.
    assert!(kp.spec.public_key.starts_with("ssh-ed25519 "));

    // Two imports of the same key are two names for one key.
    let (st, other) = call::<Keypair>(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "laptop-2", "public_key": ED25519})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(other.spec.fingerprint, kp.spec.fingerprint);

    // A 2048-bit rsa key is accepted and named for what it is.
    let (st, rsa) = call::<Keypair>(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "old", "public_key": RSA_2048})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(rsa.spec.key_type, "ssh-rsa");
    assert_eq!(rsa.spec.public_key, RSA_2048);

    // By name and by id, and in a list.
    let id = kp.meta.id.to_string();
    let (st, again) = call::<Keypair>(&h.app, &me, "GET", "/keypairs/laptop", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(again.meta.id, kp.meta.id);
    let (_, by_id) = call::<Keypair>(&h.app, &me, "GET", &format!("/keypairs/{id}"), None).await;
    assert_eq!(by_id.spec.fingerprint, kp.spec.fingerprint);
    let (_, page) = call::<Page<Keypair>>(&h.app, &me, "GET", "/keypairs", None).await;
    assert_eq!(page.total, 3);
}

#[tokio::test]
async fn generate_returns_the_private_key_once() {
    let h = harness().await;
    let me = caller(Id::new(kind::PROJECT), "member");

    let (st, body) = raw(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "made"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let private = body["private_key"].as_str().expect("private key present");
    assert!(private.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
    assert!(private
        .trim_end()
        .ends_with("-----END OPENSSH PRIVATE KEY-----"));
    let public = body["spec"]["public_key"].as_str().unwrap();
    assert!(public.starts_with("ssh-ed25519 "));
    // The name travels into the key's comment.
    assert!(public.ends_with(" made"), "{public}");
    assert_eq!(body["spec"]["key_type"], "ssh-ed25519");
    assert!(body["spec"]["fingerprint"]
        .as_str()
        .unwrap()
        .starts_with("SHA256:"));

    // Two calls make two different pairs.
    let (_, second) = raw(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "made2"})),
    )
    .await;
    assert_ne!(
        second["private_key"].as_str().unwrap(),
        private,
        "every pair is new"
    );

    // The private half is nowhere in what the API gives back afterwards, nor
    // in what is stored.
    let id = body["id"].as_str().unwrap().to_owned();
    for path in ["/keypairs", &format!("/keypairs/{id}"), "/keypairs/made"] {
        let (st, shown) = raw(&h.app, &me, "GET", path, None).await;
        assert_eq!(st, StatusCode::OK);
        let text = shown.to_string();
        assert!(!text.contains("BEGIN OPENSSH PRIVATE KEY"), "{path}");
        assert!(!text.contains(&private[40..80]), "{path} leaked the key");
    }
    let stored = h.store.collection::<Keypair>().get(&kp_id(&id)).unwrap();
    assert!(!stored.spec.public_key.contains("PRIVATE KEY"));
}

fn kp_id(id: &str) -> pistack_types::Id {
    pistack_types::Id::parse(forge::KEYPAIR_KIND, id).expect("a keypair id")
}

#[tokio::test]
async fn bad_keys_are_refused() {
    let h = harness().await;
    let me = caller(Id::new(kind::PROJECT), "member");
    for (what, key) in [
        ("not a key at all", "hello"),
        ("no comment", ED25519_NO_COMMENT),
        ("a key type that is not one", "ssh-nope AAAAgADAAQID x"),
        (
            "two lines",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG6N\nAAAA demo@box",
        ),
        ("too small an rsa key", RSA_1024),
        ("truncated body", "ssh-ed25519 AAAA demo@box"),
        (
            "an empty key",
            "ssh-ed25519 AAAAaW52YWxpZC1iYXNlNjQh demo@box",
        ),
    ] {
        let (st, body) = raw(
            &h.app,
            &me,
            "POST",
            "/keypairs",
            Some(json!({"name": "k", "public_key": key})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{what} was accepted: {body}");
        assert!(body["error"]["message"].is_string(), "{what}: no message");
    }
    // Nothing was stored by any of them.
    let (_, page) = call::<Page<Keypair>>(&h.app, &me, "GET", "/keypairs", None).await;
    assert_eq!(page.total, 0);
}

#[tokio::test]
async fn a_project_sees_only_its_own_pairs() {
    let h = harness().await;
    let a = caller(Id::new(kind::PROJECT), "member");
    let b = caller(Id::new(kind::PROJECT), "member");

    let (st, _) = raw(
        &h.app,
        &a,
        "POST",
        "/keypairs",
        Some(json!({"name": "shared", "public_key": ED25519})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // B's list is empty and A's key is not B's, by name or by id.
    let (_, page) = call::<Page<Keypair>>(&h.app, &b, "GET", "/keypairs", None).await;
    assert_eq!(page.total, 0);
    let (st, _) = raw(&h.app, &b, "GET", "/keypairs/shared", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // The same name in another project is a different pair.
    let (st, _) = raw(
        &h.app,
        &b,
        "POST",
        "/keypairs",
        Some(json!({"name": "shared", "public_key": ED25519})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "names are per project");

    // A caller with no project cannot hold one.
    let nobody = Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "root".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    };
    let (st, body) = raw(
        &h.app,
        &nobody,
        "POST",
        "/keypairs",
        Some(json!({"name": "orphan"})),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");

    // A reader may list but not write.
    let reader = caller(Id::new(kind::PROJECT), "reader");
    let (st, _) = raw(&h.app, &reader, "GET", "/keypairs", None).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn a_machine_takes_the_key_of_its_pair() {
    let h = harness().await;
    let me = caller(project(&h.store, "alpha"), "member");

    let (st, _) = raw(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "laptop", "public_key": ED25519})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let create = |extra: Value| {
        let mut spec = json!({"size": "s.1x1", "image": "ubuntu"});
        for (k, v) in extra.as_object().unwrap() {
            spec[k] = v.clone();
        }
        spec
    };

    // A pair by name, beside a raw key: the machine gets both.
    let (st, m) = call::<Machine>(
        &h.app,
        &me,
        "POST",
        "/machines",
        Some(json!({
            "name": "web",
            "spec": create(json!({
                "keypair": "laptop",
                "ssh_keys": ["ssh-ed25519 AAAAbox someone"],
            })),
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{m:?}");
    assert_eq!(m.spec.ssh_keys.len(), 2);
    assert!(m.spec.ssh_keys.contains(&ED25519.to_owned()));
    // The name is kept on the spec, so the machine can say what it was made
    // with.
    assert_eq!(m.spec.keypair.as_deref(), Some("laptop"));

    // Raw keys keep working on their own.
    let (_, plain) = call::<Machine>(
        &h.app,
        &me,
        "POST",
        "/machines",
        Some(json!({"name": "plain", "spec": create(json!({}))})),
    )
    .await;
    assert!(plain.spec.ssh_keys.is_empty());

    // The same key twice arrives once.
    let (_, dup) = call::<Machine>(
        &h.app,
        &me,
        "POST",
        "/machines",
        Some(json!({
            "name": "dup",
            "spec": create(json!({"keypair": "laptop", "ssh_keys": [ED25519]})),
        })),
    )
    .await;
    assert_eq!(dup.spec.ssh_keys.len(), 1);

    // A pair from another project is not a pair at all.
    let (st, body) = raw(
        &h.app,
        &caller(project(&h.store, "beta"), "member"),
        "POST",
        "/machines",
        Some(json!({"name": "x", "spec": create(json!({"keypair": "laptop"}))})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("laptop"));

    // Delete the pair: what was made with it still carries the key, and the
    // name it was made with.
    let (st, _) = raw(&h.app, &me, "DELETE", "/keypairs/laptop", None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, still) = call::<Machine>(&h.app, &me, "GET", "/machines/web", None).await;
    assert!(still.spec.ssh_keys.contains(&ED25519.to_owned()));
    assert_eq!(still.spec.keypair.as_deref(), Some("laptop"));
    // And it is gone.
    let (st, _) = raw(&h.app, &me, "GET", "/keypairs/laptop", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = raw(&h.app, &me, "DELETE", "/keypairs/laptop", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_read_back_machine_names_its_pair() {
    let h = harness().await;
    let me = caller(project(&h.store, "alpha"), "member");
    let (st, _) = raw(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "laptop", "public_key": ED25519})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, m) = call::<Machine>(
        &h.app,
        &me,
        "POST",
        "/machines",
        Some(json!({
            "name": "web",
            "spec": {"size": "s.1x1", "image": "ubuntu", "keypair": "laptop"},
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{m:?}");

    // What the create returned and what a later read returns both carry the
    // name, and the raw JSON shows it too: this is what `pi machine show`
    // reads.
    let (_, got) = call::<Machine>(&h.app, &me, "GET", "/machines/web", None).await;
    assert_eq!(got.spec.keypair.as_deref(), Some("laptop"));
    let (_, raw_body) = raw(&h.app, &me, "GET", "/machines/web", None).await;
    assert_eq!(raw_body["spec"]["keypair"], "laptop");

    // A machine made without a pair has no field at all, not an empty one.
    let (_, plain) = call::<Machine>(
        &h.app,
        &me,
        "POST",
        "/machines",
        Some(json!({"name": "plain", "spec": {"size": "s.1x1", "image": "ubuntu"}})),
    )
    .await;
    assert_eq!(plain.spec.keypair, None);
    let (_, plain_body) = raw(&h.app, &me, "GET", "/machines/plain", None).await;
    assert!(plain_body["spec"].get("keypair").is_none());
}

#[tokio::test]
async fn a_machine_keeps_its_pair_through_an_update() {
    let h = harness().await;
    let me = caller(project(&h.store, "alpha"), "member");
    let (st, kp) = call::<Keypair>(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "laptop", "public_key": ED25519})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{kp:?}");
    let (st, m) = call::<Machine>(
        &h.app,
        &me,
        "POST",
        "/machines",
        Some(json!({
            "name": "web",
            "spec": {"size": "s.1x1", "image": "ubuntu", "keypair": "laptop"},
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{m:?}");
    let spec = |keypair: Value| {
        json!({
            "spec": {
                "size": "s.1x1",
                "image": "ubuntu",
                "ssh_keys": m.spec.ssh_keys,
                "keypair": keypair,
            },
        })
    };

    // What a read returns may be put back unchanged, by name or by id.
    let mut rev = m.meta.revision;
    for spelling in [json!("laptop"), json!(kp.meta.id)] {
        let (st, body) = put(&h.app, &me, "/machines/web", rev, spec(spelling)).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["spec"]["keypair"], "laptop");
        rev = body["revision"].as_u64().expect("a revision");
    }

    // Another pair, or none at all, is a change and is refused.
    let (st, _) = call::<Keypair>(
        &h.app,
        &me,
        "POST",
        "/keypairs",
        Some(json!({"name": "desk", "public_key": RSA_2048})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, body) = put(&h.app, &me, "/machines/web", rev, spec(json!("desk"))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("keypair"));

    // A machine with no pair keeps having none, even if asked to gain one.
    let (st, plain) = call::<Machine>(
        &h.app,
        &me,
        "POST",
        "/machines",
        Some(json!({"name": "plain", "spec": {"size": "s.1x1", "image": "ubuntu"}})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, body) = put(
        &h.app,
        &me,
        "/machines/plain",
        plain.meta.revision,
        json!({"spec": {"size": "s.1x1", "image": "ubuntu", "keypair": "laptop"}}),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

#[tokio::test]
async fn the_index_lists_keypairs() {
    let h = harness().await;
    let me = caller(Id::new(kind::PROJECT), "member");
    let (st, body) = raw(&h.app, &me, "GET", "/", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body["resources"]
            .as_array()
            .unwrap()
            .contains(&json!("keypairs")),
        "{body}"
    );
}
