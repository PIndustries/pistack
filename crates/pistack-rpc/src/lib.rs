// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Host RPC and events.
//!
//! * [`proto`] – the wire types `anvil` and the control plane exchange.
//! * [`queue`] – [`CommandQueue`], per-node FIFOs with pending/ack tracking
//!   and timeouts; modules enqueue, the HTTP layer delivers.
//! * [`server`] – [`router`], the axum routes agents call. [`router_with_store`]
//!   applies join-token and client-certificate checks (strict mTLS by default;
//!   set `PISTACK_RPC_INSECURE=1` for local HTTP).
//! * [`client`] – [`AgentClient`], the agent side over reqwest.
//! * [`events`] – [`EventBus`], a typed `broadcast<Event>` with filtered
//!   subscriptions.
//!
//! The transport today is JSON over HTTP with a long-poll for commands; no
//! protobuf toolchain is required. The types are plain `serde`, so moving
//! to gRPC later touches only `server` and `client`.

pub mod client;
pub mod error;
pub mod events;
pub mod proto;
pub mod queue;
pub mod server;

pub use client::AgentClient;
pub use error::RpcError;
pub use events::{EventBus, EventStream};
pub use proto::{
    AgentEvent, AgentHello, Command, CommandId, CommandKind, CommandResult, DiskTarget,
    EnrollRequest, EnrollResponse, ExecEnv, ExecOutput, ExecRequest, Heartbeat, HeartbeatAck,
    HelloAck, ImageSource, LogText, MachineKind, MachineRef, MachineReport, MachineRequest,
    OvnHostConfig, OvnReport, PortPlug, PowerState, ProbeReport, ReportAck, RunningMachine,
    VolumeAttach, VolumeAttached, VolumeDetach, JOIN_HEADER, PROBE_LIVENESS, PROBE_READINESS,
};
pub use queue::{CommandQueue, NodeInfo, PendingInfo, QueueConfig, Ticket};
pub use server::{
    agent_address, router, router_with_auth, router_with_auth_and, router_with_extra,
    router_with_store, RpcAuth, REQUIRE_JOIN_ENV, RPC_INSECURE_ENV,
};

#[cfg(test)]
mod loopback {
    //! Client against router over a real socket.
    use std::time::Duration;

    use chrono::Utc;
    use pistack_types::{kind, Id, Phase, Resources};
    use serde_json::json;

    use super::*;

    async fn serve(queue: CommandQueue) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(queue);
        let h = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), h)
    }

    #[tokio::test]
    async fn full_round_trip() {
        let queue = CommandQueue::new(QueueConfig {
            command_timeout: Duration::from_secs(30),
            capacity: 16,
            heartbeat_interval: Duration::from_secs(7),
            ovn: None,
        });
        let (base, server) = serve(queue.clone()).await;
        let client = AgentClient::new(&base, "host-a")
            .unwrap()
            .with_token("join-token");

        // hello
        let ack = client
            .hello(&AgentHello {
                node_name: "host-a".into(),
                capabilities: vec!["kvm".into(), "ovn".into()],
                resources: Resources {
                    vcpus: 16,
                    memory_mib: 65536,
                    disk_gib: 1000,
                    gpus: 1,
                },
                version: "test".into(),
                labels: [("rack".to_string(), "r1".to_string())]
                    .into_iter()
                    .collect(),
                underlay_ip: None,
            })
            .await
            .unwrap();
        assert_eq!(ack.node_name, "host-a");
        assert_eq!(ack.heartbeat_interval_secs, 7);
        let info = queue.node("host-a").unwrap();
        assert_eq!(info.capabilities, vec!["kvm", "ovn"]);
        assert_eq!(info.labels["rack"], "r1");

        // Empty poll returns quickly with wait=0.
        let none = client.next_commands(Duration::ZERO).await.unwrap();
        assert!(none.is_empty());

        // A module enqueues; the agent's long-poll picks it up.
        let agent = client.clone();
        let poller =
            tokio::spawn(async move { agent.next_commands(Duration::from_secs(10)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let machine = Id::new(kind::MACHINE);
        let ticket = queue
            .enqueue(
                "host-a",
                CommandKind::StartMachine,
                json!({"machine": machine.to_string()}),
            )
            .unwrap();
        let cmds = poller.await.unwrap().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, ticket.id());
        assert_eq!(cmds[0].kind, CommandKind::StartMachine);
        assert_eq!(cmds[0].payload["machine"], machine.to_string());

        // heartbeat reports the machine running
        let hb = client
            .heartbeat(&Heartbeat {
                node_name: "host-a".into(),
                resources_free: Resources {
                    vcpus: 14,
                    memory_mib: 61440,
                    disk_gib: 980,
                    gpus: 1,
                },
                machines: vec![MachineReport {
                    id: machine.clone(),
                    phase: Phase::Ready,
                    message: String::new(),
                    resources: None,
                    addresses: vec!["10.0.0.5".into()],
                    power: None,
                    probes: Vec::new(),
                }],
                at: Utc::now(),
                ovn: None,
                usage: None,
            })
            .await
            .unwrap();
        assert!(hb.known);
        assert_eq!(hb.commands_waiting, 0);
        assert_eq!(queue.node("host-a").unwrap().machines[0].id, machine);

        // report: one real result and one bogus id
        let bogus = uuid::Uuid::now_v7();
        let rep = client
            .report(&[
                CommandResult::ok(cmds[0].id).with_payload(json!({"domain": "web"})),
                CommandResult::ok(bogus),
            ])
            .await
            .unwrap();
        assert_eq!(rep.accepted, 1);
        assert_eq!(rep.unknown, vec![bogus]);

        let result = ticket.wait().await.unwrap();
        assert!(result.ok);
        assert_eq!(result.payload["domain"], "web");
        assert_eq!(queue.pending_count(), 0);

        server.abort();
    }

    #[tokio::test]
    async fn exec_command_round_trips_over_the_command_poll() {
        let queue = CommandQueue::new(QueueConfig {
            command_timeout: Duration::from_secs(5),
            capacity: 4,
            heartbeat_interval: Duration::from_secs(30),
            ovn: None,
        });
        let (base, server) = serve(queue.clone()).await;
        let client = AgentClient::new(&base, "host-a").unwrap();
        let machine = Id::new(kind::MACHINE);
        let payload = json!({
            "id": machine.to_string(),
            "container": "web",
            "command": ["/bin/echo", "hi"],
            "timeout_secs": 5
        });
        let agent = client.clone();
        let poller = tokio::spawn(async move { agent.next_commands(Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        let ticket = queue
            .enqueue("host-a", CommandKind::Exec, payload.clone())
            .unwrap();
        let cmds = poller.await.unwrap().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, ticket.id());
        assert_eq!(cmds[0].kind, CommandKind::Exec);
        assert_eq!(cmds[0].payload, payload);
        client
            .report(&[CommandResult::ok(cmds[0].id).with_payload(json!({
                "exit_code": 0,
                "stdout": "hi\n",
                "stderr": ""
            }))])
            .await
            .unwrap();
        let result = ticket.wait().await.unwrap();
        assert!(result.ok);
        let out: ExecOutput = serde_json::from_value(result.payload).unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "hi\n");
        assert!(out.stderr.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn server_errors_become_api_errors() {
        let queue = CommandQueue::default();
        let (base, server) = serve(queue).await;

        // Bad node name in the path.
        let client = AgentClient::new(&base, "bad name!").unwrap();
        let e = client.next_commands(Duration::ZERO).await.unwrap_err();
        match e {
            RpcError::Api { status, error } => {
                assert_eq!(status, 400);
                assert_eq!(error.code, pistack_types::error::ErrorCode::BadRequest);
                assert!(error.message.contains("node_name"));
            }
            other => panic!("unexpected {other}"),
        }

        // Unknown route: axum's plain 404, synthesised into an ApiError.
        let http = reqwest::Client::new();
        let resp = http.get(format!("{base}/agents")).send().await.unwrap();
        assert_eq!(resp.status(), 404);

        // Unreachable server: a transport error.
        let dead = AgentClient::new("http://127.0.0.1:1", "n1").unwrap();
        assert!(matches!(
            dead.hello(&hello("n1")).await.unwrap_err(),
            RpcError::Http(_)
        ));

        server.abort();
    }

    fn hello(name: &str) -> AgentHello {
        AgentHello {
            node_name: name.into(),
            capabilities: vec![],
            resources: Resources::default(),
            version: String::new(),
            labels: Default::default(),
            underlay_ip: None,
        }
    }

    #[tokio::test]
    async fn long_poll_wait_is_capped_by_server() {
        let queue = CommandQueue::default();
        let (base, server) = serve(queue).await;
        // The client caps too, but hit the server with a raw oversize wait
        // and make sure it answers (empty) within the cap. We cannot wait
        // 30s in a unit test, so just verify the parameter is accepted and
        // a small wait returns promptly.
        let http = reqwest::Client::new();
        let start = std::time::Instant::now();
        let resp = http
            .get(format!("{base}/agents/n1/commands?wait=1&max=0"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let cmds: Vec<Command> = resp.json().await.unwrap();
        assert!(cmds.is_empty());
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(5),
            "{elapsed:?}"
        );
        server.abort();
    }
}
