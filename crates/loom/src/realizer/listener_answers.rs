// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;

pub(crate) async fn listener_answers(addr: &str) -> bool {
    let Ok(sock) = addr.parse::<SocketAddr>() else {
        return false;
    };
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(sock)).await {
            Ok(Ok(stream)) => {
                drop(stream);
                return true;
            }
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    false
}
