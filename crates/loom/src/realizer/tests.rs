// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::realizer::ensure_forward::ensure_forward;
use crate::realizer::service_label::SERVICE_LABEL;
use crate::types::{kinds, Protocol, Service, ServiceKind, ServicePort};
use pistack_store::Store;
use pistack_types::Meta;
use relay::{ListenerProtocol, PoolProtocol, Provider};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;

use super::*;
use crate::types::{ServiceSpec, ServiceStatus};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn is_open(at: &str) -> bool {
    TcpStream::connect(at).await.is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn losing_the_lead_mid_bind_closes_the_listener() {
    let store = Store::in_memory().unwrap();
    let relay = Arc::new(RelayRealizer::new().with_node_address([127, 0, 0, 1].into()));
    relay.bind_store(store.clone());
    let node_port = free_port();
    let mut svc = Service {
        meta: Meta::new(kinds::SERVICE, "mid-bind", None),
        spec: ServiceSpec {
            ports: vec![ServicePort {
                name: "http".into(),
                protocol: Protocol::Tcp,
                port: 80,
                target_port: Some(8080),
                node_port: None,
            }],
            kind: ServiceKind::NodePort,
            ..ServiceSpec::default()
        },
        status: ServiceStatus::default(),
    };
    svc.status.node_ports.insert(80, node_port);
    let at = format!("127.0.0.1:{node_port}");

    let reached = Arc::new(tokio::sync::Notify::new());
    let go = Arc::new(tokio::sync::Notify::new());
    *relay.held.pause.lock().unwrap() = Some((reached.clone(), go.clone()));

    let pass = {
        let relay = relay.clone();
        let svc = svc.clone();
        tokio::spawn(async move { relay.realize(&svc, &[]).await })
    };
    // The cluster-side listener goes first; let it finish.
    reached.notified().await;
    go.notify_one();
    // The node port is bound and the pass has not returned yet.
    reached.notified().await;
    assert!(is_open(&at).await, "node port should be bound mid-pass");
    relay.release().await;
    go.notify_one();
    let err = pass.await.unwrap().unwrap_err();
    assert!(err.message.contains("lead was lost"), "{}", err.message);
    assert!(
        !is_open(&at).await,
        "a bind in flight survived losing the lead"
    );
    assert!(relay.held.ids().is_empty());

    // The next term opens it again on the same address.
    *relay.held.pause.lock().unwrap() = None;
    relay.realize(&svc, &[]).await.unwrap();
    assert!(is_open(&at).await);
    relay.release().await;
    assert!(!is_open(&at).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pass_that_fails_after_the_bind_is_still_closed() {
    let store = Store::in_memory().unwrap();
    let relay = Arc::new(RelayRealizer::new().with_node_address([127, 0, 0, 1].into()));
    relay.bind_store(store.clone());
    let port = free_port();
    let bind = SocketAddr::from(([127, 0, 0, 1], port));
    let at = bind.to_string();

    let reached = Arc::new(tokio::sync::Notify::new());
    let go = Arc::new(tokio::sync::Notify::new());
    *relay.held.pause.lock().unwrap() = Some((reached.clone(), go.clone()));
    let pass = {
        let relay = relay.clone();
        tokio::spawn(async move {
            let client = relay.client().unwrap();
            ensure_forward(
                &relay.held,
                &client,
                None,
                "after-bind",
                SERVICE_LABEL,
                "svc",
                Provider::Forward,
                ListenerProtocol::Tcp,
                PoolProtocol::Tcp,
                port,
                Some(bind),
                &[],
                &[],
            )
            .await
        })
    };
    reached.notified().await;
    assert!(is_open(&at).await);
    // Make the rest of the pass fail with the socket already open.
    for l in store.collection::<relay::Listener>().list(None).unwrap() {
        store
            .collection::<relay::Listener>()
            .delete(&l.meta.id)
            .unwrap();
    }
    go.notify_one();
    assert!(pass.await.unwrap().is_err());
    assert!(is_open(&at).await, "the failed pass left its socket open");

    // Losing the lead closes it all the same.
    relay.release_services().await;
    assert!(!is_open(&at).await);
}
