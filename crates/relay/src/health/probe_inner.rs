// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The connection one probe makes: a connect, then for an `Http` monitor a
//! `GET` whose status line must be in the monitor's expected codes.

use super::Probe;
use crate::types::{HealthMonitor, MonitorType};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) async fn probe_inner(addr: SocketAddr, hm: &HealthMonitor) -> Probe {
    let mut stream = match tokio::net::TcpStream::connect(addr).await {
        Ok(s) => s,
        Err(e) => return Probe::Down(format!("connect: {e}")),
    };
    if hm.kind != MonitorType::Http {
        return Probe::Up;
    }
    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: pistack-relay/{}\r\nConnection: close\r\n\r\n",
        hm.path,
        addr,
        pistack_types::BUILD
    );
    if let Err(e) = stream.write_all(req.as_bytes()).await {
        return Probe::Down(format!("write: {e}"));
    }
    let mut buf = vec![0u8; 512];
    let mut read = 0;
    while read < buf.len() {
        match stream.read(&mut buf[read..]).await {
            Ok(0) => break,
            Ok(n) => {
                read += n;
                if buf[..read].contains(&b'\n') {
                    break;
                }
            }
            Err(e) => return Probe::Down(format!("read: {e}")),
        }
    }
    let line = String::from_utf8_lossy(&buf[..read]);
    let line = line.lines().next().unwrap_or_default();
    // "HTTP/1.1 200 OK"
    let code = line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok());
    match code {
        Some(c) if hm.expected_codes.contains(&c) => Probe::Up,
        Some(c) => Probe::Down(format!("status {c}")),
        None => Probe::Down(format!("bad status line {line:?}")),
    }
}
