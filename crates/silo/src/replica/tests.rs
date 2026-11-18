// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::Id;

use super::marker::Marker;
use super::source::Source;
use super::*;

#[test]
fn the_leader_is_asked_first_and_this_server_never() {
    let store = Store::in_memory().unwrap();
    let peers = Peers {
        source: Source::Cluster {
            store: store.clone(),
            me: Some("n2".into()),
        },
        net: keep::peer::PeerNet::plain(),
        key_from: None,
    };
    assert!(peers.urls().is_empty(), "no member list, nobody to ask");

    store
        .put_meta(
            "members",
            &serde_json::json!([
            {"id": "n1", "api": "http://10.0.0.11:8080", "role": "voter",
                 "agent_api": "https://10.0.0.11:8444"},
            {"id": "n2", "api": "http://10.0.0.12:8080", "role": "voter"},
                {"id": "n3", "api": "http://10.0.0.13:8080", "role": "learner"},
            {"id": "n4", "api": "", "role": "learner"}
            ]),
        )
        .unwrap();
    store.put_meta("cluster_leader_id", &"n3").unwrap();
    assert_eq!(
        peers.urls(),
        vec!["http://10.0.0.13:8080", "http://10.0.0.11:8080"]
    );
    // The TLS address goes with the server that has one.
    let targets = peers.targets();
    assert_eq!(targets[0], ("http://10.0.0.13:8080".to_owned(), None));
    assert_eq!(targets[1].1.as_deref(), Some("https://10.0.0.11:8444"));
}

#[tokio::test]
async fn a_marker_keeps_the_checksum_and_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = Id::new("bkt");
    assert!(read_marker(dir.path(), &bucket, "a/b").await.is_none());
    write_marker(dir.path(), &bucket, "a/b", "abc123")
        .await
        .unwrap();
    write_marker(dir.path(), &bucket, "a\nb", "ffff")
        .await
        .unwrap();
    assert_eq!(
        read_marker(dir.path(), &bucket, "a/b").await,
        Some(Marker {
            sha256: "abc123".into(),
            key: "a/b".into()
        })
    );
    // A key may hold a newline; everything after the first line is the key.
    assert_eq!(
        read_marker(dir.path(), &bucket, "a\nb").await.unwrap().key,
        "a\nb"
    );
    remove_marker(dir.path(), &bucket, "a/b").await;
    assert!(read_marker(dir.path(), &bucket, "a/b").await.is_none());
}
