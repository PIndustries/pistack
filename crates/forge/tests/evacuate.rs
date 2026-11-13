// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End to end over a real socket with two `anvil` agents: a host goes
//! silent, its machine is evacuated and starts on the other host, and when
//! the first host's agent comes back, still holding its copy, it is told to
//! delete it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil::MachineRuntime as _;
use axum::Router;
use forge::maintain::EvacuateReport;
use forge::{DriverRegistry, Forge, Host, Machine, EVACUATED_FROM_ANNOTATION};
use pistack_rpc::{AgentClient, CommandQueue, QueueConfig};
use pistack_store::Store;
use pistack_types::{Id, Module, ModuleContext, Phase};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

fn start_agent(
    base: &str,
    node: &str,
    runtime: Arc<anvil::MockRuntime>,
    stop: CancellationToken,
) -> JoinHandle<anyhow::Result<()>> {
    let client = AgentClient::new(format!("{base}/rpc"), node).unwrap();
    let facts = anvil::HostFacts {
        node_name: node.into(),
        resources: pistack_types::Resources {
            vcpus: 4,
            memory_mib: 8192,
            disk_gib: 100,
            gpus: 0,
        },
        capabilities: vec![],
    };
    tokio::spawn(anvil::run_agent_with_facts(
        client,
        anvil::Runtimes::all(runtime),
        facts,
        stop,
    ))
}

async fn held(runtime: &anvil::MockRuntime) -> Vec<Id> {
    runtime
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect()
}

#[tokio::test]
async fn a_host_that_returns_after_an_evacuation_drops_its_stale_machines() {
    let deadline = Instant::now() + Duration::from_secs(30);

    let store = Store::in_memory().unwrap();
    let shutdown = CancellationToken::new();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: shutdown.clone(),
        public_url: "http://test".into(),
        node_name: "control".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(10),
        capacity: 64,
        heartbeat_interval: Duration::from_secs(1),
        ovn: None,
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
    let http = reqwest::Client::new();
    let hosts = store.collection::<Host>();
    let machines = store.collection::<Machine>();
    let host_of = |node: &str| {
        hosts
            .list(None)
            .unwrap()
            .into_iter()
            .find(|h| h.spec.node_name == node && h.status.phase == Phase::Ready)
    };

    // Only the first agent is up when the machines are made, so they land
    // on it.
    let first_runtime = Arc::new(anvil::MockRuntime::new());
    let first_stop = shutdown.child_token();
    let first_agent = start_agent(&base, "first", first_runtime.clone(), first_stop.clone());
    let first = loop {
        assert!(Instant::now() < deadline, "first host never registered");
        if let Some(h) = host_of("first") {
            break h;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let mut ids = Vec::new();
    for name in ["web", "db"] {
        let resp = http
            .post(format!("{base}/compute/machines"))
            .json(&serde_json::json!({
                "name": name,
                "spec": {"size": "s.1x1", "image": "ubuntu-24.04"}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201, "{}", resp.text().await.unwrap());
        let m: Machine = resp.json().await.unwrap();
        ids.push(m.meta.id.clone());
    }
    for id in &ids {
        loop {
            assert!(Instant::now() < deadline, "machine never became ready");
            let m = machines.get(id).unwrap();
            if m.status.phase == Phase::Ready {
                assert_eq!(m.status.host.as_ref(), Some(&first.meta.id));
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let mut on_first = held(&first_runtime).await;
    on_first.sort();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(on_first, want);

    let second_runtime = Arc::new(anvil::MockRuntime::new());
    let second_agent = start_agent(&base, "second", second_runtime.clone(), shutdown.clone());
    let second = loop {
        assert!(Instant::now() < deadline, "second host never registered");
        if let Some(h) = host_of("second") {
            break h;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // The first host loses its agent. Its runtime keeps the machines, as a
    // hypervisor would. Age the last report instead of waiting out the
    // real limit.
    first_stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), first_agent)
        .await
        .expect("first agent stops")
        .unwrap()
        .unwrap();
    hosts
        .modify(&first.meta.id, |h| {
            h.status.last_seen = Some(chrono::Utc::now() - chrono::Duration::seconds(120));
            Ok(())
        })
        .unwrap();
    loop {
        assert!(Instant::now() < deadline, "first host never went down");
        if hosts.get(&first.meta.id).unwrap().status.phase == Phase::Error {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Evacuate it. Both machines must come up on the second host.
    let resp = http
        .post(format!("{base}/compute/hosts/first/evacuate"))
        .json(&serde_json::json!({"rebuild_from_image": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
    let report: EvacuateReport = resp.json().await.unwrap();
    assert_eq!(report.evacuated.len(), 2, "{report:?}");
    assert!(report.skipped.is_empty(), "{report:?}");
    for id in &ids {
        loop {
            assert!(Instant::now() < deadline, "machine never came back");
            let m = machines.get(id).unwrap();
            if m.status.phase == Phase::Ready {
                assert_eq!(m.status.host.as_ref(), Some(&second.meta.id));
                break;
            }
            assert_ne!(m.status.phase, Phase::Error, "{}", m.status.message);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let mut on_second = held(&second_runtime).await;
    on_second.sort();
    assert_eq!(on_second, want);
    // The dead host still has its copies: nothing could reach it.
    assert_eq!(held(&first_runtime).await.len(), 2);

    // The first host comes back with the same runtime. Its heartbeat
    // reports both machines, and both are deleted there.
    let first_agent = start_agent(&base, "first", first_runtime.clone(), shutdown.clone());
    loop {
        assert!(
            Instant::now() < deadline,
            "stale machines were never deleted: {:?}",
            held(&first_runtime).await
        );
        if held(&first_runtime).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        hosts.get(&first.meta.id).unwrap().status.phase,
        Phase::Ready
    );

    // The live copies are untouched, through a few more heartbeats.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let mut on_second = held(&second_runtime).await;
    on_second.sort();
    assert_eq!(on_second, want);
    for id in &ids {
        let m = machines.get(id).unwrap();
        assert_eq!(m.status.phase, Phase::Ready);
        assert_eq!(m.status.host.as_ref(), Some(&second.meta.id));
        assert_eq!(
            m.meta.annotations.get(EVACUATED_FROM_ANNOTATION),
            Some(&first.meta.id.to_string())
        );
    }

    // A host that is up again cannot be evacuated.
    let resp = http
        .post(format!("{base}/compute/hosts/first/evacuate"))
        .json(&serde_json::json!({"rebuild_from_image": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    shutdown.cancel();
    for agent in [first_agent, second_agent] {
        tokio::time::timeout(Duration::from_secs(5), agent)
            .await
            .expect("agent exits on shutdown")
            .unwrap()
            .unwrap();
    }
}
