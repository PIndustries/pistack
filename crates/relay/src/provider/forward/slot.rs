// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One balancer's socket and the task that serves it.

use super::dial::Dial;
use crate::types::Algorithm;
use pistack_types::Id;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub(super) struct Slot {
    pub(super) bound: SocketAddr,
    /// Held until the accept task starts, then `None`.
    pub(super) listener: Option<TcpListener>,
    pub(super) task: Option<JoinHandle<()>>,
    pub(super) cancel: tokio_util::sync::CancellationToken,
    /// Who the accept loop dials and what it owes them. Replaced by
    /// `ensure_listener`, read by every connection it accepts.
    pub(super) dial: Arc<parking_lot::Mutex<Arc<Dial>>>,
    pub(super) udp: Option<crate::udp::UdpForward>,
}

impl Slot {
    pub(super) fn new(bound: SocketAddr, listener: Option<TcpListener>) -> Self {
        Self {
            bound,
            listener,
            task: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            dial: Arc::new(parking_lot::Mutex::new(Arc::new(Dial::new(
                Algorithm::default(),
            )))),
            udp: None,
        }
    }
}

pub(super) type Slots = parking_lot::Mutex<HashMap<Id, Slot>>;
