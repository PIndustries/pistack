// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End to end over a real socket: forge + the rpc router on one listener,
//! an `anvil` agent loop with the mock runtime joining it, a machine created
//! through the compute API scheduled onto that agent and started by its
//! runtime, then deleted again.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil::MachineRuntime as _;
use axum::Router;
use forge::{DriverRegistry, Forge, Host, Machine};
use pistack_rpc::{AgentClient, CommandQueue, QueueConfig};
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, Phase};
use tokio_util::sync::CancellationToken;

const NODE: &str = "agent-1";

#[tokio::test]
async fn machine_runs_on_a_joined_agent() {
    let deadline = Instant::now() + Duration::from_secs(15);

    // Control plane.
    let store = Store::in_memory().unwrap();
    let shutdown = CancellationToken::new();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: shutdown.clone(),
        public_url: "http://test".into(),
        node_name: "control".into(),
    };
    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(10),
        capacity: 64,
        heartbeat_interval: Duration::from_secs(1),
    });
    let forge = Forge::with_queue(Arc::new(DriverRegistry::new()), queue.clone()).dev_admin(true);
    let app = Router::new()
        .nest("/compute", forge.router(ctx.clone()))
        .nest("/rpc", pistack_rpc::router(queue.clone()));
    forge.start(ctx).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let sd = shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { sd.cancelled().await })
            .await
            .unwrap();
    });

    // The agent, with a mock runtime we keep a handle on.
    let mock = Arc::new(anvil::MockRuntime::new());
    let runtimes = anvil::Runtimes::all(mock.clone());
    let client = AgentClient::new(format!("{base}/rpc"), NODE).unwrap();
    // Pinned facts: the test must not depend on this box's free disk.
    let facts = anvil::HostFacts {
        node_name: NODE.into(),
        resources: pistack_types::Resources {
            vcpus: 4,
            memory_mib: 8192,
            disk_gib: 100,
            gpus: 0,
        },
        capabilities: vec![],
    };
    let agent = tokio::spawn(anvil::run_agent_with_facts(
        client,
        runtimes,
        facts,
        shutdown.clone(),
    ));

    let http = reqwest::Client::new();

    // The hello creates a Ready host for the agent's node.
    let host = loop {
        assert!(Instant::now() < deadline, "host never registered");
        let hosts: pistack_types::Page<Host> = http
            .get(format!("{base}/compute/hosts"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(h) = hosts
            .items
            .into_iter()
            .find(|h| h.spec.node_name == NODE && h.status.phase == Phase::Ready)
        {
            break h;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(host.spec.drivers.iter().any(|d| d == "mock"));

    // Create a machine; it must land on that host and come up with an address.
    let resp = http
        .post(format!("{base}/compute/machines"))
        .json(&serde_json::json!({
            "name": "web",
            "spec": {"size": "s.1x1", "image": "ubuntu-24.04"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "{}", resp.text().await.unwrap());
    let m: Machine = resp.json().await.unwrap();
    let id = m.meta.id.to_string();

    let ready = loop {
        assert!(Instant::now() < deadline, "machine never became ready");
        let m: Machine = http
            .get(format!("{base}/compute/machines/{id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if m.status.phase == Phase::Ready && !m.status.addresses.is_empty() {
            break m;
        }
        assert_ne!(m.status.phase, Phase::Error, "{}", m.status.message);
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(ready.status.host.as_ref(), Some(&host.meta.id));
    assert_eq!(ready.status.addresses, vec!["10.0.0.1"]);
    assert_eq!(ready.status.power, forge::PowerState::On);
    // The mock really holds it.
    let held = mock.list().await.unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, ready.meta.id);

    // Delete: the reconciler stops + deletes through the agent, then drops
    // the record.
    let resp = http
        .delete(format!("{base}/compute/machines/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    loop {
        assert!(Instant::now() < deadline, "machine never went away");
        let resp = http
            .get(format!("{base}/compute/machines/{id}"))
            .send()
            .await
            .unwrap();
        if resp.status() == 404 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(mock.list().await.unwrap().is_empty());

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), agent)
        .await
        .expect("agent exits on shutdown")
        .unwrap()
        .unwrap();
}
