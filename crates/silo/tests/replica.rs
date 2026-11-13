// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Two control-plane servers: one store (records replicate), two data
//! directories (bytes do not). Server A listens on a real socket and
//! server B fetches from it.

use axum::Router;
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{kind, Id, Meta};
use silo::replica::{sync_once, SyncReport};
use silo::{BucketSpec, Client, Peers};
use std::time::Duration;
use tokio::io::AsyncReadExt;

struct Pair {
    a: Client,
    b: Client,
    a_dir: tempfile::TempDir,
    b_dir: tempfile::TempDir,
    url: String,
}

/// Serve `client` the way the gateway mounts it, at `/silo`.
async fn serve(client: Client) -> String {
    let app = Router::new().nest("/silo", silo::http::router(client));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    url
}

async fn pair() -> Pair {
    let store = Store::in_memory().unwrap();
    // The signing secret every member has; the peer key comes from it.
    store
        .put_meta(gate::token::META_JWT_SECRET, &"c2VjcmV0".to_owned())
        .unwrap();
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let a = Client::new(store.clone(), a_dir.path());
    let url = serve(a.clone()).await;
    let peers = Peers::fixed(vec![url.clone()]).with_key_from(store.clone());
    let b = Client::new(store, b_dir.path()).with_peers(peers);
    Pair {
        a,
        b,
        a_dir,
        b_dir,
        url,
    }
}

fn bucket(c: &Client, name: &str, versioning: bool) -> Id {
    c.create_bucket(
        Meta::new(kind::BUCKET, name, Some(Id::new(kind::PROJECT))),
        BucketSpec { versioning },
    )
    .unwrap()
    .meta
    .id
}

async fn read(c: &Client, bucket: &Id, key: &str) -> Vec<u8> {
    let (_obj, mut file) = c.open_object(bucket, key).await.unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await.unwrap();
    bytes
}

#[tokio::test]
async fn a_read_on_another_server_fetches_the_bytes() {
    let p = pair().await;
    let id = bucket(&p.a, "data", false);
    p.a.put_object(&id, "dir/report.txt", b"written on a", None)
        .await
        .unwrap();
    let on_b = p.b_dir.path().join(id.to_string()).join("dir/report.txt");
    assert!(!on_b.exists());

    assert_eq!(read(&p.b, &id, "dir/report.txt").await, b"written on a");
    assert_eq!(std::fs::read(&on_b).unwrap(), b"written on a");

    // The second read is served from b's own disk: a is gone.
    std::fs::remove_dir_all(p.a_dir.path().join(id.to_string())).unwrap();
    assert_eq!(read(&p.b, &id, "dir/report.txt").await, b"written on a");
}

#[tokio::test]
async fn an_overwritten_key_is_fetched_again() {
    let p = pair().await;
    let id = bucket(&p.a, "data", false);
    p.a.put_object(&id, "k", b"first!", None).await.unwrap();
    assert_eq!(read(&p.b, &id, "k").await, b"first!");

    // Same length, so the size check alone would pass the old file.
    p.a.put_object(&id, "k", b"second", None).await.unwrap();
    assert_eq!(read(&p.b, &id, "k").await, b"second");
}

#[tokio::test]
async fn wrong_bytes_from_a_peer_never_become_the_object() {
    let p = pair().await;
    let id = bucket(&p.a, "data", false);
    p.a.put_object(&id, "k", b"honest bytes", None)
        .await
        .unwrap();
    std::fs::write(
        p.a_dir.path().join(id.to_string()).join("k"),
        b"rotten bytes",
    )
    .unwrap();

    let err = p.b.open_object(&id, "k").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(err.message.contains("checksum"), "{err}");
    let dir = p.b_dir.path().join(id.to_string());
    let left: Vec<_> = std::fs::read_dir(&dir)
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(
        left.is_empty(),
        "nothing is kept from a bad answer: {left:?}"
    );
}

#[tokio::test]
async fn no_peer_is_not_found_and_a_dead_peer_is_unavailable() {
    let store = Store::in_memory().unwrap();
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let a = Client::new(store.clone(), a_dir.path());
    let id = bucket(&a, "data", false);
    a.put_object(&id, "k", b"bytes", None).await.unwrap();

    // A single server: the store has no member list, so nobody to ask.
    let alone =
        Client::new(store.clone(), b_dir.path()).with_peers(Peers::from_cluster(store.clone()));
    let err = alone.open_object(&id, "k").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "{err}");

    // A member that does not answer. The port was free a moment ago.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    store
        .put_meta(
            "members",
            &serde_json::json!([
                {"id": "n1", "api": format!("http://127.0.0.1:{port}"), "role": "voter"}
            ]),
        )
        .unwrap();
    let err = alone.open_object(&id, "k").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(err.message.contains("1 other server"), "{err}");
}

#[tokio::test]
async fn the_pull_copies_what_is_missing_and_a_follower_prunes() {
    let p = pair().await;
    let plain = bucket(&p.a, "plain", false);
    let kept = bucket(&p.a, "versions", true);
    p.a.put_object(&plain, "one", b"1", None).await.unwrap();
    p.a.put_object(&plain, "two", b"22", None).await.unwrap();
    p.a.put_object(&kept, "doc", b"old", None).await.unwrap();
    p.a.put_object(&kept, "doc", b"newer", None).await.unwrap();

    // Two current objects, plus both generations of the versioned key.
    let report = sync_once(&p.b, true, Duration::ZERO).await.unwrap();
    assert_eq!(
        report,
        SyncReport {
            fetched: 4,
            missing: 0,
            pruned: 0
        }
    );
    let b_plain = p.b_dir.path().join(plain.to_string());
    assert_eq!(std::fs::read(b_plain.join("two")).unwrap(), b"22");
    let b_kept = p.b_dir.path().join(kept.to_string());
    assert_eq!(std::fs::read(b_kept.join("doc")).unwrap(), b"newer");
    assert_eq!(std::fs::read_dir(b_kept.join(".blobs")).unwrap().count(), 1);
    assert_eq!(
        sync_once(&p.b, true, Duration::ZERO).await.unwrap(),
        SyncReport::default(),
        "a second pass has nothing to do"
    );

    p.a.delete_object(&plain, "one").await.unwrap();
    p.a.delete_bucket(&kept).await.unwrap();

    // The leader does not prune: a put writes the file before the record.
    let report = sync_once(&p.b, false, Duration::ZERO).await.unwrap();
    assert_eq!(report.pruned, 0);
    assert!(b_plain.join("one").exists());

    // Nor does a follower touch anything younger than the grace period.
    let report = sync_once(&p.b, true, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(report.pruned, 0);

    let report = sync_once(&p.b, true, Duration::ZERO).await.unwrap();
    assert_eq!(report.pruned, 2, "one object and one bucket directory");
    assert!(!b_plain.join("one").exists());
    assert!(b_plain.join("two").exists());
    assert!(!b_kept.exists());
}

#[tokio::test]
async fn the_peer_route_serves_bytes_by_checksum_and_nothing_else() {
    let p = pair().await;
    let id = bucket(&p.a, "data", false);
    let obj = p.a.put_object(&id, "k", b"peer bytes", None).await.unwrap();
    let http = reqwest::Client::new();
    let key = keep::peer::peer_key(p.a.store()).unwrap();
    let get = |path: String| {
        let http = http.clone();
        let key = key.clone();
        let url = format!("{}/silo{path}", p.url);
        async move {
            http.get(url)
                .header(keep::peer::PEER_HEADER, key)
                .send()
                .await
                .unwrap()
        }
    };

    let resp = get(format!("/peer/blobs/{id}/{}", obj.status.sha256)).await;
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"peer bytes");

    // Knowing the checksum is not enough: the caller has to be a member.
    for wrong in [None, Some("guess")] {
        let mut req = http.get(format!(
            "{}/silo/peer/blobs/{id}/{}",
            p.url, obj.status.sha256
        ));
        if let Some(wrong) = wrong {
            req = req.header(keep::peer::PEER_HEADER, wrong);
        }
        assert_eq!(req.send().await.unwrap().status().as_u16(), 401);
    }

    let other = "0".repeat(64);
    assert_eq!(
        get(format!("/peer/blobs/{id}/{other}"))
            .await
            .status()
            .as_u16(),
        404
    );
    assert_eq!(
        get(format!("/peer/blobs/{id}/not-a-checksum"))
            .await
            .status()
            .as_u16(),
        404
    );
    // Every other route still wants a caller.
    assert_eq!(
        get(format!("/buckets/{id}/objects/k"))
            .await
            .status()
            .as_u16(),
        401
    );

    // A server that only has the record answers 404; it does not go and
    // fetch on a peer's behalf, so two servers cannot ask each other forever.
    let b_url = serve(p.b.clone()).await;
    let resp = http
        .get(format!(
            "{b_url}/silo/peer/blobs/{id}/{}",
            obj.status.sha256
        ))
        .header(keep::peer::PEER_HEADER, &key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    assert!(!p.b_dir.path().join(id.to_string()).exists());
}
