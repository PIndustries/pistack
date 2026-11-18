// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

use axum::http::{header, StatusCode};
use axum::Router;
use fabric::{FloatingIp, FloatingIpSpec, NetworkSpec, Port, PortSpec, SubnetSpec};
use pistack_rpc::{MetadataNetwork, MetadataQuery};
use pistack_store::Store;
use pistack_types::{kind, Id, Meta};

use crate::types::{Host, Machine};

use super::*;
use crate::types::{new_machine, MachineSpec};
use std::net::Ipv4Addr;
use std::sync::Arc;

#[test]
fn listen_stays_off_the_unspecified_addresses() {
    assert!(parse_listen("127.0.0.1:18880").is_ok());
    assert!(parse_listen("0.0.0.0:18880").is_err());
    assert!(parse_listen("[::]:18880").is_err());
    assert!(parse_listen("metadata").is_err());
}

#[test]
fn a_guest_reads_its_name_address_key_and_user_data() {
    let store = Store::in_memory().unwrap();
    let backend: Arc<dyn fabric::NetBackend> = Arc::new(fabric::MockBackend::new());
    let client = fabric::Client::new(store.clone(), backend);
    let project = Id::new(kind::PROJECT);
    let net = client
        .create_network(
            Meta::new(kind::NETWORK, "app", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    client
        .create_subnet(
            Meta::new(kind::SUBNET, "app-net", Some(project.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.90.0.0/24".parse().unwrap(),
                gateway: Some("10.90.0.1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let machine = store
        .collection::<Machine>()
        .create(new_machine(
            "box",
            Some(project.clone()),
            MachineSpec {
                size: "s.1x1".into(),
                image: "ubuntu-24.04".into(),
                networks: vec!["app".into()],
                ssh_keys: vec!["ssh-ed25519 AAAAbox".into()],
                user_data: Some("#cloud-config\n".into()),
                ..MachineSpec::default()
            },
        ))
        .unwrap();
    let port = client
        .create_port(
            Meta::new(kind::PORT, "box-eth0", Some(project.clone())),
            PortSpec {
                network: net.meta.id.clone(),
                device: Some(machine.meta.id.clone()),
                fixed_ips: vec!["10.90.0.10".parse().unwrap()],
                ..PortSpec::default()
            },
        )
        .unwrap();
    let peer = IpAddr::V4(Ipv4Addr::new(10, 90, 0, 10));
    let (code, body) = answer(&store, peer, "/latest/meta-data/local-ipv4");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, b"10.90.0.10");
    let (_, host) = answer(&store, peer, "/latest/meta-data/hostname");
    assert_eq!(host, b"box");
    let (_, pubip) = answer(&store, peer, "/latest/meta-data/public-ipv4");
    assert_eq!(pubip, b"10.90.0.10");
    let (_, key) = answer(&store, peer, "/latest/meta-data/public-keys/0/openssh-key");
    assert_eq!(key, b"ssh-ed25519 AAAAbox");
    let (_, data) = answer(&store, peer, "/latest/user-data");
    assert_eq!(data, b"#cloud-config\n");

    let ext = NetworkSpec {
        external: true,
        ..Default::default()
    };
    let fleet = client
        .create_network(
            Meta::new(kind::NETWORK, "fleet", Some(project.clone())),
            ext,
        )
        .unwrap();
    store
        .collection::<FloatingIp>()
        .create(fabric::FloatingIp {
            meta: Meta::new(kind::FLOATING_IP, "box", Some(project)),
            spec: FloatingIpSpec {
                network: fleet.meta.id,
                address: Some("10.66.66.84".parse().unwrap()),
                port: Some(port.meta.id),
            },
            status: Default::default(),
        })
        .unwrap();
    let (_, pubip) = answer(&store, peer, "/latest/meta-data/public-ipv4/");
    assert_eq!(pubip, b"10.66.66.84");
    let (missing, _) = answer(
        &store,
        IpAddr::V4(Ipv4Addr::new(10, 90, 0, 99)),
        "/latest/meta-data/hostname",
    );
    assert_eq!(missing, StatusCode::NOT_FOUND);
}

#[test]
fn a_guest_reads_a_named_secret_and_not_one_it_was_not_given() {
    let store = Store::in_memory().unwrap();
    let project = Id::new(kind::PROJECT);
    let machine = store
        .collection::<Machine>()
        .create(new_machine(
            "box",
            Some(project.clone()),
            MachineSpec {
                size: "s.1x1".into(),
                image: "ubuntu-24.04".into(),
                secrets: vec!["db-password".into()],
                ..MachineSpec::default()
            },
        ))
        .unwrap();
    let peer = IpAddr::V4(Ipv4Addr::new(10, 90, 0, 10));
    store
        .collection::<Port>()
        .create(Port {
            meta: Meta::new(kind::PORT, "box-eth0", Some(project.clone())),
            spec: PortSpec {
                network: Id::new(kind::NETWORK),
                device: Some(machine.meta.id.clone()),
                fixed_ips: vec![peer],
                ..PortSpec::default()
            },
            status: Default::default(),
        })
        .unwrap();
    let keep = keep::Client::open(&store).unwrap();
    keep.create_secret(
        keep::Secret::new(
            "db-password",
            Some(project.clone()),
            keep::SecretSpec::default(),
        ),
        b"s3cr3t",
    )
    .unwrap();
    keep.create_secret(
        keep::Secret::new("other", Some(project.clone()), keep::SecretSpec::default()),
        b"nope",
    )
    .unwrap();
    keep.create_secret(
        keep::Secret::new(
            "db-password",
            Some(Id::new(kind::PROJECT)),
            keep::SecretSpec::default(),
        ),
        b"wrong-project",
    )
    .unwrap();

    let (code, names) = answer(&store, peer, "/latest/meta-data/secrets");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(names, b"db-password");
    let (code, body) = answer(&store, peer, "/latest/meta-data/secrets/db-password");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, b"s3cr3t");
    let (code, body) = answer(&store, peer, "/latest/meta-data/secrets/other");
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert!(body.is_empty());
    let (code, _) = answer(&store, peer, "/latest/meta-data/secrets/missing");
    assert_eq!(code, StatusCode::NOT_FOUND);
}

/// Two projects that picked the same subnet, one guest each at the same
/// address, on two hosts.
struct Twins {
    store: Store,
    red_net: Id,
    blue_net: Id,
    red: Id,
}

fn twins() -> Twins {
    let store = Store::in_memory().unwrap();
    let backend: Arc<dyn fabric::NetBackend> = Arc::new(fabric::MockBackend::new());
    let client = fabric::Client::new(store.clone(), backend);
    let mut made = Vec::new();
    for (name, node) in [("red", "node-a"), ("blue", "node-b")] {
        let project = Id::new(kind::PROJECT);
        let host = store
            .collection::<Host>()
            .create(crate::types::new_host(
                node,
                node,
                Default::default(),
                vec![],
                Default::default(),
            ))
            .unwrap();
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, "app", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        for (sub, cidr, gw) in [
            ("v4", "10.90.0.0/24", "10.90.0.1"),
            ("v6", "fd00:90::/64", "fd00:90::1"),
        ] {
            client
                .create_subnet(
                    Meta::new(kind::SUBNET, sub, Some(project.clone())),
                    SubnetSpec {
                        network: net.meta.id.clone(),
                        cidr: cidr.parse().unwrap(),
                        gateway: Some(gw.parse().unwrap()),
                        allocation_pools: vec![],
                        dns_nameservers: vec![],
                        enable_dhcp: true,
                        host_routes: vec![],
                    },
                )
                .unwrap();
        }
        let mut machine = new_machine(
            name,
            Some(project.clone()),
            MachineSpec {
                size: "s.1x1".into(),
                image: "ubuntu-24.04".into(),
                networks: vec!["app".into()],
                ..MachineSpec::default()
            },
        );
        machine.status.host = Some(host.meta.id.clone());
        let machine = store.collection::<Machine>().create(machine).unwrap();
        client
            .create_port(
                Meta::new(kind::PORT, "eth0", Some(project)),
                PortSpec {
                    network: net.meta.id.clone(),
                    device: Some(machine.meta.id.clone()),
                    fixed_ips: vec!["10.90.0.10".parse().unwrap()],
                    ..PortSpec::default()
                },
            )
            .unwrap();
        made.push((net.meta.id, machine.meta.id.clone()));
    }
    Twins {
        store,
        red_net: made[0].0.clone(),
        blue_net: made[1].0.clone(),
        red: made[0].1.clone(),
    }
}

#[test]
fn a_host_is_told_the_networks_of_the_guests_it_runs() {
    let t = twins();
    let on_a = host_networks(&t.store, "node-a").unwrap();
    assert_eq!(
        on_a,
        vec![MetadataNetwork {
            network: t.red_net.to_string(),
            port: format!("pimeta-{}", t.red_net.uuid),
            mac: fabric::backend::ovn::metadata_port_mac(&t.red_net),
            // The way back to the guests is IPv4 only.
            cidrs: vec!["10.90.0.0/24".into()],
        }]
    );
    let on_b = host_networks(&t.store, "node-b").unwrap();
    assert_eq!(on_b.len(), 1);
    assert_eq!(on_b[0].network, t.blue_net.to_string());
    assert!(host_networks(&t.store, "node-c").unwrap().is_empty());

    // The last guest on the host goes, and the network with it.
    t.store.collection::<Machine>().delete(&t.red).unwrap();
    assert!(host_networks(&t.store, "node-a").unwrap().is_empty());
    assert_eq!(host_networks(&t.store, "node-b").unwrap().len(), 1);
}

#[test]
fn the_network_and_the_host_decide_whose_metadata_is_read() {
    let t = twins();
    let peer: IpAddr = "10.90.0.10".parse().unwrap();
    let name = "/latest/meta-data/hostname";
    let red = t.red_net.to_string();
    let blue = t.blue_net.to_string();

    let (code, body) = answer_on(&t.store, "node-a", &red, peer, name);
    assert_eq!((code, body.as_slice()), (StatusCode::OK, &b"red"[..]));
    let (code, body) = answer_on(&t.store, "node-b", &blue, peer, name);
    assert_eq!((code, body.as_slice()), (StatusCode::OK, &b"blue"[..]));

    // A host cannot read for a guest it does not run, whichever
    // network it names.
    for (node, network) in [("node-a", &blue), ("node-b", &red), ("node-c", &red)] {
        let (code, body) = answer_on(&t.store, node, network, peer, name);
        assert_eq!(code, StatusCode::NOT_FOUND, "{node} {network}");
        assert!(body.is_empty());
    }
    let other: IpAddr = "10.90.0.11".parse().unwrap();
    let (code, _) = answer_on(&t.store, "node-a", &red, other, name);
    assert_eq!(code, StatusCode::NOT_FOUND);
    let (code, _) = answer_on(&t.store, "node-a", "not-a-network", peer, name);
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_agent_asks_over_its_own_channel_and_only_for_itself() {
    use tower::ServiceExt;

    let t = twins();
    let queue = pistack_rpc::CommandQueue::new(Default::default());
    let rpc = pistack_rpc::router_with_auth_and(
        queue,
        t.store.clone(),
        pistack_rpc::RpcAuth::open(),
        agent_routes(t.store.clone()),
    );
    let app = Router::new().nest("/rpc", rpc.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/rpc", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let agent = pistack_rpc::AgentClient::new(base, "node-a").unwrap();
    let peer: IpAddr = "10.90.0.10".parse().unwrap();
    let red = t.red_net.to_string();
    let got = agent
        .metadata(&red, peer, "/latest/meta-data/local-ipv4")
        .await
        .unwrap();
    assert_eq!(got, (200, b"10.90.0.10".to_vec()));
    let got = agent
        .metadata(&t.blue_net.to_string(), peer, "/latest/meta-data/hostname")
        .await
        .unwrap();
    assert_eq!(got, (404, Vec::new()));
    // This process is not on OVN, so no host is asked to build anything.
    assert!(agent.metadata_networks().await.unwrap().is_empty());

    // A certificate for one node does not speak for another.
    let as_b = rpc.layer(axum::Extension(keep::PeerIdentity {
        addr: None,
        common_name: Some("node-b".into()),
        tls: true,
        cert_serial: None,
    }));
    let query = MetadataQuery {
        node_name: "node-a".into(),
        network: red,
        address: peer,
        path: "/latest/meta-data/hostname".into(),
    };
    let post = axum::http::Request::post("/metadata/guest")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&query).unwrap()))
        .unwrap();
    let refused = as_b.clone().oneshot(post).await.unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let list = axum::http::Request::get("/metadata/networks/node-a")
        .body(axum::body::Body::empty())
        .unwrap();
    let refused = as_b.oneshot(list).await.unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
}
