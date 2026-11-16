// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Transfers over HTTP. The offer is made on the volume, taken by id under
//! `/transfers`, and the secret appears in exactly one response.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use gate::quota::VOLUMES_GIB;
use gate::{Project, ProjectSpec, ProjectStatus};
use pistack_types::{kind, Caller, Id, Meta, Phase};
use serde_json::{json, Value};
use slab::{AllowAll, Client, ProjectQuota, QuotaCheck};
use std::sync::Arc;
use tower::ServiceExt;

fn tenant(store: &pistack_store::Store, name: &str, limits: &[(&str, u64)]) -> Id {
    let mut spec = ProjectSpec::default();
    for (r, n) in limits {
        spec.quotas.insert((*r).to_string(), *n);
    }
    store
        .collection::<Project>()
        .create(Project {
            meta: Meta::new(kind::PROJECT, name, None),
            spec,
            status: ProjectStatus::default(),
        })
        .unwrap()
        .meta
        .id
}

fn app(dir: &std::path::Path, quota: Arc<dyn QuotaCheck>) -> (Client, Router) {
    let base = Client::local(dir).unwrap();
    let c = Client::new(base.store().clone(), base.backend().clone(), quota)
        .with_silo_root(base.silo_root().to_path_buf());
    c.seed_defaults().unwrap();
    let app = slab::http::router(c.clone());
    (c, app)
}

fn caller(project: Option<Id>, role: &str) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: format!(
            "{role}-at-{}",
            project.clone().map(|p| p.to_string()).unwrap_or_default()
        ),
        project,
        roles: vec![role.into()],
        system: false,
    }
}

async fn send(
    app: &Router,
    who: Caller,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = req.body(body).expect("request");
    req.extensions_mut().insert(who);
    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.expect("body");
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json")
    };
    (status, v)
}

async fn make_volume(app: &Router, who: &Caller, name: &str, gib: u64) -> String {
    let (st, vol) = send(
        app,
        who.clone(),
        "POST",
        "/volumes",
        Some(json!({ "name": name, "spec": { "size_gib": gib } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{vol}");
    vol["id"].as_str().expect("id").to_owned()
}

/// Offer `id`, take it as `joiner`, answer the transfer id and the accepted volume.
async fn move_volume(app: &Router, owner: &Caller, joiner: &Caller, id: &str) -> Value {
    let (_, offered) = send(
        app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();
    let secret = offered["secret"].as_str().unwrap().to_owned();
    let (st, moved) = send(
        app,
        joiner.clone(),
        "POST",
        &format!("/transfers/{tid}/accept"),
        Some(json!({ "secret": secret })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{moved}");
    moved
}

#[tokio::test]
async fn an_offer_is_made_on_the_volume_and_secret_shown_once() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let owner = caller(Some(giver.clone()), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let c_vol = c.get_volume(Some(&giver), &id).unwrap();
    c.reconcile_volume(&c_vol.meta.id).await.unwrap();

    let (st, offered) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{offered}");
    let tid = offered["transfer"]["id"].as_str().expect("id").to_owned();
    let secret = offered["secret"].as_str().expect("secret").to_owned();
    assert!(secret.starts_with("vxt_"), "{secret}");
    assert_eq!(offered["transfer"]["status"]["phase"], "pending");

    // Reading the transfer back, from either side, never gives the secret.
    let (st, read) = send(
        &app,
        owner.clone(),
        "GET",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{read}");
    assert!(!read.to_string().contains(&secret), "{read}");

    // The volume's own listing shows the offer without it either.
    let (st, listed) = send(
        &app,
        owner,
        "GET",
        &format!("/volumes/{id}/transfers"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{listed}");
    assert_eq!(listed["total"], 1, "{listed}");
    assert!(!listed.to_string().contains(&secret), "{listed}");

    // A reader may look but cannot offer.
    let reader = caller(Some(giver.clone()), "reader");
    let (st, _) = send(
        &app,
        reader,
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_member_of_another_project_accepts_with_id_and_secret() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let taker = tenant(c.store(), "taker", &[]);
    let owner = caller(Some(giver.clone()), "member");
    let joiner = caller(Some(taker.clone()), "member");

    let id = make_volume(&app, &owner, "data", 2).await;
    let vol = c.get_volume(Some(&giver), &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (_, offered) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();
    let secret = offered["secret"].as_str().unwrap().to_owned();

    // The taker cannot see the volume yet, and does not need to.
    let (st, _) = send(&app, joiner.clone(), "GET", &format!("/volumes/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, moved) = send(
        &app,
        joiner.clone(),
        "POST",
        &format!("/transfers/{tid}/accept"),
        Some(json!({ "secret": secret })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{moved}");
    assert_eq!(moved["id"], id);
    assert_eq!(moved["status"]["phase"], "ready");

    // It is the taker's now, and visible to the giver only as a closed offer.
    let (st, _) = send(&app, joiner, "GET", &format!("/volumes/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (_, page) = send(&app, owner, "GET", "/transfers", None).await;
    assert_eq!(page["items"][0]["status"]["phase"], "accepted");
    assert_eq!(page["items"][0]["spec"]["resource"], id);
}

#[tokio::test]
async fn a_wrong_secret_and_an_unknown_id_answer_the_same_way() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let taker = tenant(c.store(), "taker", &[]);
    let owner = caller(Some(giver), "member");
    let joiner = caller(Some(taker), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (_, offered) = send(
        &app,
        owner,
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();

    let (st_wrong, wrong) = send(
        &app,
        joiner.clone(),
        "POST",
        &format!("/transfers/{tid}/accept"),
        Some(json!({ "secret": "vxt_deadbeef" })),
    )
    .await;
    let (st_missing, missing) = send(
        &app,
        joiner,
        "POST",
        "/transfers/xfer-00000000-0000-0000-0000-000000000000/accept",
        Some(json!({ "secret": "vxt_deadbeef" })),
    )
    .await;

    assert_eq!(st_wrong, StatusCode::NOT_FOUND);
    assert_eq!(st_wrong, st_missing);
    // Same shape, and neither says anything about secrets or existence.
    assert_eq!(wrong["error"]["code"], missing["error"]["code"]);
    for body in [&wrong, &missing] {
        let msg = body["error"]["message"].as_str().unwrap();
        assert!(msg.contains("not found"), "{msg}");
        assert!(!msg.contains("secret"), "{msg}");
    }
}

#[tokio::test]
async fn the_receiving_project_limit_is_the_one_that_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(ProjectQuota));
    let giver = tenant(c.store(), "giver", &[]);
    let tight = tenant(c.store(), "tight", &[(VOLUMES_GIB, 1)]);
    let roomy = tenant(c.store(), "roomy", &[]);
    let owner = caller(Some(giver.clone()), "member");
    let squeeze = caller(Some(tight.clone()), "member");
    let space = caller(Some(roomy.clone()), "member");

    let id = make_volume(&app, &owner, "data", 8).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (_, offered) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();
    let secret = offered["secret"].as_str().unwrap().to_owned();

    let (st, refused) = send(
        &app,
        squeeze,
        "POST",
        &format!("/transfers/{tid}/accept"),
        Some(json!({ "secret": secret.clone() })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["error"]["code"], "quota_exceeded");
    assert_eq!(refused["error"]["details"]["resource"], VOLUMES_GIB);
    assert_eq!(refused["error"]["details"]["limit"], 1);

    // Still open, so the same secret lands it where there is room.
    let (st, _) = send(
        &app,
        space,
        "POST",
        &format!("/transfers/{tid}/accept"),
        Some(json!({ "secret": secret.clone() })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, read) = send(&app, owner, "GET", &format!("/transfers/{tid}"), None).await;
    assert_eq!(read["status"]["phase"], "accepted");
}

#[tokio::test]
async fn only_the_giver_can_cancel_and_only_while_it_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let taker = tenant(c.store(), "taker", &[]);
    let owner = caller(Some(giver.clone()), "member");
    let joiner = caller(Some(taker.clone()), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (_, offered) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();

    // A project that was not the giver cannot take the offer back. It gets
    // the same answer as an id it was never shown.
    let (st, _) = send(
        &app,
        joiner.clone(),
        "DELETE",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, closed) = send(
        &app,
        owner.clone(),
        "DELETE",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{closed}");
    assert_eq!(closed["status"]["phase"], "closed");

    // Twice is a conflict, not a second success.
    let (st, _) = send(&app, owner, "DELETE", &format!("/transfers/{tid}"), None).await;
    assert_eq!(st, StatusCode::CONFLICT);
}

#[tokio::test]
async fn an_admin_may_offer_and_cancel_across_projects() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let admin = Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    };

    let id = make_volume(&app, &admin, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    c.volumes()
        .modify(&vol.meta.id, |v| {
            v.meta.project = Some(giver.clone());
            Ok(())
        })
        .unwrap();

    let (st, offered) = send(
        &app,
        admin.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{offered}");
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();
    let (_, read) = send(&app, admin.clone(), "GET", "/transfers", None).await;
    assert_eq!(read["total"], 1);
    let (st, _) = send(&app, admin, "DELETE", &format!("/transfers/{tid}"), None).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn a_token_with_no_project_cannot_take_an_offer() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let owner = caller(Some(giver), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (_, offered) = send(
        &app,
        owner,
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();
    let secret = offered["secret"].as_str().unwrap().to_owned();

    let nobody = caller(None, "member");
    let (st, body) = send(
        &app,
        nobody,
        "POST",
        &format!("/transfers/{tid}/accept"),
        Some(json!({ "secret": secret })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

    // And an id that is not a transfer id is refused before anything is read.
    let (st, _) = send(
        &app,
        caller(Some(tenant(c.store(), "other", &[])), "member"),
        "POST",
        "/transfers/not-an-xfer-id/accept",
        Some(json!({ "secret": secret })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_attached_volume_refuses_to_be_offered() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let owner = caller(Some(giver), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    c.mark_attached(&vol.meta.id, &Id::new("mach"), None)
        .unwrap();

    let (st, body) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("attached"));

    // Detached again, it can be offered.
    c.mark_detached(&vol.meta.id).unwrap();
    let (st, _) = send(
        &app,
        owner,
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
}

#[tokio::test]
async fn an_offered_volume_cannot_be_deleted_or_attached_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let owner = caller(Some(giver), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (st, _) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, body) = send(
        &app,
        owner.clone(),
        "DELETE",
        &format!("/volumes/{id}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    let (st, body) = send(
        &app,
        owner,
        "POST",
        &format!("/volumes/{id}/attach"),
        Some(json!({ "machine": Id::new("mach").to_string() })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        c.get_volume(None, &id).unwrap().status.phase,
        Phase::Ready,
        "the volume was not touched"
    );
}

#[tokio::test]
async fn both_sides_can_read_an_offer_and_a_stranger_cannot() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let taker = tenant(c.store(), "taker", &[]);
    let stranger = tenant(c.store(), "stranger", &[]);
    let owner = caller(Some(giver.clone()), "member");
    let joiner = caller(Some(taker.clone()), "member");
    let outsider = caller(Some(stranger), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (_, offered) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();

    // The giver sees its own offer; a project-scoped reader is enough.
    let (st, _) = send(
        &app,
        owner.clone(),
        "GET",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        send(&app, owner, "GET", "/transfers", None).await.1["total"],
        1
    );

    // Before it is accepted, nothing ties the offer to a taker: the record
    // names the volume and the giver, and the only thing naming a receiver
    // is the secret, which is not in any read. So a prospective taker cannot
    // list its way to the offer; it was handed the id and the secret.
    let (st, _) = send(
        &app,
        joiner.clone(),
        "GET",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        send(&app, joiner.clone(), "GET", "/transfers", None)
            .await
            .1["total"],
        0
    );

    // Once it is taken, both sides can see the result.
    let secret = offered["secret"].as_str().unwrap().to_owned();
    let (st, _) = send(
        &app,
        joiner.clone(),
        "POST",
        &format!("/transfers/{tid}/accept"),
        Some(json!({ "secret": secret })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, read) = send(&app, joiner, "GET", &format!("/transfers/{tid}"), None).await;
    assert_eq!(st, StatusCode::OK, "{read}");
    assert_eq!(read["status"]["phase"], "accepted");
    assert_eq!(read["spec"]["resource"], id);

    // Someone else's project is told nothing about it.
    let (st, _) = send(
        &app,
        outsider.clone(),
        "GET",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        send(&app, outsider, "GET", "/transfers", None).await.1["total"],
        0
    );
}

#[tokio::test]
async fn a_moved_volume_answers_to_its_name_in_the_project_that_took_it() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let taker = tenant(c.store(), "taker", &[]);
    let owner = caller(Some(giver.clone()), "member");
    let joiner = caller(Some(taker.clone()), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let moved = move_volume(&app, &owner, &joiner, &id).await;
    assert_eq!(moved["id"], id);

    // A name read is what `pi volume show` and `pi volume delete` do first,
    // so a volume that arrived by transfer has to answer to it there.
    let (st, by_name) = send(&app, joiner.clone(), "GET", "/volumes/data", None).await;
    assert_eq!(st, StatusCode::OK, "{by_name}");
    assert_eq!(by_name["id"], id);
    assert_eq!(by_name["status"]["phase"], "ready");
    // The list the CLI resolves a name against, and the filter it can ask for.
    let (st, listed) = send(&app, joiner.clone(), "GET", "/volumes", None).await;
    assert_eq!(st, StatusCode::OK, "{listed}");
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["items"][0]["name"], "data");
    let (_, filtered) = send(&app, joiner.clone(), "GET", "/volumes?name=data", None).await;
    assert_eq!(filtered["total"], 1, "{filtered}");
    // What a full id answers is the same row, so both spellings agree.
    let (_, by_id) = send(&app, joiner.clone(), "GET", &format!("/volumes/{id}"), None).await;
    assert_eq!(by_id["name"], "data");

    // The giver no longer finds it by name, by id or in its own list.
    let (st, _) = send(&app, owner.clone(), "GET", "/volumes/data", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(&app, owner.clone(), "GET", &format!("/volumes/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        send(&app, owner, "GET", "/volumes", None).await.1["total"],
        0
    );

    // The taker can delete it by name: that is the whole point of the move.
    let (st, body) = send(&app, joiner, "DELETE", "/volumes/data", None).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn a_moved_snapshot_answers_to_its_name_in_the_project_that_took_it() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let taker = tenant(c.store(), "taker", &[]);
    let owner = caller(Some(giver.clone()), "member");
    let joiner = caller(Some(taker.clone()), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (st, snap) = send(
        &app,
        owner.clone(),
        "POST",
        "/snapshots",
        Some(json!({ "name": "before", "spec": { "volume": id } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{snap}");
    let sid = snap["id"].as_str().unwrap().to_owned();
    let snap = c
        .snapshots()
        .get(&Id::parse(kind::SNAPSHOT, &sid).expect("snapshot id"))
        .unwrap();
    c.reconcile_snapshot(&snap.meta.id).await.unwrap();

    move_volume(&app, &owner, &joiner, &id).await;

    let (st, by_name) = send(&app, joiner.clone(), "GET", "/snapshots/before", None).await;
    assert_eq!(st, StatusCode::OK, "{by_name}");
    assert_eq!(by_name["id"], sid);
    assert_eq!(by_name["status"]["phase"], "ready");
    let (_, listed) = send(&app, joiner.clone(), "GET", "/snapshots", None).await;
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["items"][0]["name"], "before");
    let (_, filtered) = send(&app, joiner.clone(), "GET", "/snapshots?name=before", None).await;
    assert_eq!(filtered["total"], 1, "{filtered}");
    let (_, by_id) = send(&app, joiner, "GET", &format!("/snapshots/{sid}"), None).await;
    assert_eq!(by_id["name"], "before");

    let (st, _) = send(&app, owner.clone(), "GET", "/snapshots/before", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(
        &app,
        owner.clone(),
        "GET",
        &format!("/snapshots/{sid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        send(&app, owner, "GET", "/snapshots", None).await.1["total"],
        0
    );
}

/// The annotation naming the stored hash of the one-time secret. The accept
/// path needs it so the store keeps it; no answer may carry it out.
const SECRET_HASH: &str = "pistack.io/transfer-secret";

/// Walk a JSON answer and name the first object holding `key`.
fn holder(carrying: &Value, key: &str) -> Option<Value> {
    match carrying {
        Value::Object(map) => {
            if map.contains_key(key) {
                return Some(Value::Object(map.clone()));
            }
            map.values().find_map(|v| holder(v, key))
        }
        Value::Array(items) => items.iter().find_map(|v| holder(v, key)),
        _ => None,
    }
}

fn assert_no_secret_hash(answer: &Value, where_: &str) {
    if let Some(obj) = holder(answer, SECRET_HASH) {
        panic!("{where_} carries the annotation {SECRET_HASH}: {obj}");
    }
}

/// Offer a volume and read the offer back through every route that answers
/// with a transfer, including the admin's.
#[tokio::test]
async fn no_answer_carries_the_stored_secret_hash() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let owner = caller(Some(giver.clone()), "member");
    let admin = Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    };

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();

    // The offer answer is the one that shows the secret, so it is the one
    // easiest to answer with the hash too.
    let (st, offered) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{offered}");
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();
    let secret = offered["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("vxt_"), "{secret}");
    assert_no_secret_hash(&offered, "POST /volumes/{id}/transfer");
    // The stored record still has it, so the accept below can check the secret.
    assert!(c
        .get_transfer(&tid)
        .unwrap()
        .meta
        .annotations
        .contains_key(SECRET_HASH));

    let (st, listed) = send(&app, owner.clone(), "GET", "/transfers", None).await;
    assert_eq!(st, StatusCode::OK, "{listed}");
    assert_eq!(listed["total"], 1, "{listed}");
    assert_no_secret_hash(&listed, "GET /transfers");

    let (st, read) = send(
        &app,
        owner.clone(),
        "GET",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{read}");
    assert_no_secret_hash(&read, "GET /transfers/{id}");

    let (st, on_volume) = send(
        &app,
        owner.clone(),
        "GET",
        &format!("/volumes/{id}/transfers"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{on_volume}");
    assert_eq!(on_volume["total"], 1, "{on_volume}");
    assert_no_secret_hash(&on_volume, "GET /volumes/{id}/transfers");

    // An admin with no project sees offers across projects, and gets the same
    // stripped record.
    let (st, admin_read) = send(
        &app,
        admin.clone(),
        "GET",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{admin_read}");
    assert_no_secret_hash(&admin_read, "admin GET /transfers/{id}");
    let (st, admin_list) = send(&app, admin.clone(), "GET", "/transfers", None).await;
    assert_eq!(st, StatusCode::OK, "{admin_list}");
    assert_eq!(admin_list["total"], 1, "{admin_list}");
    assert_no_secret_hash(&admin_list, "admin GET /transfers");
    let (st, admin_on_volume) = send(
        &app,
        admin,
        "GET",
        &format!("/volumes/{id}/transfers"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{admin_on_volume}");
    assert_no_secret_hash(&admin_on_volume, "admin GET /volumes/{id}/transfers");

    // The cancel answer is a transfer too.
    let (st, closed) = send(
        &app,
        owner.clone(),
        "DELETE",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{closed}");
    assert_eq!(closed["status"]["phase"], "closed");
    assert_no_secret_hash(&closed, "DELETE /transfers/{id}");
}

/// Stripping the hash from an answer must not reach the record the accept
/// path checks a secret against.
#[tokio::test]
async fn an_offer_is_still_taken_after_every_read_stripped_the_hash() {
    let dir = tempfile::tempdir().unwrap();
    let (c, app) = app(dir.path(), Arc::new(AllowAll));
    let giver = tenant(c.store(), "giver", &[]);
    let taker = tenant(c.store(), "taker", &[]);
    let owner = caller(Some(giver.clone()), "member");
    let joiner = caller(Some(taker), "member");

    let id = make_volume(&app, &owner, "data", 1).await;
    let vol = c.get_volume(None, &id).unwrap();
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let (st, offered) = send(
        &app,
        owner.clone(),
        "POST",
        &format!("/volumes/{id}/transfer"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{offered}");
    let tid = offered["transfer"]["id"].as_str().unwrap().to_owned();
    let secret = offered["secret"].as_str().unwrap().to_owned();
    assert_no_secret_hash(&offered, "offer answer");

    // Read the offer out through every route, then take it with the secret
    // the offer was answered with.
    send(&app, owner.clone(), "GET", "/transfers", None).await;
    send(
        &app,
        owner.clone(),
        "GET",
        &format!("/transfers/{tid}"),
        None,
    )
    .await;
    let (_, moved) = send(
        &app,
        joiner,
        "POST",
        &format!("/transfers/{tid}/accept"),
        Some(json!({ "secret": secret })),
    )
    .await;
    assert_eq!(moved["id"], id, "{moved}");

    let (st, read) = send(&app, owner, "GET", &format!("/transfers/{tid}"), None).await;
    assert_eq!(st, StatusCode::OK, "{read}");
    assert_eq!(read["status"]["phase"], "accepted");
    assert_no_secret_hash(&read, "GET /transfers/{id} after the accept");
}
