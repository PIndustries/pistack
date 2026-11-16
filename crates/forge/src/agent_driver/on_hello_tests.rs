// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use super::test_support::*;
use crate::driver::DriverRegistry;
use crate::types::Host;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use std::sync::Arc;

#[tokio::test]
async fn on_hello_stores_and_replaces_underlay() {
    let store = Store::in_memory().unwrap();
    let drivers = Arc::new(DriverRegistry::new());
    let queue = CommandQueue::default();
    let reg = Registrar::new(store.clone(), drivers, queue.clone());

    let mut h = hello("h1");
    h.underlay_ip = Some("10.1.0.2".into());
    let host = reg.on_hello(&h).unwrap();
    assert_eq!(host.spec.underlay_ip.as_deref(), Some("10.1.0.2"));

    h.underlay_ip = Some("100.64.8.8".into());
    let again = reg.on_hello(&h).unwrap();
    assert_eq!(again.meta.id, host.meta.id);
    assert_eq!(again.spec.underlay_ip.as_deref(), Some("100.64.8.8"));

    h.underlay_ip = None;
    let cleared = reg.on_hello(&h).unwrap();
    assert_eq!(cleared.spec.underlay_ip, None);

    h.underlay_ip = Some("8.8.8.8".into());
    let rejected = reg.on_hello(&h).unwrap();
    assert_eq!(rejected.spec.underlay_ip, None);

    h.underlay_ip = Some("192.168.5.9".into());
    queue.hello(&h);
    reg.resync();
    let kept = store.collection::<Host>().get(&host.meta.id).unwrap();
    assert_eq!(kept.spec.underlay_ip.as_deref(), Some("192.168.5.9"));
}
