// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Per-balancer state: the socket, the serving task, and the shared handles
//! every accepted connection is handed.

use super::table::Table;
use crate::picker::Picker;
use crate::types::Algorithm;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub(super) enum Serve {
    Http,
    Stream,
    Tls(Arc<rustls::ServerConfig>),
}

pub(super) struct Slot {
    pub(super) bound: SocketAddr,
    pub(super) listener: Option<TcpListener>,
    pub(super) task: Option<JoinHandle<()>>,
    pub(super) cancel: tokio_util::sync::CancellationToken,
    pub(super) members: Arc<parking_lot::Mutex<Table>>,
    pub(super) picker: Picker,
    pub(super) conns: Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
    pub(super) limit: Arc<AtomicUsize>,
    pub(super) inflight: Arc<AtomicUsize>,
    pub(super) mode: Arc<parking_lot::Mutex<Serve>>,
    pub(super) udp: Option<crate::udp::UdpForward>,
}

impl Slot {
    /// A balancer that has just been bound: no listener task yet, and the
    /// default algorithm serving plain HTTP routes.
    pub(super) fn new(bound: SocketAddr, listener: TcpListener) -> Self {
        Self {
            bound,
            listener: Some(listener),
            task: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            members: Arc::new(parking_lot::Mutex::new(Table::default())),
            picker: Picker::new(Algorithm::default()),
            conns: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            limit: Arc::new(AtomicUsize::new(0)),
            inflight: Arc::new(AtomicUsize::new(0)),
            mode: Arc::new(parking_lot::Mutex::new(Serve::Http)),
            udp: None,
        }
    }
}
