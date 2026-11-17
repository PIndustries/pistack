// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One client's pairing with one member. Ending the pairing stops the task
//! carrying its replies.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(super) struct Session {
    pub(super) dest: SocketAddr,
    pub(super) sock: Arc<UdpSocket>,
    pub(super) cancel: CancellationToken,
    pub(super) task: JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}
