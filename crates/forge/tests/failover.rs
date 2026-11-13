// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End to end over real sockets: two control planes on one store and an
//! `anvil` agent that knows both. The first is the leader and is killed
//! with a start command in flight; the second takes over. The machine must
//! come up, and the host must have started it once.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil::MachineRuntime;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use forge::{DriverRegistry, Forge, Host, Machine};
use pistack_rpc::proto::{MachineRequest, RunningMachine};
use pistack_rpc::{AgentClient, CommandQueue, QueueConfig, COMMAND_RECORD_PREFIX};
use pistack_store::Store;
use pistack_types::{ApiResult, Id, Module, ModuleContext, Phase};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const NODE: &str = "agent-1";

/// The mock runtime, counting how often it is asked to start a machine.
#[derive(Default)]
struct Counting {
    inner: anvil::MockRuntime,
    starts: AtomicUsize,
}

#[async_trait::async_trait]
impl MachineRuntime for Counting {
    fn name(&self) -> &'static str {
        "mock"
    }
    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.inner.start(req).await
    }
    async fn stop(&self, id: &Id) -> ApiResult<()> {
        self.inner.stop(id).await
    }
    async fn delete(&self, id: &Id) -> ApiResult<()> {
        self.inner.delete(id).await
    }
    async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
        self.inner.list().await
    }
}

/// Answer every result an agent posts with an error, as a leader does that
/// handed out a command and went away before the answer.
async fn lose_results(req: Request, next: Next) -> Response {
    if req.method() == Method::POST && req.uri().path().ends_with("/results") {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    next.run(req).await
}

/// One control plane on `listener`: forge and the agent routes over
/// `store`, leading from the start.
async fn control(store: Store, listener: TcpListener, takes_results: bool) {
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "control".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(20),
        capacity: 64,
        heartbeat_interval: Duration::from_secs(1),
        ovn: None,
    });
    queue.attach_store(store);
    let forge = Forge::with_queue(Arc::new(DriverRegistry::new()), queue.clone()).dev_admin(true);
    let mut rpc = pistack_rpc::router(queue);
    if !takes_results {
        rpc = rpc.layer(axum::middleware::from_fn(lose_results));
    }
    let app = Router::new()
        .nest("/compute", forge.router(ctx.clone()))
        .nest("/rpc", rpc);
    forge.start(ctx).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn records(store: &Store) -> usize {
    store.raw_scan("meta", COMMAND_RECORD_PREFIX).unwrap().len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_in_flight_when_the_leader_dies_is_not_lost_and_not_run_twice() {
    let deadline = Instant::now() + Duration::from_secs(30);
    let store = Store::in_memory().unwrap();
    let machines = store.collection::<Machine>();
    let hosts = store.collection::<Host>();

    // The first leader runs on a runtime of its own, so it can be killed
    // outright: its reconciler, its queue and its listener all stop at once
    // and write nothing more.
    let first = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_base = format!("http://{}", listener.local_addr().unwrap());
    let std_listener = listener.into_std().unwrap();
    first.spawn({
        let store = store.clone();
        async move {
            let listener = TcpListener::from_std(std_listener).unwrap();
            control(store, listener, false).await
        }
    });
    // The second server's address is known to the agent from the start. It
    // serves nothing until it takes over.
    let second_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_base = format!("http://{}", second_listener.local_addr().unwrap());

    let runtime = Arc::new(Counting::default());
    let client = AgentClient::new(format!("{first_base}/rpc"), NODE).unwrap();
    client.learn_servers([second_base.as_str()]);
    let stop = CancellationToken::new();
    let agent = tokio::spawn(anvil::run_agent_with_facts(
        client,
        anvil::Runtimes::all(runtime.clone()),
        anvil::HostFacts {
            node_name: NODE.into(),
            resources: pistack_types::Resources {
                vcpus: 4,
                memory_mib: 8192,
                disk_gib: 100,
                gpus: 0,
            },
            capabilities: vec![],
        },
        stop.clone(),
    ));
    loop {
        assert!(Instant::now() < deadline, "host never registered");
        let ready = hosts
            .list(None)
            .unwrap()
            .into_iter()
            .any(|h| h.spec.node_name == NODE && h.status.phase == Phase::Ready);
        if ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The leader places the machine and sends the start. The host runs it;
    // the answer never reaches the leader.
    let resp = reqwest::Client::new()
        .post(format!("{first_base}/compute/machines"))
        .json(&serde_json::json!({
            "name": "web",
            "spec": {"size": "s.1x1", "image": "ubuntu-24.04"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "{}", resp.text().await.unwrap());
    let machine: Machine = resp.json().await.unwrap();
    let id = machine.meta.id.clone();
    loop {
        assert!(Instant::now() < deadline, "host never ran the start");
        if runtime.starts.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(machines.get(&id).unwrap().status.phase, Phase::Creating);
    assert_eq!(records(&store), 1, "the command's id is in the store");

    // The leader dies. The other server takes over on the same store.
    first.shutdown_background();
    tokio::spawn(control(store.clone(), second_listener, true));

    let ready = loop {
        assert!(Instant::now() < deadline, "machine never became ready");
        let m = machines.get(&id).unwrap();
        if m.status.phase == Phase::Ready {
            break m;
        }
        assert_ne!(m.status.phase, Phase::Error, "{}", m.status.message);
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(ready.status.addresses, vec!["10.0.0.1"]);
    assert_eq!(ready.status.power, forge::PowerState::On);
    // The new leader sent the start again under the id it had, and the host
    // answered from what it kept.
    assert_eq!(runtime.starts.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.list().await.unwrap().len(), 1);
    // Answered, so nothing is left in the store.
    let gone = loop {
        if records(&store) == 0 || Instant::now() >= deadline {
            break records(&store);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(gone, 0);

    stop.cancel();
    agent.await.unwrap().unwrap();
}
