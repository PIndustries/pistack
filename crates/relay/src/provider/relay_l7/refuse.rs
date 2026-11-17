// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The answer when no member can take the request: 503.

use tokio::io::AsyncWriteExt;

pub(super) async fn refuse<S>(mut stream: S)
where
    S: tokio::io::AsyncWrite + Unpin,
{
    let body = b"unavailable";
    let msg = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes()).await;
    let _ = stream.write_all(body).await;
}
