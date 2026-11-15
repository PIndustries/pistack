// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::execute::execute;
use crate::probe::spec::probe_action::ProbeAction;
use pistack_rpc::proto::PROBE_READINESS;
use pistack_types::{kind, Id};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn http_and_tcp_probes_hit_local_listeners() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 512];
                let _ = sock.read(&mut buf).await;
                let text = std::str::from_utf8(&buf).unwrap_or("");
                let status = if text.contains("GET /ready") {
                    200
                } else {
                    500
                };
                let body = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(body.as_bytes()).await;
            });
        }
    });
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_addr = tcp.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            if tcp.accept().await.is_err() {
                break;
            }
        }
    });

    let base = DueCheck {
        machine: Id::new(kind::MACHINE),
        generation: 1,
        container: "web".into(),
        kind: PROBE_READINESS.into(),
        action: ProbeAction::Tcp {
            port: tcp_addr.port(),
        },
        timeout: Duration::from_secs(2),
        address: Some(addr.ip().to_string()),
    };
    assert!(execute(&base).await.success);

    let mut closed = base.clone();
    closed.action = ProbeAction::Tcp { port: 1 };
    let refused = execute(&closed).await;
    assert!(!refused.success);
    assert!(!refused.unsupported);

    let mut ready = base.clone();
    ready.action = ProbeAction::Http {
        path: "/ready".into(),
        port: addr.port(),
        host: None,
        https: false,
    };
    assert!(execute(&ready).await.success);

    let mut bad = ready.clone();
    bad.action = ProbeAction::Http {
        path: "/live".into(),
        port: addr.port(),
        host: None,
        https: false,
    };
    let failed = execute(&bad).await;
    assert!(!failed.success);
    assert_eq!(failed.message, "http status 500");

    let mut exec = base.clone();
    exec.action = ProbeAction::Exec {
        command: vec!["/bin/true".into()],
    };
    let unsupported = execute(&exec).await;
    assert!(unsupported.unsupported);
    assert!(!unsupported.success);
    assert_eq!(unsupported.message, "exec probes are not supported");
}
