// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::driver::DriverRegistry;
use crate::types::{new_machine, new_size, Host, Machine, MachineSpec, Size};
use axum::http::{header, StatusCode};
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{ListQuery, Phase};
use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;

#[test]
fn query_parses() {
    let q = serde_urlencoded_lite("limit=5&labels=env%3Dprod%2Ctier%3Dweb&name=a+b").unwrap();
    assert_eq!(q.limit, Some(5));
    assert_eq!(q.labels.as_deref(), Some("env=prod,tier=web"));
    assert_eq!(q.name.as_deref(), Some("a b"));
    assert!(serde_urlencoded_lite("limit=x").is_err());
}

#[test]
fn pagination_cuts_and_links() {
    let items: Vec<Size> = (0..5)
        .map(|i| {
            new_size(
                format!("s{i}"),
                pistack_types::Resources {
                    vcpus: 1,
                    memory_mib: 1,
                    disk_gib: 0,
                    gpus: 0,
                },
            )
        })
        .collect();
    let q = ListQuery {
        limit: Some(2),
        ..Default::default()
    };
    let p1 = paginate(items.clone(), &q);
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.total, 5);
    let next = p1.next.clone().unwrap();
    let p2 = paginate(
        items.clone(),
        &ListQuery {
            limit: Some(2),
            after: Some(next),
            ..Default::default()
        },
    );
    assert_eq!(p2.items.len(), 2);
    assert!(p1
        .items
        .iter()
        .all(|a| p2.items.iter().all(|b| a.meta.id != b.meta.id)));
    let p3 = paginate(
        items,
        &ListQuery {
            limit: Some(2),
            after: p2.next,
            ..Default::default()
        },
    );
    assert_eq!(p3.items.len(), 1);
    assert!(p3.next.is_none());
}

#[test]
fn names_validated() {
    assert!(validate_name("web-1").is_ok());
    assert!(validate_name("Web").is_err());
    assert!(validate_name("").is_err());
    assert!(validate_name("-x").is_err());
}

#[tokio::test]
async fn migrate_route_refuses_without_underlay_and_enqueues() {
    use crate::types::new_host;
    use axum::body::Body as HttpBody;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use pistack_rpc::proto::{CommandKind, MigrateRequest};
    use pistack_types::Resources;
    use tower::ServiceExt;

    let store = Store::in_memory().unwrap();
    let queue = CommandQueue::default();
    let cap = Resources {
        vcpus: 2,
        memory_mib: 2048,
        disk_gib: 20,
        gpus: 0,
    };
    // Both hosts keep machine disks on storage the other can see.
    let mut labels = BTreeMap::new();
    labels.insert(
        crate::types::DISK_LABEL.to_owned(),
        crate::types::DISK_SHARED.to_owned(),
    );
    let src = store
        .collection::<Host>()
        .create(new_host(
            "src",
            "node-src",
            cap,
            vec!["kvm".into()],
            labels.clone(),
        ))
        .unwrap();
    let mut bare = new_host("bare", "node-bare", cap, vec!["kvm".into()], labels.clone());
    bare.spec.underlay_ip = None;
    store.collection::<Host>().create(bare).unwrap();
    let mut dest = new_host("dest", "node-dest", cap, vec!["kvm".into()], labels);
    dest.spec.underlay_ip = Some("10.1.0.2".into());
    let dest = store.collection::<Host>().create(dest).unwrap();
    let mut m = new_machine(
        "web",
        None,
        MachineSpec {
            size: "s.1x1".into(),
            image: "ubuntu".into(),
            networks: vec!["tenant".into()],
            ..Default::default()
        },
    );
    m.status.host = Some(src.meta.id.clone());
    m.status.phase = Phase::Ready;
    let m = store.collection::<Machine>().create(m).unwrap();
    let id = m.meta.id.to_string();
    let app = router(AppState {
        store: store.clone(),
        drivers: Arc::new(DriverRegistry::new()),
        dev_admin: true,
        queue: Some(queue.clone()),
        strategy: Default::default(),
        samples: Arc::new(gauge::SampleRing::memory()),
        fencer: Arc::new(crate::maintain::BmcFencer::new(store.clone())),
    });

    async fn post(app: &axum::Router, path: &str, host: &str) -> (StatusCode, Vec<u8>) {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(HttpBody::from(
                serde_json::to_vec(&serde_json::json!({ "host": host })).unwrap(),
            ))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, bytes)
    }

    let (st, bytes) = post(&app, &format!("/machines/{id}/migrate"), "bare").await;
    assert_ne!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let still = store.collection::<Machine>().get(&m.meta.id).unwrap();
    assert_eq!(still.status.host.as_ref(), Some(&src.meta.id));
    assert_eq!(queue.pending_count(), 0);

    let (st, bytes) = post(&app, &format!("/machines/{id}/migrate"), "dest").await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let body: Machine = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.status.phase, Phase::Updating);
    assert_eq!(body.status.message, format!("migrating {}", dest.meta.id));
    assert_eq!(body.status.host.as_ref(), Some(&src.meta.id));
    let cmds = queue
        .next("node-src", std::time::Duration::from_secs(1), 4)
        .await;
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].kind, CommandKind::MigrateMachine);
    let req: MigrateRequest = serde_json::from_value(cmds[0].payload.clone()).unwrap();
    assert_eq!(req.dest, "qemu+ssh://10.1.0.2/system");
}
