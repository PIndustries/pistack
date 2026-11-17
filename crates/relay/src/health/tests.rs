// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Health checker tests: the probes, the hysteresis, and a tick that flips
//! a member and touches its pool.

use super::apply;
use super::probe;
use super::Probe;
use crate::types::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn hm(rise: u32, fall: u32) -> HealthMonitor {
    HealthMonitor {
        rise,
        fall,
        ..HealthMonitor::default()
    }
}

#[test]
fn rise_and_fall_hysteresis() {
    let hm = hm(2, 3);
    let mut s = MemberStatus::default(); // NoMonitor
    assert!(!apply(&mut s, &hm, &Probe::Up));
    assert_eq!(s.streak, 1);
    assert!(apply(&mut s, &hm, &Probe::Up));
    assert_eq!(s.operating_status, OperatingStatus::Online);
    assert!(!apply(&mut s, &hm, &Probe::Down("x".into())));
    assert!(!apply(&mut s, &hm, &Probe::Down("x".into())));
    assert_eq!(s.operating_status, OperatingStatus::Online);
    // a success resets the fall streak
    assert!(!apply(&mut s, &hm, &Probe::Up));
    assert_eq!(s.streak, 0);
    assert!(!apply(&mut s, &hm, &Probe::Down("x".into())));
    assert!(!apply(&mut s, &hm, &Probe::Down("x".into())));
    assert!(apply(&mut s, &hm, &Probe::Down("x".into())));
    assert_eq!(s.operating_status, OperatingStatus::Offline);
    assert_eq!(s.message, "x");
}

#[tokio::test]
async fn tcp_and_http_probes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    assert_eq!(probe(addr, &HealthMonitor::default()).await, Probe::Up);
    let http = HealthMonitor {
        kind: MonitorType::Http,
        ..HealthMonitor::default()
    };
    assert_eq!(probe(addr, &http).await, Probe::Down("status 503".into()));
    let lenient = HealthMonitor {
        expected_codes: vec![200, 503],
        ..http
    };
    assert_eq!(probe(addr, &lenient).await, Probe::Up);

    // nothing listening
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = closed.local_addr().unwrap();
    drop(closed);
    assert!(matches!(
        probe(dead, &HealthMonitor::default()).await,
        Probe::Down(_)
    ));
}
