// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Usage end to end over a real socket: an agent with the mock runtime
//! reports what its machine uses in a heartbeat, forge turns that into
//! series, and they come back from the stats API, in the Prometheus text
//! and through an alarm.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil::runtime::mock::{MOCK_CPU_STEP_NS, MOCK_MEMORY_BYTES};
use axum::Router;
use forge::stats::Stats;
use forge::{DriverRegistry, Forge, Machine};
use gauge::{AlarmPhase, AlarmSpec, Comparison};
use pistack_rpc::{AgentClient, CommandQueue, QueueConfig};
use pistack_store::Store;
use pistack_types::{kind, Meta, Module, ModuleContext, Phase};
use tokio_util::sync::CancellationToken;

const NODE: &str = "usage-agent-1";

fn value(stats: &Stats, metric: &str) -> Option<f64> {
    stats
        .series
        .iter()
        .find(|s| s.metric == metric)
        .map(|s| s.value)
}

#[tokio::test]
async fn a_machines_usage_reaches_the_api_the_exposition_and_an_alarm() {
    // An agent measures at most every ten seconds, and the machine is made
    // after its first report.
    let deadline = Instant::now() + Duration::from_secs(60);

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

    // An alarm on the machine's memory, by its name, before it exists.
    let gauge = gauge::Gauge.client(store.clone());
    let alarm = gauge
        .create_alarm(
            Meta::new(kind::ALARM, "usage-web-memory", None),
            AlarmSpec {
                name: "usage-web-memory".into(),
                metric: "pistack_machine_memory_bytes{name=\"usage-web\"}".into(),
                comparison: Comparison::Gt,
                threshold: 1024.0,
                for_seconds: 0,
            },
        )
        .unwrap();
    assert_eq!(alarm.status.phase, AlarmPhase::Pending);

    let runtimes = anvil::Runtimes::all(Arc::new(anvil::MockRuntime::new()));
    let client = AgentClient::new(format!("{base}/rpc"), NODE).unwrap();
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

    // The host's own usage arrives with the first heartbeat.
    let host = loop {
        assert!(Instant::now() < deadline, "the host never reported usage");
        let response = http
            .get(format!("{base}/compute/hosts/{NODE}/stats"))
            .send()
            .await
            .unwrap();
        if response.status() == 200 {
            let stats: Stats = response.json().await.unwrap();
            if value(&stats, "pistack_host_load1").is_some() {
                break stats;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(host.host, NODE);
    assert_eq!(value(&host, "pistack_host_vcpus"), Some(4.0));
    assert_eq!(value(&host, "pistack_host_memory_mib"), Some(8192.0));
    assert!(value(&host, "pistack_host_data_disk_free_bytes").unwrap() > 0.0);
    assert_eq!(value(&host, "pistack_host_machines"), Some(0.0));
    for s in &host.series {
        assert_eq!(s.labels.get("host").map(String::as_str), Some(NODE));
        assert!(!s.labels.contains_key("machine"), "{}", s.metric);
    }

    let response = http
        .post(format!("{base}/compute/machines"))
        .json(&serde_json::json!({
            "name": "usage-web",
            "spec": {"size": "s.1x1", "image": "ubuntu-24.04"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "{}", response.text().await.unwrap());
    let machine: Machine = response.json().await.unwrap();
    let id = machine.meta.id.to_string();

    // Nothing is known about it until its host measures again.
    let stats: Stats = http
        .get(format!("{base}/compute/machines/{id}/stats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats.name, "usage-web");
    assert!(stats.series.is_empty(), "{stats:?}");

    let stats = loop {
        assert!(
            Instant::now() < deadline,
            "the machine never reported usage"
        );
        let m: Machine = http
            .get(format!("{base}/compute/machines/{id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_ne!(m.status.phase, Phase::Error, "{}", m.status.message);
        let stats: Stats = http
            .get(format!("{base}/compute/machines/usage-web/stats"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !stats.series.is_empty() {
            break stats;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(stats.id, id);
    assert_eq!(stats.host, NODE);
    assert_eq!(
        value(&stats, "pistack_machine_memory_bytes"),
        Some(MOCK_MEMORY_BYTES as f64)
    );
    // The mock's CPU time is a whole number of steps.
    let cpu = value(&stats, "pistack_machine_cpu_seconds_total").unwrap();
    let step = MOCK_CPU_STEP_NS as f64 / 1e9;
    assert!(cpu >= step && (cpu / step).fract() == 0.0, "{cpu}");
    let disk = stats
        .series
        .iter()
        .find(|s| s.metric == "pistack_machine_disk_written_bytes_total")
        .expect("a disk series");
    assert_eq!(disk.labels["device"], "vda");
    let nic = stats
        .series
        .iter()
        .find(|s| s.metric == "pistack_machine_network_receive_bytes_total")
        .expect("a network series");
    assert_eq!(nic.labels["interface"], "mock0");
    for s in &stats.series {
        assert_eq!(s.labels["machine"], id, "{}", s.metric);
        assert_eq!(s.labels["name"], "usage-web");
        assert_eq!(s.labels["host"], NODE);
        assert!(s.labels.contains_key("project"));
    }
    assert_eq!(stats.series.len(), 9, "{stats:?}");

    // The same series are in the text a scraper reads.
    let text = gauge.metrics_text();
    let line = text
        .lines()
        .find(|l| {
            l.starts_with("pistack_machine_memory_bytes{")
                && l.contains(&format!("machine=\"{id}\""))
        })
        .unwrap_or_else(|| panic!("no memory line for {id} in {text}"));
    assert!(line.contains("name=\"usage-web\""), "{line}");
    assert!(line.contains(&format!("host=\"{NODE}\"")), "{line}");
    assert!(line.ends_with(&format!(" {MOCK_MEMORY_BYTES}")), "{line}");
    assert!(
        text.contains("# TYPE pistack_machine_cpu_seconds_total counter"),
        "{text}"
    );

    // And the alarm that was waiting for the machine has fired.
    let alarm = gauge.get_alarm(None, &alarm.meta.id.to_string()).unwrap();
    assert_eq!(alarm.status.phase, AlarmPhase::Firing);
    assert_eq!(alarm.status.last_value, Some(MOCK_MEMORY_BYTES as f64));
    assert!(alarm
        .status
        .series
        .as_deref()
        .is_some_and(|series| series.contains(&id)));

    shutdown.cancel();
    let _ = agent.await;
}
