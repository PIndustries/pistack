// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client_type::Client;
use super::fabric_client::fabric_client;
use crate::types::*;
use pistack_store::Store;
use pistack_types::{kind, Id, Kind, Meta, Phase};

const DOC: &str = r#"
kind: network
name: app
spec: {}
---
kind: subnet
name: app-net
spec:
  network: app
  cidr: "10.20.0.0/24"
"#;

#[test]
fn apply_network_and_subnet_twice_does_not_duplicate() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let stack = client
        .create_stack(
            Meta::new(kind::STACK, "web", Some(project.clone())),
            StackSpec {
                name: "web".into(),
                document: DOC.into(),
                declared: Vec::new(),
                parameters: Default::default(),
            },
        )
        .unwrap();

    let first = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(first.status.phase, StackPhase::Ready);
    let nets = store
        .collection::<fabric::Network>()
        .list(Some(&project))
        .unwrap();
    let subs = store
        .collection::<fabric::Subnet>()
        .list(Some(&project))
        .unwrap();
    assert_eq!(nets.len(), 1);
    assert_eq!(subs.len(), 1);
    let net = nets.iter().find(|n| n.meta.name == "app").unwrap();
    let sub = subs.iter().find(|s| s.meta.name == "app-net").unwrap();
    assert_eq!(sub.spec.network, net.meta.id);
    assert!(sub.spec.gateway.is_none());
    assert!(sub.spec.allocation_pools.is_empty());
    assert!(sub.spec.enable_dhcp);
    assert_eq!(sub.spec.cidr.to_string(), "10.20.0.0/24");
    assert_eq!(
        first.status.created,
        vec![net.meta.id.to_string(), sub.meta.id.to_string()]
    );
    assert!(first.status.message.contains(&net.meta.id.to_string()));
    assert!(first.status.message.contains(&sub.meta.id.to_string()));

    let second = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(second.status.phase, StackPhase::Ready);
    assert!(second.status.created.is_empty(), "{:?}", second.status);
    assert!(second.status.message.contains("present"));
    assert!(!second.status.message.contains("created"));
    let nets2 = store
        .collection::<fabric::Network>()
        .list(Some(&project))
        .unwrap();
    let subs2 = store
        .collection::<fabric::Subnet>()
        .list(Some(&project))
        .unwrap();
    assert_eq!(nets2.len(), 1);
    assert_eq!(subs2.len(), 1);
    assert_eq!(nets2[0].meta.id, net.meta.id);
    assert_eq!(subs2[0].meta.id, sub.meta.id);
    assert_eq!(nets2[0].meta.revision, net.meta.revision);
    assert_eq!(subs2[0].meta.revision, sub.meta.revision);
}

/// One document: network, subnet, router, database machine, web pod,
/// listener, and DNS name. An unknown kind stays ignored.
const APP: &str = r#"
kind: network
name: app
spec: {}
---
kind: subnet
name: app-net
spec:
  network: app
  cidr: "10.20.0.0/24"
  gateway: "10.20.0.1"
---
kind: router
name: edge
spec:
  interfaces:
    - app-net
---
kind: machine
name: db
spec:
  size: s.1x1
  image: postgres
  networks:
    - app
---
kind: pod
name: web
spec:
  containers:
    - name: web
      image: web:1
  networks:
    - app
---
kind: listener
name: web-http
spec:
  balancer: web-http
  vip_network: app
  protocol: tcp
  port: 80
---
kind: dns
name: web.app.test
spec:
  zone: app.test
  name: web
  type: A
  records:
    - "10.20.0.10"
---
kind: share
name: data
spec: {}
"#;

fn stack(client: &Client, project: &Id, name: &str, document: &str) -> Stack {
    client
        .create_stack(
            Meta::new(kind::STACK, name, Some(project.clone())),
            StackSpec {
                name: name.into(),
                document: document.into(),
                declared: Vec::new(),
                parameters: Default::default(),
            },
        )
        .unwrap()
}

#[test]
fn apply_app_document_twice_keeps_one_row_of_each() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let stack = stack(&client, &project, "app", APP);

    let first = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(first.status.phase, StackPhase::Ready);
    assert!(first.status.message.contains("ignored"));
    assert!(first.status.message.contains("share"));

    let nets = store
        .collection::<fabric::Network>()
        .list(Some(&project))
        .unwrap();
    let subs = store
        .collection::<fabric::Subnet>()
        .list(Some(&project))
        .unwrap();
    let routers = store
        .collection::<fabric::Router>()
        .list(Some(&project))
        .unwrap();
    let machines = store
        .collection::<forge::Machine>()
        .list(Some(&project))
        .unwrap();
    let pods = store
        .collection::<loom::Pod>()
        .list(Some(&project))
        .unwrap();
    let listeners = store
        .collection::<relay::Listener>()
        .list(Some(&project))
        .unwrap();
    let records = store
        .collection::<beacon::RecordSet>()
        .list(Some(&project))
        .unwrap();
    let balancers = store
        .collection::<relay::Balancer>()
        .list(Some(&project))
        .unwrap();
    let zones = store
        .collection::<beacon::Zone>()
        .list(Some(&project))
        .unwrap();
    let ports = store
        .collection::<fabric::Port>()
        .list(Some(&project))
        .unwrap();

    assert_eq!(nets.len(), 1, "network");
    assert_eq!(subs.len(), 1, "subnet");
    assert_eq!(routers.len(), 1, "router");
    assert_eq!(machines.len(), 1, "machine");
    assert_eq!(pods.len(), 1, "pod");
    assert_eq!(listeners.len(), 1, "listener");
    assert_eq!(records.len(), 1, "dns");
    assert_eq!(balancers.len(), 1, "balancer behind the listener");
    assert_eq!(zones.len(), 1, "zone behind the dns name");
    assert_eq!(ports.len(), 1, "router interface");

    let net = &nets[0];
    let sub = &subs[0];
    let router = &routers[0];
    let machine = &machines[0];
    let pod = &pods[0];
    let listener = &listeners[0];
    let record = &records[0];
    assert_eq!(net.meta.name, "app");
    assert_eq!(sub.meta.name, "app-net");
    assert_eq!(sub.spec.network, net.meta.id);
    assert_eq!(router.meta.name, "edge");
    assert_eq!(router.spec.interfaces, vec![sub.meta.id.clone()]);
    assert_eq!(machine.meta.name, "db");
    assert_eq!(machine.spec.image, "postgres");
    assert_eq!(machine.spec.networks, vec!["app".to_string()]);
    assert_eq!(machine.status.phase, Phase::Pending);
    assert_eq!(pod.meta.name, "web");
    assert_eq!(pod.spec.containers[0].image, "web:1");
    assert_eq!(pod.status.phase, loom::PodPhase::Pending);
    assert_eq!(listener.meta.name, "web-http");
    assert_eq!(listener.spec.port, 80);
    assert_eq!(listener.spec.protocol, relay::ListenerProtocol::Tcp);
    assert_eq!(listener.spec.balancer, balancers[0].meta.id);
    assert_eq!(listener.status.phase, Phase::Pending);
    assert_eq!(record.meta.name, "web.app.test");
    assert_eq!(record.spec.name, "web.app.test.");
    assert_eq!(record.spec.kind, beacon::RecordType::A);
    assert_eq!(record.spec.records, vec!["10.20.0.10".to_string()]);
    assert_eq!(record.spec.zone, zones[0].meta.id);
    assert_eq!(record.status.phase, Phase::Pending);
    assert_eq!(
        first.status.created,
        vec![
            net.meta.id.to_string(),
            sub.meta.id.to_string(),
            router.meta.id.to_string(),
            machine.meta.id.to_string(),
            pod.meta.id.to_string(),
            listener.meta.id.to_string(),
            record.meta.id.to_string(),
        ]
    );

    let second = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(second.status.phase, StackPhase::Ready);
    assert!(second.status.created.is_empty(), "{:?}", second.status);
    assert!(second.status.message.contains("present"));
    assert!(second.status.message.contains("ignored"));
    assert!(!second.status.message.contains("created"));

    assert_eq!(
        store
            .collection::<fabric::Network>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<fabric::Subnet>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<fabric::Router>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<forge::Machine>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<loom::Pod>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<relay::Listener>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<beacon::RecordSet>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<relay::Balancer>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<beacon::Zone>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<fabric::Port>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );

    let net2 = store
        .collection::<fabric::Network>()
        .get(&net.meta.id)
        .unwrap();
    let sub2 = store
        .collection::<fabric::Subnet>()
        .get(&sub.meta.id)
        .unwrap();
    let router2 = store
        .collection::<fabric::Router>()
        .get(&router.meta.id)
        .unwrap();
    let machine2 = store
        .collection::<forge::Machine>()
        .get(&machine.meta.id)
        .unwrap();
    let pod2 = store.collection::<loom::Pod>().get(&pod.meta.id).unwrap();
    let listener2 = store
        .collection::<relay::Listener>()
        .get(&listener.meta.id)
        .unwrap();
    let record2 = store
        .collection::<beacon::RecordSet>()
        .get(&record.meta.id)
        .unwrap();
    assert_eq!(net2.meta.revision, net.meta.revision);
    assert_eq!(sub2.meta.revision, sub.meta.revision);
    assert_eq!(router2.meta.revision, router.meta.revision);
    assert_eq!(machine2.meta.revision, machine.meta.revision);
    assert_eq!(pod2.meta.revision, pod.meta.revision);
    assert_eq!(listener2.meta.revision, listener.meta.revision);
    assert_eq!(record2.meta.revision, record.meta.revision);
}

#[test]
fn kit_apply_uses_realize_and_does_not_duplicate() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let document =
        "kind: network\nname: kit-net\nspec: {}\n---\nkind: router\nname: kit-edge\nspec: {}\n";
    let kit = client
        .create_kit(
            Meta::new(KIT_KIND, "base", Some(project.clone())),
            KitSpec {
                name: "base".into(),
                document: document.into(),
                declared: Vec::new(),
                parameters: Default::default(),
            },
        )
        .unwrap();
    assert_eq!(kit.status.message, crate::types::ACCEPTED_MESSAGE);

    let first = client.apply_kit(&kit.meta.id).unwrap();
    assert_eq!(first.status.created.len(), 2, "{:?}", first.status);
    assert_eq!(
        store
            .collection::<fabric::Network>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<fabric::Router>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );

    let second = client.apply_kit(&kit.meta.id).unwrap();
    assert!(second.status.created.is_empty(), "{:?}", second.status);
    assert!(second.status.message.contains("present"));
    assert_eq!(
        store
            .collection::<fabric::Network>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .collection::<fabric::Router>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn workflow_applies_stacks_in_order_and_stops_on_error() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    stack(
        &client,
        &project,
        "first",
        "kind: network\nname: app\nspec: {}\n",
    );
    stack(
        &client,
        &project,
        "bad",
        "kind: machine\nname: broken\nspec: {}\n",
    );
    stack(
        &client,
        &project,
        "later",
        "kind: network\nname: later-net\nspec: {}\n",
    );
    let wf = client
        .create_workflow(
            Meta::new(WORKFLOW_KIND, "rollout", Some(project.clone())),
            WorkflowSpec {
                name: "rollout".into(),
                stacks: vec!["first".into(), "bad".into(), "later".into()],
            },
        )
        .unwrap();
    assert_eq!(wf.status.message, crate::types::WORKFLOW_ACCEPTED_MESSAGE);

    let err = client.apply_workflow(&wf.meta.id).unwrap_err();
    assert!(err.message.contains("stopped on stack bad"), "{err}");
    assert!(err.message.contains("size and image"), "{err}");

    let stored = client.get_workflow(&wf.meta.id).unwrap();
    assert_eq!(stored.status.phase, WorkflowPhase::Failed);
    assert_eq!(stored.status.applied, vec!["first".to_string()]);

    let nets = store
        .collection::<fabric::Network>()
        .list(Some(&project))
        .unwrap();
    assert_eq!(nets.len(), 1);
    assert_eq!(nets[0].meta.name, "app");
    assert!(store
        .collection::<forge::Machine>()
        .list(Some(&project))
        .unwrap()
        .is_empty());

    let again = client
        .create_workflow(
            Meta::new(WORKFLOW_KIND, "again", Some(project.clone())),
            WorkflowSpec {
                name: "again".into(),
                stacks: vec!["first".into()],
            },
        )
        .unwrap();
    let done = client.apply_workflow(&again.meta.id).unwrap();
    assert_eq!(done.status.phase, WorkflowPhase::Accepted);
    assert_eq!(done.status.applied, vec!["first".to_string()]);
    assert!(done.status.message.contains("applied first"));
    assert_eq!(
        store
            .collection::<fabric::Network>()
            .list(Some(&project))
            .unwrap()
            .len(),
        1
    );
}

/// Knobs for a document that names one of each kind.
struct App {
    mtu: u16,
    db: &'static str,
    web: &'static str,
    port: u16,
    record: &'static str,
    extra: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            mtu: 1500,
            db: "postgres",
            web: "web:1",
            port: 80,
            record: "10.20.0.10",
            extra: false,
        }
    }
}

impl App {
    fn doc(&self) -> String {
        let App {
            mtu,
            db,
            web,
            port,
            record,
            extra,
        } = self;
        let mut doc = format!(
            r#"
kind: network
name: app
spec:
  mtu: {mtu}
---
kind: subnet
name: app-net
spec:
  network: app
  cidr: "10.20.0.0/24"
  gateway: "10.20.0.1"
---
kind: router
name: edge
spec:
  interfaces:
    - app-net
---
kind: machine
name: db
spec:
  size: s.1x1
  image: {db}
  networks:
    - app
---
kind: pod
name: web
spec:
  containers:
    - name: web
      image: "{web}"
  networks:
    - app
---
kind: listener
name: web-http
spec:
  balancer: web-http
  vip_network: app
  protocol: tcp
  port: {port}
---
kind: dns
name: web.app.test
spec:
  zone: app.test
  name: web
  type: A
  records:
    - "{record}"
"#
        );
        if *extra {
            doc.push_str("---\nkind: network\nname: extra\nspec: {}\n");
        }
        doc
    }
}

fn named<K: Kind>(store: &Store, project: &Id, name: &str) -> K {
    store
        .collection::<K>()
        .get_by_name(Some(project), name)
        .unwrap()
}

fn set_document(client: &Client, id: &Id, document: String) -> Stack {
    client
        .update_stack(id, None, |s| {
            s.spec.document = document.clone();
            Ok(())
        })
        .unwrap()
}

#[test]
fn apply_after_update_changes_managed_rows_and_records_a_revision() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let stack = stack(&client, &project, "app", &App::default().doc());

    let first = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(first.status.phase, StackPhase::Ready);
    assert_eq!(first.status.revision, 1);
    assert_eq!(first.status.history.len(), 1);
    assert_eq!(first.status.managed, first.status.created);
    assert_eq!(first.status.managed.len(), 7);

    let net: fabric::Network = named(&store, &project, "app");
    let machine: forge::Machine = named(&store, &project, "db");
    let pod: loom::Pod = named(&store, &project, "web");
    let listener: relay::Listener = named(&store, &project, "web-http");
    let record: beacon::RecordSet = named(&store, &project, "web.app.test");
    let sub: fabric::Subnet = named(&store, &project, "app-net");

    let v2 = App {
        mtu: 1400,
        db: "postgres-16",
        web: "web:2",
        port: 8080,
        record: "10.20.0.11",
        extra: true,
    };
    let edited = set_document(&client, &stack.meta.id, v2.doc());
    assert_eq!(edited.status.revision, 1, "PUT stores, apply realizes");
    assert_eq!(edited.status.phase, StackPhase::Ready);

    let second = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(second.status.phase, StackPhase::Ready, "{second:?}");
    assert_eq!(second.status.revision, 2);
    assert_eq!(second.status.history.len(), 2);
    assert_eq!(second.status.history[1].spec.document, v2.doc());
    assert_eq!(second.status.history[1].restored_from, None);
    let extra: fabric::Network = named(&store, &project, "extra");
    assert_eq!(second.status.created, vec![extra.meta.id.to_string()]);
    assert_eq!(
        second.status.updated,
        vec![
            net.meta.id.to_string(),
            machine.meta.id.to_string(),
            pod.meta.id.to_string(),
            listener.meta.id.to_string(),
            record.meta.id.to_string(),
        ]
    );
    assert!(second.status.message.contains("updated"), "{second:?}");
    assert!(second.status.managed.contains(&extra.meta.id.to_string()));

    let net2: fabric::Network = named(&store, &project, "app");
    let machine2: forge::Machine = named(&store, &project, "db");
    let pod2: loom::Pod = named(&store, &project, "web");
    let listener2: relay::Listener = named(&store, &project, "web-http");
    let record2: beacon::RecordSet = named(&store, &project, "web.app.test");
    let sub2: fabric::Subnet = named(&store, &project, "app-net");
    assert_eq!(net2.meta.id, net.meta.id, "updated in place");
    assert_eq!(net2.spec.mtu, 1400);
    assert_eq!(machine2.spec.image, "postgres-16");
    assert_eq!(pod2.spec.containers[0].image, "web:2");
    assert_eq!(listener2.spec.port, 8080);
    assert_eq!(listener2.spec.balancer, listener.spec.balancer);
    assert_eq!(record2.spec.records, vec!["10.20.0.11".to_string()]);
    assert_eq!(record2.spec.name, "web.app.test.");
    assert_eq!(sub2.meta.revision, sub.meta.revision, "unchanged row");

    // Re-applying the same spec writes nothing and keeps the revision.
    let third = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(third.status.revision, 2);
    assert_eq!(third.status.history.len(), 2);
    assert!(third.status.created.is_empty());
    assert!(third.status.updated.is_empty(), "{third:?}");
    let net3: fabric::Network = named(&store, &project, "app");
    assert_eq!(net3.meta.revision, net2.meta.revision);
}

#[test]
fn rollback_restores_an_earlier_revision_and_re_realizes_it() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let v1 = App::default();
    let v2 = App {
        mtu: 1400,
        web: "web:2",
        port: 8080,
        record: "10.20.0.11",
        extra: true,
        ..App::default()
    };
    let stack = stack(&client, &project, "app", &v1.doc());
    let err = client.rollback_stack(&stack.meta.id, None).unwrap_err();
    assert!(err.message.contains("no previous revision"), "{err}");

    client.apply_stack(&stack.meta.id).unwrap();
    set_document(&client, &stack.meta.id, v2.doc());
    client.apply_stack(&stack.meta.id).unwrap();
    let extra: fabric::Network = named(&store, &project, "extra");

    let back = client.rollback_stack(&stack.meta.id, None).unwrap();
    assert_eq!(back.status.phase, StackPhase::Ready, "{back:?}");
    assert_eq!(back.spec.document, v1.doc());
    assert_eq!(back.status.revision, 3);
    assert_eq!(back.status.history.len(), 3);
    assert_eq!(back.status.history[2].restored_from, Some(1));
    assert_eq!(back.status.history[2].spec, back.status.history[0].spec);
    assert!(back.status.left.is_empty(), "{back:?}");
    assert!(back.status.message.contains("deleted"), "{back:?}");
    assert!(
        back.status.message.contains(&extra.meta.id.to_string()),
        "{back:?}"
    );
    assert!(
        !back.status.managed.contains(&extra.meta.id.to_string()),
        "deleted managed ids leave ownership"
    );

    let net: fabric::Network = named(&store, &project, "app");
    let pod: loom::Pod = named(&store, &project, "web");
    let listener: relay::Listener = named(&store, &project, "web-http");
    let record: beacon::RecordSet = named(&store, &project, "web.app.test");
    assert_eq!(net.spec.mtu, 1500);
    assert_eq!(pod.spec.containers[0].image, "web:1");
    assert_eq!(listener.spec.port, 80);
    assert_eq!(record.spec.records, vec!["10.20.0.10".to_string()]);
    assert!(
        store
            .collection::<fabric::Network>()
            .get(&extra.meta.id)
            .is_err(),
        "rows dropped from the document are deleted"
    );

    let forward = client.rollback_stack(&stack.meta.id, Some(2)).unwrap();
    assert_eq!(forward.status.revision, 4);
    assert_eq!(forward.status.history[3].restored_from, Some(2));
    let listener: relay::Listener = named(&store, &project, "web-http");
    assert_eq!(listener.spec.port, 8080);
    assert!(forward.status.left.is_empty());
    // Rolling forward recreates the extra network.
    let extra_again: fabric::Network = named(&store, &project, "extra");
    assert!(forward
        .status
        .managed
        .contains(&extra_again.meta.id.to_string()));

    let err = client.rollback_stack(&stack.meta.id, Some(99)).unwrap_err();
    assert!(err.message.contains("revision 99 not found"), "{err}");
    let err = client.rollback_stack(&stack.meta.id, Some(4)).unwrap_err();
    assert!(err.message.contains("already the stack's spec"), "{err}");
}

#[test]
fn failed_update_is_marked_failed_and_rollback_returns_to_the_last_good_spec() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let stack = stack(&client, &project, "web", DOC);
    let good = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(good.status.revision, 1);

    // A subnet's cidr is fixed once it exists; fabric refuses the change.
    let bad = DOC.replace("10.20.0.0/24", "10.30.0.0/24");
    set_document(&client, &stack.meta.id, bad);
    let err = client.apply_stack(&stack.meta.id).unwrap_err();
    assert!(err.message.contains("subnet app-net"), "{err}");
    assert!(err.message.contains("immutable"), "{err}");

    let failed = client.get_stack(&stack.meta.id).unwrap();
    assert_eq!(failed.status.phase, StackPhase::Failed);
    assert!(
        failed.status.message.starts_with("apply failed:"),
        "{failed:?}"
    );
    assert_eq!(failed.status.revision, 1, "a failed apply records nothing");
    assert_eq!(failed.status.history.len(), 1);
    assert_eq!(failed.status.managed, good.status.managed);

    let back = client.rollback_stack(&stack.meta.id, None).unwrap();
    assert_eq!(back.status.phase, StackPhase::Ready);
    assert_eq!(back.spec.document, DOC);
    assert_eq!(back.status.revision, 1, "same spec as the newest revision");
    assert_eq!(back.status.history.len(), 1);
    let sub: fabric::Subnet = named(&store, &project, "app-net");
    assert_eq!(sub.spec.cidr.to_string(), "10.20.0.0/24");
}

#[test]
fn machine_image_is_fixed_once_the_machine_leaves_pending() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let doc = "kind: machine\nname: db\nspec:\n  size: s.1x1\n  image: postgres\n";
    let stack = stack(&client, &project, "db", doc);
    client.apply_stack(&stack.meta.id).unwrap();
    let machine: forge::Machine = named(&store, &project, "db");
    store
        .collection::<forge::Machine>()
        .modify(&machine.meta.id, |m| {
            m.status.phase = Phase::Ready;
            m.spec.restarts = 2;
            Ok(())
        })
        .unwrap();

    // Changing only networks is allowed and keeps the reboot counter.
    set_document(&client, &stack.meta.id, format!("{doc}  networks: [app]\n"));
    let ok = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(ok.status.updated, vec![machine.meta.id.to_string()]);
    let machine: forge::Machine = named(&store, &project, "db");
    assert_eq!(machine.spec.networks, vec!["app".to_string()]);
    assert_eq!(machine.spec.restarts, 2);

    set_document(&client, &stack.meta.id, doc.replace("postgres", "mysql"));
    let err = client.apply_stack(&stack.meta.id).unwrap_err();
    assert!(err
        .message
        .contains("only change while the machine is pending"));
    let failed = client.get_stack(&stack.meta.id).unwrap();
    assert_eq!(failed.status.phase, StackPhase::Failed);
    let machine: forge::Machine = named(&store, &project, "db");
    assert_eq!(machine.spec.image, "postgres");
}

#[test]
fn drop_on_update_deletes_managed_rows_in_dependency_order() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let full = App {
        extra: true,
        ..App::default()
    };
    let stack = stack(&client, &project, "app", &full.doc());
    let first = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(first.status.phase, StackPhase::Ready);
    let extra: fabric::Network = named(&store, &project, "extra");
    let record: beacon::RecordSet = named(&store, &project, "web.app.test");
    let listener: relay::Listener = named(&store, &project, "web-http");
    let pod: loom::Pod = named(&store, &project, "web");
    let machine: forge::Machine = named(&store, &project, "db");
    let router: fabric::Router = named(&store, &project, "edge");
    let sub: fabric::Subnet = named(&store, &project, "app-net");
    let net: fabric::Network = named(&store, &project, "app");

    // Keep only the base network: every other managed row is an orphan.
    set_document(
        &client,
        &stack.meta.id,
        "kind: network\nname: app\nspec:\n  mtu: 1500\n".into(),
    );
    let dropped = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(dropped.status.phase, StackPhase::Ready, "{dropped:?}");
    assert!(dropped.status.left.is_empty(), "{dropped:?}");
    assert!(dropped.status.message.contains("deleted"), "{dropped:?}");
    assert_eq!(dropped.status.managed, vec![net.meta.id.to_string()]);
    // Sync deletes remove the row; async kinds are annotated instead.
    assert!(store
        .collection::<beacon::RecordSet>()
        .get(&record.meta.id)
        .is_err());
    assert!(store
        .collection::<relay::Listener>()
        .get(&listener.meta.id)
        .is_err());
    assert!(store
        .collection::<fabric::Router>()
        .get(&router.meta.id)
        .is_err());
    assert!(store
        .collection::<fabric::Subnet>()
        .get(&sub.meta.id)
        .is_err());
    assert!(store
        .collection::<fabric::Network>()
        .get(&extra.meta.id)
        .is_err());
    let pod_after = store.collection::<loom::Pod>().get(&pod.meta.id).unwrap();
    assert!(loom::delete_requested(&pod_after), "pod marked for delete");
    let machine_after = store
        .collection::<forge::Machine>()
        .get(&machine.meta.id)
        .unwrap();
    assert!(
        forge::delete_requested(&machine_after),
        "machine marked for delete"
    );
    let kept: fabric::Network = named(&store, &project, "app");
    assert_eq!(kept.meta.id, net.meta.id);
}

#[test]
fn drop_on_rollback_deletes_the_extra_managed_network() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let v1 = App::default();
    let v2 = App {
        extra: true,
        ..App::default()
    };
    let stack = stack(&client, &project, "app", &v1.doc());
    client.apply_stack(&stack.meta.id).unwrap();
    set_document(&client, &stack.meta.id, v2.doc());
    client.apply_stack(&stack.meta.id).unwrap();
    let extra: fabric::Network = named(&store, &project, "extra");
    assert!(store
        .collection::<fabric::Network>()
        .get(&extra.meta.id)
        .is_ok());

    let back = client.rollback_stack(&stack.meta.id, None).unwrap();
    assert_eq!(back.status.phase, StackPhase::Ready, "{back:?}");
    assert!(back.status.left.is_empty(), "{back:?}");
    assert!(back.status.message.contains("deleted"), "{back:?}");
    assert!(!back.status.managed.contains(&extra.meta.id.to_string()));
    assert!(
        store
            .collection::<fabric::Network>()
            .get(&extra.meta.id)
            .is_err(),
        "extra network removed on rollback"
    );
}

#[test]
fn rows_the_stack_did_not_create_are_left_alone() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let shared = fabric_client(&store)
        .create_network(
            Meta::new(kind::NETWORK, "shared", Some(project.clone())),
            fabric::NetworkSpec::default(),
        )
        .unwrap();
    let stack = stack(
        &client,
        &project,
        "uses-shared",
        "kind: network\nname: shared\nspec:\n  mtu: 1400\n",
    );
    let applied = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(applied.status.phase, StackPhase::Ready);
    assert!(applied.status.updated.is_empty());
    assert!(applied.status.managed.is_empty());
    assert!(applied.status.message.contains("present"));
    let after: fabric::Network = named(&store, &project, "shared");
    assert_eq!(after.spec.mtu, 1500);
    assert_eq!(after.meta.revision, shared.meta.revision);
}

#[test]
fn status_from_before_managed_existed_still_updates_created_rows() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let stack = stack(
        &client,
        &project,
        "old",
        "kind: network\nname: app\nspec: {}\n",
    );
    client.apply_stack(&stack.meta.id).unwrap();
    client
        .stacks()
        .modify(&stack.meta.id, |s| {
            s.status.managed.clear();
            s.status.history.clear();
            s.status.revision = 0;
            Ok(())
        })
        .unwrap();
    set_document(
        &client,
        &stack.meta.id,
        "kind: network\nname: app\nspec:\n  mtu: 1400\n".into(),
    );
    let applied = client.apply_stack(&stack.meta.id).unwrap();
    assert_eq!(applied.status.updated.len(), 1, "{applied:?}");
    assert_eq!(applied.status.revision, 1);
    let net: fabric::Network = named(&store, &project, "app");
    assert_eq!(net.spec.mtu, 1400);
}

#[test]
fn history_keeps_the_newest_revisions() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let stack = stack(
        &client,
        &project,
        "many",
        "kind: network\nname: app\nspec: {}\n",
    );
    let total = REVISION_HISTORY_LIMIT as u64 + 2;
    for i in 0..total {
        let mtu = 1300 + i;
        set_document(
            &client,
            &stack.meta.id,
            format!("kind: network\nname: app\nspec:\n  mtu: {mtu}\n"),
        );
        client.apply_stack(&stack.meta.id).unwrap();
    }
    let done = client.get_stack(&stack.meta.id).unwrap();
    assert_eq!(done.status.revision, total);
    assert_eq!(done.status.history.len(), REVISION_HISTORY_LIMIT);
    assert_eq!(done.status.history[0].revision, 3);
    let err = client.rollback_stack(&stack.meta.id, Some(1)).unwrap_err();
    assert!(err.message.contains("revision 1 not found"), "{err}");
}

#[test]
fn update_stack_rejects_a_bad_document_and_a_stale_revision() {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone());
    let project = Id::new(kind::PROJECT);
    let stack = stack(&client, &project, "web", DOC);
    let err = client
        .update_stack(&stack.meta.id, None, |s| {
            s.spec.document = "  ".into();
            Ok(())
        })
        .unwrap_err();
    assert!(err.message.contains("empty"), "{err}");
    let err = client
        .update_stack(&stack.meta.id, Some(stack.meta.revision + 5), |_| Ok(()))
        .unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Conflict, "{err}");
    let same = client.get_stack(&stack.meta.id).unwrap();
    assert_eq!(same.spec.document, DOC);
}
