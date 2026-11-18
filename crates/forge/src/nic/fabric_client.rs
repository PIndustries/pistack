// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Client, HostNat, MockBackend, NetBackend};
use pistack_store::Store;
use std::sync::Arc;

pub(crate) fn fabric_client(store: &Store) -> Client {
    let backend: Arc<dyn NetBackend> = match std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref()
    {
        Some("hostnat") => Arc::new(HostNat::new()),
        Some("overlay") => Arc::new(fabric::OverlayBackend::new()),
        Some("ovn") => Arc::new(fabric::OvnBackend::from_env()),
        _ => Arc::new(MockBackend::new()),
    };
    Client::new(store.clone(), backend)
}
