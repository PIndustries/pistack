// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

use super::*;

#[test]
fn advertise_stays_on_a_private_address() {
    assert!(check_advertise("10.66.66.1".parse().unwrap()).is_ok());
    assert!(check_advertise("192.168.1.9".parse().unwrap()).is_ok());
    assert!(check_advertise(IpAddr::V4(Ipv4Addr::UNSPECIFIED)).is_err());
    assert!(check_advertise("8.8.8.8".parse().unwrap()).is_err());
    assert!(check_advertise("255.255.255.255".parse().unwrap()).is_err());
}

#[test]
fn scan_is_the_private_prefix_without_self() {
    let got = scan_targets("10.66.66.1".parse().unwrap(), 24).unwrap();
    assert_eq!(got.len(), 253);
    assert!(!got.contains(&"10.66.66.0".parse().unwrap()));
    assert!(!got.contains(&"10.66.66.1".parse().unwrap()));
    assert!(!got.contains(&"10.66.66.255".parse().unwrap()));
    assert!(got.contains(&"10.66.66.2".parse().unwrap()));
    assert!(scan_targets("10.66.66.1".parse().unwrap(), 16).is_err());
    assert!(scan_targets("10.66.66.1".parse().unwrap(), 32)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn two_sockets_hear_each_other() {
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    let left = Announce {
        cluster_id: "c1".into(),
        node_id: "a".into(),
        api: "http://127.0.0.1:1".into(),
        raft: "127.0.0.1:2".into(),
    };
    let right = Announce {
        cluster_id: "c1".into(),
        node_id: "b".into(),
        api: "http://127.0.0.1:3".into(),
        raft: "127.0.0.1:4".into(),
    };
    let left_task = collect_announcements(
        ip,
        18451,
        "127.0.0.1:18452".parse().unwrap(),
        &left,
        Duration::from_millis(400),
        "a",
    );
    let right_task = collect_announcements(
        ip,
        18452,
        "127.0.0.1:18451".parse().unwrap(),
        &right,
        Duration::from_millis(400),
        "b",
    );
    let (heard_left, heard_right) = tokio::join!(left_task, right_task);
    let heard_left = heard_left.unwrap();
    let heard_right = heard_right.unwrap();
    assert_eq!(heard_left, vec![right]);
    assert_eq!(heard_right, vec![left]);
}

#[tokio::test]
async fn sweep_hears_one_responder() {
    let responder: SocketAddr = "127.0.0.1:18461".parse().unwrap();
    let bind: SocketAddr = "127.0.0.1:18462".parse().unwrap();
    let peer = Announce {
        cluster_id: String::new(),
        node_id: "peer".into(),
        api: "http://127.0.0.1:9".into(),
        raft: "127.0.0.1:10".into(),
    };
    let mine = Announce {
        cluster_id: String::new(),
        node_id: "me".into(),
        api: "http://127.0.0.1:11".into(),
        raft: "127.0.0.1:12".into(),
    };
    let sock = UdpSocket::bind(responder).await.unwrap();
    let reply = peer.clone();
    let task = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        let (_len, from) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf))
            .await
            .expect("responder timed out")
            .unwrap();
        let payload = serde_json::to_vec(&reply).unwrap();
        sock.send_to(&payload, from).await.unwrap();
    });
    let heard = sweep(bind, &[responder], &mine, Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!(heard, vec![peer]);
    task.await.unwrap();
}
