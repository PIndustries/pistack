// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The copy loops: accept on the balancer's socket, dial a member, pipe
//! both directions.

use super::dial::Dial;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};

pub(super) async fn accept_loop(
    listener: TcpListener,
    dial: Arc<parking_lot::Mutex<Arc<Dial>>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        let dial = dial.lock().clone();
                        tokio::spawn(async move {
                            forward_one(stream, dial, peer.ip()).await;
                        });
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "forward accept ended");
                        break;
                    }
                }
            }
        }
    }
}

pub(super) async fn forward_one(mut inbound: TcpStream, dial: Arc<Dial>, client: IpAddr) {
    let _ = inbound.set_nodelay(true);
    let Some(dest) = dial.picker.pick(&dial.snapshot(), Some(client)) else {
        return;
    };
    dial.open(dest);
    let outbound = TcpStream::connect(dest).await;
    let mut outbound = match outbound {
        Ok(stream) => stream,
        Err(e) => {
            dial.close(dest);
            tracing::debug!(%dest, error = %e, "forward member is not up");
            return;
        }
    };
    let _ = outbound.set_nodelay(true);
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
    dial.close(dest);
}
