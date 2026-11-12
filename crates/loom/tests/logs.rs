// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Pod logs are the text a host returns for `ReadLogs`.

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
use pistack_types::{Id, Meta, Resources};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;
use tower::ServiceExt;

fn pod(name: &str, machine: Option<Id>, host: Option<Id>) -> Pod {
    Pod {
        meta: Meta::new(loom::kinds::POD, name, None),
        spec: PodSpec {
            containers: vec![Container {
                name: "app".into(),
                image: "img".into(),
                ..Container::default()
            }],
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

fn queue() -> CommandQueue {
    CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_millis(400),
        capacity: 8,
        heartbeat_interval: Duration::from_secs(30),
        ovn: None,
    })
}

fn logs_app(store: Store, queue: Option<CommandQueue>) -> Router {
    router(AppState {
        store,
        dev_admin: true,
        queue,
    })
}

async fn get_logs(app: &Router, id: &Id) -> (StatusCode, Option<String>, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/pods/{id}/logs"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, content_type, bytes)
}

#[tokio::test]
async fn pod_logs_return_the_command_text() {
    let store = Store::in_memory().unwrap();
    let node = host("host-a", "node-a");
    store
        .collection::<forge::Host>()
        .create(node.clone())
        .unwrap();
    let machine = Id::new(pistack_types::kind::MACHINE);
    let row = pod("web", Some(machine.clone()), Some(node.meta.id.clone()));
    store.collection::<Pod>().create(row.clone()).unwrap();
    let q = queue();
    let app = logs_app(store.clone(), Some(q.clone()));

    let (tx, rx) = tokio::sync::oneshot::channel();
    let worker = q.clone();
    let expect = machine.to_string();
    tokio::spawn(async move {
        let cmds = worker.next("node-a", Duration::from_secs(2), 1).await;
        let cmd = cmds.into_iter().next().expect("read_logs command");
        let kind = cmd.kind;
        let payload = cmd.payload.clone();
        worker
            .ack(
                "node-a",
                CommandResult::ok(cmd.id).with_payload(json!({"text": "hello"})),
            )
            .unwrap();
        let _ = tx.send((kind, payload));
    });

    let (status, content_type, body) = get_logs(&app, &row.meta.id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("text/plain; charset=utf-8"));
    assert_eq!(body, b"hello");
    let (kind, payload) = rx.await.unwrap();
    assert_eq!(kind, CommandKind::ReadLogs);
    assert_eq!(payload, json!({"id": expect}));
    assert_eq!(
        store.collection::<Pod>().get(&row.meta.id).unwrap().status,
        row.status
    );

    let (tx, rx) = tokio::sync::oneshot::channel();
    let worker = q.clone();
    tokio::spawn(async move {
        let cmds = worker.next("node-a", Duration::from_secs(2), 1).await;
        let cmd = cmds.into_iter().next().expect("empty text command");
        worker
            .ack(
                "node-a",
                CommandResult::ok(cmd.id).with_payload(json!({"text": ""})),
            )
            .unwrap();
        let _ = tx.send(());
    });
    let (status, _, body) = get_logs(&app, &row.meta.id).await;
    rx.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());

    let worker = q.clone();
    tokio::spawn(async move {
        let cmds = worker.next("node-a", Duration::from_secs(2), 1).await;
        let cmd = cmds.into_iter().next().expect("failed command");
        worker
            .ack(
                "node-a",
                CommandResult::failed(cmd.id, "disk is not available"),
            )
            .unwrap();
    });
    let (status, _, _) = get_logs(&app, &row.meta.id).await;
    assert_ne!(status, StatusCode::OK);

    let worker = q.clone();
    tokio::spawn(async move {
        let cmds = worker.next("node-a", Duration::from_secs(2), 1).await;
        let cmd = cmds.into_iter().next().expect("missing text command");
        worker
            .ack("node-a", CommandResult::ok(cmd.id).with_payload(json!({})))
            .unwrap();
    });
    let (status, _, body) = get_logs(&app, &row.meta.id).await;
    assert_ne!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&body).contains("Ready"));

    let bare = logs_app(store.clone(), None);
    let (status, _, body) = get_logs(&bare, &row.meta.id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(String::from_utf8_lossy(&body).contains("pod logs are not configured"));

    let no_machine = pod("no-machine", None, Some(node.meta.id.clone()));
    store
        .collection::<Pod>()
        .create(no_machine.clone())
        .unwrap();
    let (status, _, _) = get_logs(&app, &no_machine.meta.id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let no_host = pod("no-host", Some(Id::new(pistack_types::kind::MACHINE)), None);
    store.collection::<Pod>().create(no_host.clone()).unwrap();
    let (status, _, _) = get_logs(&app, &no_host.meta.id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let missing_host = pod(
        "missing-host",
        Some(Id::new(pistack_types::kind::MACHINE)),
        Some(Id::new(pistack_types::kind::HOST)),
    );
    store
        .collection::<Pod>()
        .create(missing_host.clone())
        .unwrap();
    let (status, _, _) = get_logs(&app, &missing_host.meta.id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let blank = host("host-blank", "");
    store
        .collection::<forge::Host>()
        .create(blank.clone())
        .unwrap();
    let blank_pod = pod(
        "blank",
        Some(Id::new(pistack_types::kind::MACHINE)),
        Some(blank.meta.id.clone()),
    );
    store.collection::<Pod>().create(blank_pod.clone()).unwrap();
    let (status, _, _) = get_logs(&app, &blank_pod.meta.id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let cmds = q.next("node-a", Duration::from_millis(50), 4).await;
    assert!(
        cmds.is_empty(),
        "failed lookups must not leave a read_logs command"
    );

    let (status, _, _) = get_logs(&app, &row.meta.id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
