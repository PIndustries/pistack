// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The answer when no route matches and there is no default member: 404.

use tokio::io::AsyncWriteExt;

pub(super) async fn not_found<S>(mut stream: S)
where
    S: tokio::io::AsyncWrite + Unpin,
{
    let body = b"no route";
    let msg = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes()).await;
    let _ = stream.write_all(body).await;
}
