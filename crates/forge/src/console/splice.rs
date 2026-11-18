// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use std::net::Ipv4Addr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Copy bytes between the browser socket and the loopback VNC port.
pub async fn splice(socket: WebSocket, port: u16) {
    let stream = match TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(%error, port, "vnc connect failed");
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);

    let from_vnc = tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let to_browser = tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            if sink.send(Message::Binary(chunk.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(message) = stream.next().await {
        let bytes = match message {
            Ok(Message::Binary(bytes)) => bytes,
            Ok(Message::Text(text)) => text.as_bytes().to_vec().into(),
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Err(_) => break,
        };
        if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
            break;
        }
    }

    from_vnc.abort();
    to_browser.abort();
}
