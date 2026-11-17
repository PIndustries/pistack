// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The bound UDP forwarder: the socket it holds, the members it may send
//! to, and the task copying between them.

use super::run::run;
use pistack_types::{ApiError, ApiResult};
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// A bound UDP forwarder. Drop cancels the copy task.
pub struct UdpForward {
    bound: SocketAddr,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl UdpForward {
    /// Bind `addr` (port 0 for an ephemeral port) and copy datagrams to
    /// `members`. `members` must be non-empty. The address must be loopback
    /// or private.
    pub async fn bind(addr: SocketAddr, members: Vec<SocketAddr>) -> ApiResult<Self> {
        let ip = addr.ip();
        if members.is_empty() {
            return Err(ApiError::unavailable("udp pool has no up member"));
        }
        if !crate::bind::allowed(ip) {
            return Err(ApiError::bad_request(format!(
                "udp refuses {ip}; only a loopback or private address can be bound"
            )));
        }
        let socket = UdpSocket::bind(addr)
            .await
            .map_err(|e| ApiError::unavailable(format!("udp bind {addr}: {e}")))?;
        let bound = socket
            .local_addr()
            .map_err(|e| ApiError::unavailable(format!("udp local address: {e}")))?;
        if !crate::bind::allowed(bound.ip()) {
            return Err(ApiError::unavailable(format!(
                "udp bound {bound}, which is not a loopback or private address"
            )));
        }
        let members = Arc::new(parking_lot::Mutex::new(members));
        let cancel = CancellationToken::new();
        let cursor = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(run(
            Arc::new(socket),
            members.clone(),
            cursor,
            cancel.clone(),
            ip,
        ));
        Ok(Self {
            bound,
            members,
            cancel,
            task: Some(task),
        })
    }

    pub fn bound(&self) -> SocketAddr {
        self.bound
    }

    pub fn set_members(&self, members: Vec<SocketAddr>) {
        *self.members.lock() = members;
    }

    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for UdpForward {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
