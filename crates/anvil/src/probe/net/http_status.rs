// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::net::parse_status::parse_status;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub async fn http_status<S>(mut stream: S, path: &str, host: &str) -> Result<u16, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: pistack-probe\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;
    let mut buf = [0u8; 128];
    let mut n = 0;
    while n < buf.len() && !buf[..n].windows(2).any(|w| w == b"\r\n") {
        let read = stream
            .read(&mut buf[n..])
            .await
            .map_err(|e| format!("read: {e}"))?;
        if read == 0 {
            break;
        }
        n += read;
    }
    parse_status(&buf[..n]).ok_or_else(|| "http response has no status".to_owned())
}
