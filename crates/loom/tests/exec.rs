// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Pod exec is a scoped request that runs on the host holding the pod.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use loom::http::{router, AppState};
use loom::{Container, Pod, PodSpec, PodStatus};
use pistack_rpc::proto::{CommandKind, CommandResult};
use pistack_rpc::{CommandQueue, QueueConfig};
use pistack_store::Store;
use pistack_types::{Caller, Id, Meta, Resources};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;
use tower::ServiceExt;

fn pod(
    name: &str,
    project: Option<Id>,
    machine: Option<Id>,
    host: Option<Id>,
    containers: &[&str],
) -> Pod {
    Pod {
        meta: Meta::new(loom::kinds::POD, name, project),
        spec: PodSpec {
            containers: containers
                .iter()
                .map(|name| Container {
                    name: (*name).into(),
                    image: "img".into(),
                    ..Container::default()
                })
                .collect(),
            ..PodSpec::default()
        },
        status: PodStatus {
            machine,
            host,
            ..PodStatus::default()
        },
    }
}

fn host(name: &str, node: &str) -> forge::Host {
    forge::new_host(
        name,
        node,
        Resources {
            vcpus: 2,
            memory_mib: 1024,
            disk_gib: 10,
            gpus: 0,
        },
        Vec::new(),
        BTreeMap::new(),
    )
}

fn caller(project: Option<Id>, system: bool) -> Caller {
    Caller {
        principal: Id::new(pistack_types::kind::PRINCIPAL),
        principal_name: "member".into(),
        project,
        roles: Vec::new(),
        system,
    }
}

fn queue() -> CommandQueue {
    CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(2),
        capacity: 8,
        heartbeat_interval: Duration::from_secs(30),
        ovn: None,
    })
}

fn app(store: Store, queue: Option<CommandQueue>) -> Router {
    router(AppState {
        store,
        dev_admin: false,
        queue,
    })
}

async fn post_exec(
    app: &Router,
    id: &Id,
    body: Value,
    caller: Option<Caller>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/pods/{id}/exec"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    if let Some(caller) = caller {
        req.extensions_mut().insert(caller);
    }
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes).into_owned()}));
    (status, value)
}

fn message(body: &Value) -> &str {
    body["error"]["message"].as_str().unwrap_or("")
}

#[tokio::test]
async fn pod_exec_is_scoped_and_routed_to_the_host() {
    let store = Store::in_memory().unwrap();
    let node = host("host-a", "node-a");
    let other = host("host-b", "node-b");
    store
        .collection::<forge::Host>()
        .create(node.clone())
        .unwrap();
    store
        .collection::<forge::Host>()
        .create(other.clone())
        .unwrap();
    let project = Id::new(pistack_types::kind::PROJECT);
    let elsewhere = Id::new(pistack_types::kind::PROJECT);
    let machine = Id::new(pistack_types::kind::MACHINE);
    let row = pod(
        "web",
        Some(project.clone()),
        Some(machine.clone()),
        Some(node.meta.id.clone()),
        &["app", "side"],
    );
    store.collection::<Pod>().create(row.clone()).unwrap();
    let q = queue();
    let api = app(store.clone(), Some(q.clone()));
    let member = caller(Some(project.clone()), false);
    let outsider = caller(Some(elsewhere), false);
    let admin = caller(None, true);

    let (status, body) = post_exec(
        &api,
        &row.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(message(&body).contains("no credentials"), "{body}");

    let (status, body) = post_exec(
        &api,
        &row.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        Some(outsider),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(message(&body), format!("pod '{}' not found", row.meta.id));

    let (status, body) = post_exec(
        &api,
        &row.meta.id,
        json!({"container": "nope", "command": ["/bin/true"]}),
        Some(member.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(message(&body).contains("nope"), "{body}");

    let (status, body) = post_exec(
        &api,
        &row.meta.id,
        json!({"container": "app", "command": []}),
        Some(member.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(message(&body).contains("empty"), "{body}");

    let (status, body) = post_exec(
        &api,
        &row.meta.id,
        json!({"container": "", "command": ["/bin/true"]}),
        Some(member.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    for timeout in [0, 3601] {
        let (status, body) = post_exec(
            &api,
            &row.meta.id,
            json!({"container": "app", "command": ["/bin/true"], "timeout_secs": timeout}),
            Some(member.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{timeout} {body}");
        assert!(message(&body).contains("timeout_secs"), "{body}");
    }

    assert!(
        q.next("node-a", Duration::from_millis(40), 4)
            .await
            .is_empty(),
        "rejected exec must not be queued"
    );
    assert!(q
        .next("node-b", Duration::from_millis(40), 4)
        .await
        .is_empty());

    let (tx, rx) = tokio::sync::oneshot::channel();
    let worker = q.clone();
    tokio::spawn(async move {
        let cmds = worker.next("node-a", Duration::from_secs(2), 1).await;
        let cmd = cmds.into_iter().next().expect("exec command");
        let kind = cmd.kind;
        let payload = cmd.payload.clone();
        worker
            .ack(
                "node-a",
                CommandResult::ok(cmd.id).with_payload(json!({
                    "exit_code": 7,
                    "stdout": "out\n",
                    "stderr": "err\n"
                })),
            )
            .unwrap();
        let _ = tx.send((kind, payload));
    });
    let (status, body) = post_exec(
        &api,
        &row.meta.id,
        json!({
            "container": "app",
            "command": ["/bin/echo", "hi"],
            "timeout_secs": 5,
            "env": [{"name": "MODE", "value": "prod"}],
            "cwd": "/work"
        }),
        Some(member.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["exit_code"], 7);
    assert_eq!(body["stdout"], "out\n");
    assert_eq!(body["stderr"], "err\n");
    let (kind, payload) = rx.await.unwrap();
    assert_eq!(kind, CommandKind::Exec);
    assert_eq!(payload["id"], machine.to_string());
    assert_eq!(payload["container"], "app");
    assert_eq!(payload["command"], json!(["/bin/echo", "hi"]));
    assert_eq!(payload["timeout_secs"], 5);
    assert_eq!(payload["cwd"], "/work");
    assert_eq!(payload["env"], json!([{"name": "MODE", "value": "prod"}]));
    assert_eq!(
        store.collection::<Pod>().get(&row.meta.id).unwrap().status,
        row.status
    );
    assert!(q
        .next("node-b", Duration::from_millis(40), 4)
        .await
        .is_empty());

    let (tx, rx) = tokio::sync::oneshot::channel();
    let worker = q.clone();
    tokio::spawn(async move {
        let cmds = worker.next("node-a", Duration::from_secs(2), 1).await;
        let cmd = cmds.into_iter().next().expect("admin exec");
        let payload = cmd.payload.clone();
        worker
            .ack(
                "node-a",
                CommandResult::ok(cmd.id).with_payload(json!({
                    "exit_code": 0,
                    "stdout": "",
                    "stderr": ""
                })),
            )
            .unwrap();
        let _ = tx.send(payload);
    });
    let (status, body) = post_exec(
        &api,
        &row.meta.id,
        json!({"container": "side", "command": ["/bin/true"]}),
        Some(admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["exit_code"], 0);
    let payload = rx.await.unwrap();
    assert_eq!(payload["container"], "side");
    assert!(payload.get("timeout_secs").is_none(), "{payload}");

    let worker = q.clone();
    tokio::spawn(async move {
        let cmds = worker.next("node-a", Duration::from_secs(2), 1).await;
        let cmd = cmds.into_iter().next().expect("failed exec");
        worker
            .ack(
                "node-a",
                CommandResult::failed(cmd.id, "disk is not available"),
            )
            .unwrap();
    });
    let (status, _) = post_exec(
        &api,
        &row.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        Some(member.clone()),
    )
    .await;
    assert_ne!(status, StatusCode::OK);

    let worker = q.clone();
    tokio::spawn(async move {
        let cmds = worker.next("node-a", Duration::from_secs(2), 1).await;
        let cmd = cmds.into_iter().next().expect("missing result");
        worker
            .ack(
                "node-a",
                CommandResult::ok(cmd.id).with_payload(json!({"exit_code": 0})),
            )
            .unwrap();
    });
    let (status, body) = post_exec(
        &api,
        &row.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        Some(member.clone()),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(
        message(&body).contains("pod exec result is missing"),
        "{body}"
    );

    let bare = app(store.clone(), None);
    let (status, body) = post_exec(
        &bare,
        &row.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        Some(member.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        message(&body).contains("pod exec is not configured"),
        "{body}"
    );

    let no_machine = pod(
        "no-machine",
        Some(project.clone()),
        None,
        Some(node.meta.id.clone()),
        &["app"],
    );
    store
        .collection::<Pod>()
        .create(no_machine.clone())
        .unwrap();
    let (status, body) = post_exec(
        &api,
        &no_machine.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        Some(member.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        message(&body).contains("pod exec is not available"),
        "{body}"
    );

    let no_host = pod(
        "no-host",
        Some(project.clone()),
        Some(Id::new(pistack_types::kind::MACHINE)),
        None,
        &["app"],
    );
    store.collection::<Pod>().create(no_host.clone()).unwrap();
    let (status, _) = post_exec(
        &api,
        &no_host.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        Some(member.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let missing_host = pod(
        "missing-host",
        Some(project.clone()),
        Some(Id::new(pistack_types::kind::MACHINE)),
        Some(Id::new(pistack_types::kind::HOST)),
        &["app"],
    );
    store
        .collection::<Pod>()
        .create(missing_host.clone())
        .unwrap();
    let (status, body) = post_exec(
        &api,
        &missing_host.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        Some(member.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(message(&body).contains("not found"), "{body}");

    let blank = host("host-blank", "");
    store
        .collection::<forge::Host>()
        .create(blank.clone())
        .unwrap();
    let blank_pod = pod(
        "blank",
        Some(project),
        Some(Id::new(pistack_types::kind::MACHINE)),
        Some(blank.meta.id.clone()),
        &["app"],
    );
    store.collection::<Pod>().create(blank_pod.clone()).unwrap();
    let (status, body) = post_exec(
        &api,
        &blank_pod.meta.id,
        json!({"container": "app", "command": ["/bin/true"]}),
        Some(member),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        message(&body).contains("pod exec is not available"),
        "{body}"
    );
    assert!(!message(&body).contains("not configured"), "{body}");

    assert!(q
        .next("node-a", Duration::from_millis(40), 4)
        .await
        .is_empty());
    assert!(q
        .next("node-b", Duration::from_millis(40), 4)
        .await
        .is_empty());
}
