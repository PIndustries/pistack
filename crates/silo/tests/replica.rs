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

/// Two servers that both listen and name each other as the only peer.
/// `a` takes the puts, as the leader does.
struct Both {
    a: Client,
    b: Client,
    store: Store,
    a_dir: tempfile::TempDir,
    b_dir: tempfile::TempDir,
}

async fn both() -> Both {
    let store = Store::in_memory().unwrap();
    store
        .put_meta(gate::token::META_JWT_SECRET, &"c2VjcmV0".to_owned())
        .unwrap();
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let a_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let b_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer = |l: &tokio::net::TcpListener| {
        Peers::fixed(vec![format!("http://{}", l.local_addr().unwrap())])
            .with_key_from(store.clone())
    };
    let a = Client::new(store.clone(), a_dir.path())
        .with_min_part_bytes(1)
        .with_peers(peer(&b_listener));
    let b = Client::new(store.clone(), b_dir.path())
        .with_min_part_bytes(1)
        .with_peers(peer(&a_listener));
    for (listener, client) in [(a_listener, a.clone()), (b_listener, b.clone())] {
        let app = Router::new().nest("/silo", silo::http::router(client));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
    }
    Both {
        a,
        b,
        store,
        a_dir,
        b_dir,
    }
}

#[tokio::test]
async fn a_put_is_answered_once_another_server_holds_the_bytes() {
    let p = both().await;
    let id = bucket(&p.a, "data", false);
    p.a.put_object(&id, "report.txt", b"first", None)
        .await
        .unwrap();

    // A client over B's directory with nobody to ask: what is on B's disk.
    let b_disk = Client::new(p.store.clone(), p.b_dir.path());
    assert_eq!(read(&b_disk, &id, "report.txt").await, b"first");

    // An overwrite is copied too, not left as the old bytes.
    p.a.put_object(&id, "report.txt", b"second, longer", None)
        .await
        .unwrap();
    assert_eq!(read(&b_disk, &id, "report.txt").await, b"second, longer");
}

#[tokio::test]
async fn a_put_no_other_server_copies_is_unavailable_and_kept() {
    let store = Store::in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // A port nothing listens on.
    let gone = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", gone.local_addr().unwrap());
    drop(gone);
    let a = Client::new(store.clone(), dir.path()).with_peers(Peers::fixed(vec![url]));
    let id = bucket(&a, "data", false);

    let err = a.put_object(&id, "k", b"bytes", None).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(
        err.message.contains("stored on this server only"),
        "{}",
        err.message
    );
    assert!(err.message.contains("0 of 1"), "{}", err.message);
    // The object is there all the same.
    assert_eq!(read(&a, &id, "k").await, b"bytes");

    // One copy asked for: nobody is waited on.
    let alone = a.clone().with_put_copies(1);
    alone.put_object(&id, "k2", b"bytes", None).await.unwrap();

    // No peers at all, as in a cluster of one server.
    let single = Client::new(store, dir.path()).with_peers(Peers::fixed(Vec::new()));
    single.put_object(&id, "k3", b"bytes", None).await.unwrap();
}

#[tokio::test]
async fn a_pull_for_a_record_that_never_arrives_is_refused() {
    let p = pair().await;
    let key = keep::peer::peer_key(p.a.store()).unwrap();
    let object = Id::new(<silo::ObjectSpec as pistack_types::Spec>::KIND);
    let resp = reqwest::Client::new()
        .post(format!(
            "{}/silo/peer/pull/{object}/{}",
            p.url,
            "0".repeat(64)
        ))
        .header(keep::peer::PEER_HEADER, &key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 503);

    let resp = reqwest::Client::new()
        .post(format!(
            "{}/silo/peer/pull/{object}/{}",
            p.url,
            "0".repeat(64)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

async fn part(
    c: &Client,
    bucket: &Id,
    key: &str,
    upload: &str,
    n: u32,
    body: &'static str,
) -> String {
    let stream =
        futures::stream::once(async move { Ok::<_, std::io::Error>(bytes::Bytes::from(body)) });
    c.upload_part(
        bucket,
        key,
        upload,
        n,
        stream,
        Some(body.len() as u64),
        None,
    )
    .await
    .unwrap()
}

fn staged_dir(dir: &tempfile::TempDir, bucket: &Id, upload: &str) -> std::path::PathBuf {
    dir.path()
        .join(bucket.to_string())
        .join(".uploads")
        .join(upload)
}

#[tokio::test]
async fn an_upload_in_flight_is_finished_on_another_server() {
    let p = both().await;
    let id = bucket(&p.a, "data", false);
    let upload =
        p.a.create_multipart_upload(&id, "big.bin", None)
            .await
            .unwrap();
    let one = part(&p.a, &id, "big.bin", &upload, 1, "hello ").await;
    let two = part(&p.a, &id, "big.bin", &upload, 2, "world").await;

    // Both parts were on B before each UploadPart returned.
    let on_b = staged_dir(&p.b_dir, &id, &upload);
    assert_eq!(std::fs::read(on_b.join("part-1")).unwrap(), b"hello ");
    assert_eq!(std::fs::read(on_b.join("part-2")).unwrap(), b"world");

    // A is no longer the leader; the client completes against B.
    let parts = vec![
        silo::multipart::CompletedPart {
            part_number: 1,
            etag: one,
        },
        silo::multipart::CompletedPart {
            part_number: 2,
            etag: two,
        },
    ];
    p.b.complete_multipart_upload(&id, "big.bin", &upload, &parts)
        .await
        .unwrap();
    assert_eq!(read(&p.b, &id, "big.bin").await, b"hello world");
    assert!(!on_b.exists());

    // A hears the upload is over and drops its staging; B does not take
    // it back from A in the meantime.
    assert_eq!(silo::staging::sync(&p.b).await.unwrap().fetched, 0);
    assert!(!on_b.exists());
    let report = silo::staging::sync(&p.a).await.unwrap();
    assert_eq!(report.removed, 1);
    assert!(!staged_dir(&p.a_dir, &id, &upload).exists());
}

#[tokio::test]
async fn an_aborted_upload_is_removed_everywhere_and_stays_gone() {
    let p = both().await;
    let id = bucket(&p.a, "data", false);
    let upload = p.a.create_multipart_upload(&id, "k", None).await.unwrap();
    part(&p.a, &id, "k", &upload, 1, "bytes").await;
    let on_b = staged_dir(&p.b_dir, &id, &upload);
    assert!(on_b.join("part-1").exists());

    p.a.abort_multipart_upload(&id, "k", &upload).await.unwrap();
    // B still stages it, and A must not copy it back.
    assert_eq!(
        silo::staging::sync(&p.a).await.unwrap(),
        silo::staging::StagedReport::default()
    );
    assert!(!staged_dir(&p.a_dir, &id, &upload).exists());

    assert_eq!(silo::staging::sync(&p.b).await.unwrap().removed, 1);
    assert!(!on_b.exists());
}

#[tokio::test]
async fn the_part_written_later_wins() {
    let p = both().await;
    let id = bucket(&p.a, "data", false);
    let upload = p.a.create_multipart_upload(&id, "k", None).await.unwrap();
    part(&p.a, &id, "k", &upload, 1, "old").await;
    let on_b = staged_dir(&p.b_dir, &id, &upload).join("part-1");
    assert_eq!(std::fs::read(&on_b).unwrap(), b"old");

    // The same part again, taken by A without telling B.
    tokio::time::sleep(Duration::from_millis(30)).await;
    let quiet = p.a.clone().with_put_copies(1);
    part(&quiet, &id, "k", &upload, 1, "newer").await;
    assert_eq!(std::fs::read(&on_b).unwrap(), b"old");

    // A does not go back to B's older bytes; B takes A's.
    assert_eq!(silo::staging::sync(&p.a).await.unwrap().fetched, 0);
    assert_eq!(silo::staging::sync(&p.b).await.unwrap().fetched, 1);
    assert_eq!(std::fs::read(&on_b).unwrap(), b"newer");
    let on_a = staged_dir(&p.a_dir, &id, &upload).join("part-1");
    assert_eq!(std::fs::read(on_a).unwrap(), b"newer");
    assert_eq!(silo::staging::sync(&p.b).await.unwrap().fetched, 0);
}

#[tokio::test]
async fn a_part_no_other_server_copies_is_unavailable() {
    let store = Store::in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let gone = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", gone.local_addr().unwrap());
    drop(gone);
    let a = Client::new(store, dir.path()).with_peers(Peers::fixed(vec![url]));
    let id = bucket(&a, "data", false);

    let err = a.create_multipart_upload(&id, "k", None).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(
        err.message.contains("stored on this server only"),
        "{}",
        err.message
    );

    let alone = a.clone().with_put_copies(1);
    let upload = alone.create_multipart_upload(&id, "k", None).await.unwrap();
    part(&alone, &id, "k", &upload, 1, "bytes").await;
    let stream =
        futures::stream::once(async { Ok::<_, std::io::Error>(bytes::Bytes::from("bytes")) });
    let err = a
        .upload_part(&id, "k", &upload, 2, stream, Some(5), None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(err.message.contains("part 2"), "{}", err.message);
}

#[tokio::test]
async fn staging_is_listed_only_for_a_member() {
    let p = both().await;
    let id = bucket(&p.a, "data", false);
    let upload = p.a.create_multipart_upload(&id, "k", None).await.unwrap();
    part(&p.a, &id, "k", &upload, 1, "bytes").await;
    let a_url = p.b.peers().unwrap().urls().remove(0);
    for path in [
        "/silo/peer/uploads".to_owned(),
        format!("/silo/peer/uploads/{id}/{upload}"),
        format!("/silo/peer/uploads/{id}/{upload}/1"),
    ] {
        let resp = reqwest::get(format!("{a_url}{path}")).await.unwrap();
        assert_eq!(resp.status().as_u16(), 401, "{path}");
    }
}
