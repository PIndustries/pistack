// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use super::test_support::*;
use crate::driver::DriverRegistry;
use crate::types::Host;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_hello_from_before_the_registrar_listened_is_registered() {
    let store = Store::in_memory().unwrap();
    let drivers = Arc::new(DriverRegistry::new());
    let queue = CommandQueue::default();
    queue.hello(&hello("early"));
    // A node that has only polled has announced nothing to register.
    let _ = queue.next("silent", std::time::Duration::ZERO, 1).await;
    let stop = CancellationToken::new();
    let registrar = Registrar::new(store.clone(), drivers, queue);
    let run = tokio::spawn(registrar.run(stop.clone()));
    let nodes = || -> Vec<String> {
        store
            .collection::<Host>()
            .list(None)
            .unwrap()
            .into_iter()
            .map(|h| h.spec.node_name.clone())
            .collect()
    };
    for _ in 0..100 {
        if !nodes().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    stop.cancel();
    run.await.unwrap();
    assert_eq!(nodes(), vec!["early"]);
}
