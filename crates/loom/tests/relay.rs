// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Service and ingress listeners programmed through the relay realizer.

use loom::controllers::ingress::IngressController;
use loom::controllers::Controller;
use loom::{
    Endpoint, Ingress, IngressBackend, IngressPath, IngressRule, IngressSpec, IngressTls,
    IntOrName, Protocol, RelayRealizer, Service, ServiceKind, ServicePort, ServiceRealizer,
    ServiceSpec, DELETE_ANNOTATION,
};
use pistack_store::Store;
use pistack_types::{kind, Id, Meta};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn endpoint(addr: SocketAddr) -> Endpoint {
    Endpoint {
        ip: addr.ip().to_string(),
        pod: Id::new(loom::kinds::POD),
        host: None,
    }
}

fn service(
    name: &str,
    kind: ServiceKind,
    port: u16,
    target: u16,
    protocol: Protocol,
    endpoints: Vec<Endpoint>,
) -> Service {
    Service {
        meta: Meta::new(loom::kinds::SERVICE, name, None),
        spec: ServiceSpec {
            ports: vec![ServicePort {
                name: "http".into(),
                protocol,
                port,
                target_port: Some(target),
                node_port: None,
            }],
            kind,
            ..ServiceSpec::default()
        },
        status: loom::ServiceStatus {
            endpoints,
            ..loom::ServiceStatus::default()
        },
    }
}

fn realizer() -> (Store, std::sync::Arc<RelayRealizer>) {
    let store = Store::in_memory().unwrap();
    let relay = std::sync::Arc::new(RelayRealizer::new());
    relay.bind_store(store.clone());
    (store, relay)
}

async fn listen() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").await.unwrap()
}

fn spawn_tcp(listener: TcpListener, body: &'static [u8]) {
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let _ = sock.set_nodelay(true);
                let mut buf = [0u8; 4];
                if sock.read_exact(&mut buf).await.is_err() {
                    return;
                }
                let _ = sock.write_all(body).await;
            });
        }
    });
}

fn spawn_http(listener: TcpListener, body: &'static [u8]) {
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let _ = sock.set_nodelay(true);
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
            });
        }
    });
}

async fn ping(addr: &str) -> Vec<u8> {
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.set_nodelay(true).unwrap();
    sock.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    sock.read_exact(&mut buf).await.unwrap();
    buf.to_vec()
}

async fn http_get(addr: &str) -> Vec<u8> {
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.set_nodelay(true).unwrap();
    sock.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    buf
}

async fn closed(addr: &str) {
    let mut gone = false;
    for _ in 0..25 {
        if tokio::net::TcpStream::connect(addr).await.is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone, "{addr} still accepts");
}

#[tokio::test]
async fn unbound_realizer_is_unavailable() {
    let relay = RelayRealizer::new();
    let svc = service(
        "web",
        ServiceKind::ClusterIp,
        80,
        8080,
        Protocol::Tcp,
        vec![],
    );
    let err = relay.realize(&svc, &[]).await.unwrap_err();
    assert!(
        err.message.contains("relay realizer is not attached"),
        "{err}"
    );
}

#[tokio::test]
async fn cluster_ip_and_load_balancer_forward_and_remove_is_idempotent() {
    let (store, relay) = realizer();
    let one = listen().await;
    let one_addr = one.local_addr().unwrap();
    spawn_tcp(one, b"one!");
    let two = listen().await;
    let two_addr = two.local_addr().unwrap();
    spawn_tcp(two, b"two!");

    let mut svc = service(
        "web",
        ServiceKind::LoadBalancer,
        80,
        one_addr.port(),
        Protocol::Tcp,
        vec![endpoint(one_addr)],
    );
    let addrs = relay.realize(&svc, &[endpoint(one_addr)]).await.unwrap();
    assert_eq!(addrs.len(), 1);
    assert!(addrs[0].starts_with("127.0.0.1:"), "{}", addrs[0]);
    assert_eq!(ping(&addrs[0]).await, b"one!");

    let again = relay.realize(&svc, &[endpoint(one_addr)]).await.unwrap();
    assert_eq!(again, addrs);
    assert_eq!(ping(&again[0]).await, b"one!");

    svc.spec.ports[0].target_port = Some(two_addr.port());
    let moved = relay.realize(&svc, &[endpoint(two_addr)]).await.unwrap();
    assert_eq!(moved, addrs);
    assert_eq!(ping(&moved[0]).await, b"two!");

    relay.remove(&svc).await.unwrap();
    relay.remove(&svc).await.unwrap();
    closed(&addrs[0]).await;

    let mut cluster = service(
        "cluster",
        ServiceKind::ClusterIp,
        80,
        one_addr.port(),
        Protocol::Tcp,
        vec![endpoint(one_addr)],
    );
    let published = relay
        .realize(&cluster, &[endpoint(one_addr)])
        .await
        .unwrap();
    assert!(published.is_empty());
    let lbs = store.collection::<relay::Balancer>().list(None).unwrap();
    let lb = lbs
        .iter()
        .find(|lb| lb.meta.name == format!("svc-{}-80", cluster.meta.id))
        .unwrap();
    let bound = lb.status.provider_ref.clone().unwrap();
    assert!(bound.starts_with("127.0.0.1:"), "{bound}");
    assert_eq!(ping(&bound).await, b"one!");
    cluster.spec.ports[0].protocol = Protocol::Sctp;
    let err = relay
        .realize(&cluster, &[endpoint(one_addr)])
        .await
        .unwrap_err();
    assert!(
        err.message.contains("service forward is tcp and udp only"),
        "{err}"
    );
    closed(&bound).await;
    assert!(store
        .collection::<relay::Balancer>()
        .list(None)
        .unwrap()
        .iter()
        .all(|lb| lb.meta.labels.get("loom.pistack.io/service")
            != Some(&cluster.meta.id.to_string())));
}

#[tokio::test]
async fn ingress_programs_a_listener_and_delete_removes_it() {
    let (store, relay) = realizer();
    let missing = Ingress {
        meta: Meta::new(loom::kinds::INGRESS, "missing", None),
        spec: IngressSpec {
            default_backend: Some(IngressBackend {
                service: "no-such-service".into(),
                port: IntOrName::Int(80),
            }),
            ..IngressSpec::default()
        },
        status: loom::IngressStatus::default(),
    };
    store
        .collection::<Ingress>()
        .create(missing.clone())
        .unwrap();
    let err = relay.ensure_ingress(&missing).await.unwrap_err();
    assert!(
        err.message.contains("ingress backend is not available"),
        "{err}"
    );
    assert!(!err.message.contains("Ready"));
    let ctl = IngressController::new(store.clone(), Some(relay.clone()));
    ctl.reconcile_once().await.unwrap();
    let stored = store.collection::<Ingress>().get(&missing.meta.id).unwrap();
    let ready = stored
        .status
        .conditions
        .iter()
        .find(|c| c.kind == "Ready")
        .unwrap();
    assert!(!ready.status);
    assert_eq!(ready.reason, "Unavailable");
    assert!(stored.status.load_balancer.is_empty());
    assert!(store
        .collection::<relay::Balancer>()
        .list(None)
        .unwrap()
        .iter()
        .all(|lb| lb.status.phase != pistack_types::Phase::Ready
            || !lb.meta.labels.contains_key("loom.pistack.io/ingress")));

    let backend = listen().await;
    let backend_addr = backend.local_addr().unwrap();
    spawn_http(backend, b"hello-edge");
    let mut web = service(
        "web",
        ServiceKind::ClusterIp,
        80,
        backend_addr.port(),
        Protocol::Tcp,
        vec![endpoint(backend_addr)],
    );
    web.status.endpoints = vec![endpoint(backend_addr)];
    store.collection::<Service>().create(web.clone()).unwrap();
    let edge = Ingress {
        meta: Meta::new(loom::kinds::INGRESS, "edge", None),
        spec: IngressSpec {
            rules: vec![IngressRule {
                host: None,
                paths: vec![IngressPath {
                    path: "/".into(),
                    path_type: loom::PathType::Prefix,
                    backend: IngressBackend {
                        service: "web".into(),
                        port: IntOrName::Name("http".into()),
                    },
                }],
            }],
            ..IngressSpec::default()
        },
        status: loom::IngressStatus::default(),
    };
    store.collection::<Ingress>().create(edge.clone()).unwrap();
    ctl.reconcile_once().await.unwrap();
    let stored = store.collection::<Ingress>().get(&edge.meta.id).unwrap();
    assert_eq!(stored.status.load_balancer.len(), 1, "{stored:?}");
    let addr = stored.status.load_balancer[0].clone();
    assert!(addr.starts_with("127.0.0.1:"), "{addr}");
    let ready = stored
        .status
        .conditions
        .iter()
        .find(|c| c.kind == "Ready")
        .unwrap();
    assert!(ready.status);
    assert_eq!(ready.reason, "Programmed");
    let body = http_get(&addr).await;
    assert!(
        body.windows(b"hello-edge".len())
            .any(|w| w == b"hello-edge"),
        "{body:?}"
    );

    let mut orphan_meta = Meta::new(kind::BALANCER, "orphan-edge", None);
    orphan_meta.labels.insert(
        "loom.pistack.io/ingress".into(),
        Id::new(loom::kinds::INGRESS).to_string(),
    );
    let orphan = relay::Balancer {
        meta: orphan_meta,
        spec: relay::BalancerSpec {
            vip_network: "cluster".into(),
            provider: relay::Provider::Forward,
            admin_up: true,
            ..relay::BalancerSpec::default()
        },
        status: relay::BalancerStatus::default(),
    };
    store
        .collection::<relay::Balancer>()
        .create(orphan.clone())
        .unwrap();
    relay.gc_ingresses().await.unwrap();
    assert!(store
        .collection::<relay::Balancer>()
        .try_get(&orphan.meta.id)
        .unwrap()
        .is_none());

    store
        .collection::<Ingress>()
        .modify(&edge.meta.id, |row| {
            row.meta
                .annotations
                .insert(DELETE_ANNOTATION.into(), "1".into());
            Ok(())
        })
        .unwrap();
    ctl.reconcile_once().await.unwrap();
    assert!(store
        .collection::<Ingress>()
        .try_get(&edge.meta.id)
        .unwrap()
        .is_none());
    closed(&addr).await;

    let bare = Store::in_memory().unwrap();
    let quiet = Ingress {
        meta: Meta::new(loom::kinds::INGRESS, "quiet", None),
        spec: IngressSpec::default(),
        status: loom::IngressStatus::default(),
    };
    bare.collection::<Ingress>().create(quiet.clone()).unwrap();
    let ctl = IngressController::new(bare.clone(), None);
    ctl.reconcile_once().await.unwrap();
    let stored = bare.collection::<Ingress>().get(&quiet.meta.id).unwrap();
    let ready = stored
        .status
        .conditions
        .iter()
        .find(|c| c.kind == "Ready")
        .unwrap();
    assert!(!ready.status);
    assert!(ready.message.contains("ingress relay is not configured"));
    assert!(stored.status.load_balancer.is_empty());
}

async fn http_req(addr: &str, host: &str, path: &str) -> String {
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.set_nodelay(true).unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    sock.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), sock.read_to_end(&mut buf))
        .await
        .expect("timed out")
        .unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

fn rule_path(path: &str, path_type: loom::PathType, svc: &str) -> IngressPath {
    IngressPath {
        path: path.into(),
        path_type,
        backend: IngressBackend {
            service: svc.into(),
            port: IntOrName::Int(80),
        },
    }
}

#[tokio::test]
async fn ingress_routes_by_host_and_path() {
    let (store, relay) = realizer();
    let mut backends = Vec::new();
    for (name, body) in [
        ("web", &b"from-web"[..]),
        ("api", &b"from-api"[..]),
        ("docs", &b"from-docs"[..]),
    ] {
        let backend = listen().await;
        let addr = backend.local_addr().unwrap();
        spawn_http(backend, body);
        let svc = service(
            name,
            ServiceKind::ClusterIp,
            80,
            addr.port(),
            Protocol::Tcp,
            vec![endpoint(addr)],
        );
        store.collection::<Service>().create(svc).unwrap();
        backends.push(addr);
    }
    let mut edge = Ingress {
        meta: Meta::new(loom::kinds::INGRESS, "routed", None),
        spec: IngressSpec {
            rules: vec![
                IngressRule {
                    host: Some("App.Example.com".into()),
                    paths: vec![
                        rule_path("/", loom::PathType::Prefix, "web"),
                        rule_path("/api", loom::PathType::Prefix, "api"),
                    ],
                },
                IngressRule {
                    host: Some("*.docs.example.com".into()),
                    paths: vec![rule_path("/guide", loom::PathType::Exact, "docs")],
                },
            ],
            ..IngressSpec::default()
        },
        status: loom::IngressStatus::default(),
    };
    store.collection::<Ingress>().create(edge.clone()).unwrap();
    let addr = relay.ensure_ingress(&edge).await.unwrap().remove(0);

    let cases = [
        ("app.example.com", "/", "from-web"),
        ("app.example.com:80", "/apis", "from-web"),
        ("app.example.com", "/api", "from-api"),
        ("app.example.com", "/api/v1/things?x=1", "from-api"),
        ("eu.docs.example.com", "/guide", "from-docs"),
        ("eu.docs.example.com", "/guide/more", "404 Not Found"),
        ("a.b.docs.example.com", "/guide", "404 Not Found"),
        ("other.example.com", "/api", "404 Not Found"),
    ];
    for (host, path, want) in cases {
        let got = http_req(&addr, host, path).await;
        assert!(got.contains(want), "{host}{path}: {got}");
    }

    // A default backend catches what no rule matches.
    edge.spec.default_backend = Some(IngressBackend {
        service: "docs".into(),
        port: IntOrName::Int(80),
    });
    edge.spec.rules.truncate(1);
    store
        .collection::<Ingress>()
        .update(edge.clone(), None)
        .unwrap();
    let again = relay.ensure_ingress(&edge).await.unwrap().remove(0);
    assert_eq!(again, addr);
    let got = http_req(&addr, "other.example.com", "/api").await;
    assert!(got.contains("from-docs"), "{got}");
    let got = http_req(&addr, "app.example.com", "/api/x").await;
    assert!(got.contains("from-api"), "{got}");

    // Dropping a rule removes its pool.
    let pools = store.collection::<relay::Pool>().list(None).unwrap();
    let names: Vec<_> = pools.iter().map(|p| p.meta.name.clone()).collect();
    assert_eq!(pools.len(), 3, "{names:?}");
    assert!(!names.iter().any(|n| n.ends_with("-route-2")), "{names:?}");
}

/// A free port in the node-port range on loopback.
async fn https_req(keep: &keep::Client, addr: &str, host: &str, path: &str) -> String {
    let bound: SocketAddr = addr.parse().unwrap();
    let cfg = keep::tls::client_config(&keep.ca().bundle_pem(), None).unwrap();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(cfg));
    let tcp = tokio::net::TcpStream::connect(bound).await.unwrap();
    let name = rustls::pki_types::ServerName::IpAddress(bound.ip().into());
    let mut tls = connector.connect(name, tcp).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    tls.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tls.read_to_end(&mut buf))
        .await
        .expect("timed out")
        .unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn ingress_tls_serves_https_with_the_named_certificate() {
    let (store, relay) = realizer();
    for (name, body) in [("web", &b"from-web"[..]), ("api", &b"from-api"[..])] {
        let backend = listen().await;
        let addr = backend.local_addr().unwrap();
        spawn_http(backend, body);
        let svc = service(
            name,
            ServiceKind::ClusterIp,
            80,
            addr.port(),
            Protocol::Tcp,
            vec![endpoint(addr)],
        );
        store.collection::<Service>().create(svc).unwrap();
    }
    let keep = keep::Client::open(&store).unwrap();
    let spec = keep::CertificateSpec {
        common_name: "app.example.com".into(),
        sans: vec!["app.example.com".into(), "127.0.0.1".into()],
        ..keep::CertificateSpec::default()
    };
    keep.issue_certificate(None, "app-cert", spec.clone())
        .unwrap();
    keep.issue_certificate(Some(Id::new(kind::PROJECT)), "elsewhere", spec)
        .unwrap();

    let tls_for = |name: &str| IngressTls {
        hosts: vec!["app.example.com".into()],
        secret_name: name.into(),
    };
    let mut edge = Ingress {
        meta: Meta::new(loom::kinds::INGRESS, "secure", None),
        spec: IngressSpec {
            rules: vec![IngressRule {
                host: Some("app.example.com".into()),
                paths: vec![
                    rule_path("/", loom::PathType::Prefix, "web"),
                    rule_path("/api", loom::PathType::Prefix, "api"),
                ],
            }],
            tls: vec![tls_for("app-cert")],
            ..IngressSpec::default()
        },
        status: loom::IngressStatus::default(),
    };
    store.collection::<Ingress>().create(edge.clone()).unwrap();
    let addrs = relay.ensure_ingress(&edge).await.unwrap();
    assert_eq!(addrs.len(), 2, "{addrs:?}");
    let (http, https) = (addrs[0].clone(), addrs[1].clone());

    // Plain HTTP keeps working; HTTPS routes the same way with the cert.
    let got = http_req(&http, "app.example.com", "/").await;
    assert!(got.contains("from-web"), "{got}");
    let got = https_req(&keep, &https, "app.example.com", "/").await;
    assert!(got.contains("from-web"), "{got}");
    let got = https_req(&keep, &https, "app.example.com", "/api/v1").await;
    assert!(got.contains("from-api"), "{got}");

    // Reconciling again keeps the same sockets.
    let again = relay.ensure_ingress(&edge).await.unwrap();
    assert_eq!(again, addrs);

    // Two different certificates on one ingress are refused, HTTPS goes away.
    edge.spec.tls = vec![tls_for("app-cert"), tls_for("other-cert")];
    let err = relay.ensure_ingress(&edge).await.unwrap_err();
    assert!(err.message.contains("one certificate per ingress"), "{err}");
    closed(&https).await;
    let got = http_req(&http, "app.example.com", "/").await;
    assert!(got.contains("from-web"), "{got}");

    // A missing name, or one from another project, is a clear error.
    for name in ["no-such-cert", "elsewhere"] {
        edge.spec.tls = vec![tls_for(name)];
        let err = relay.ensure_ingress(&edge).await.unwrap_err();
        assert!(
            err.message
                .contains(&format!("ingress certificate '{name}' not found")),
            "{err}"
        );
    }

    // Dropping tls leaves one HTTP listener.
    edge.spec.tls = vec![tls_for("app-cert")];
    let https = relay.ensure_ingress(&edge).await.unwrap().remove(1);
    edge.spec.tls.clear();
    let addrs = relay.ensure_ingress(&edge).await.unwrap();
    assert_eq!(addrs, vec![http.clone()]);
    closed(&https).await;
    let lbs = store.collection::<relay::Balancer>().list(None).unwrap();
    assert_eq!(
        lbs.len(),
        1,
        "{:?}",
        lbs.iter().map(|l| &l.meta.name).collect::<Vec<_>>()
    );

    relay.remove_ingress(&edge).await.unwrap();
    closed(&http).await;
}

fn free_node_port() -> u16 {
    (30000..=32767)
        .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
        .expect("a free node port")
}

#[tokio::test]
async fn node_port_listens_on_the_node_address_and_moves() {
    let store = Store::in_memory().unwrap();
    let relay =
        std::sync::Arc::new(RelayRealizer::new().with_node_address("127.0.0.1".parse().unwrap()));
    relay.bind_store(store.clone());
    let backend = listen().await;
    let backend_addr = backend.local_addr().unwrap();
    spawn_tcp(backend, b"node");

    let mut svc = service(
        "np",
        ServiceKind::NodePort,
        80,
        backend_addr.port(),
        Protocol::Tcp,
        vec![endpoint(backend_addr)],
    );
    let first = free_node_port();
    svc.status.node_ports.insert(80, first);
    let published = relay
        .realize(&svc, &[endpoint(backend_addr)])
        .await
        .unwrap();
    assert!(published.is_empty());
    let at = format!("127.0.0.1:{first}");
    assert_eq!(ping(&at).await, b"node");

    // Same pass again keeps the same listener.
    relay
        .realize(&svc, &[endpoint(backend_addr)])
        .await
        .unwrap();
    assert_eq!(ping(&at).await, b"node");

    // A new node port replaces the old listener.
    let second = (first + 1..=32767)
        .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
        .unwrap();
    svc.status.node_ports.insert(80, second);
    relay
        .realize(&svc, &[endpoint(backend_addr)])
        .await
        .unwrap();
    let moved = format!("127.0.0.1:{second}");
    assert_eq!(ping(&moved).await, b"node");
    closed(&at).await;

    // Back to ClusterIp drops the node listener.
    svc.spec.kind = ServiceKind::ClusterIp;
    svc.status.node_ports.clear();
    relay
        .realize(&svc, &[endpoint(backend_addr)])
        .await
        .unwrap();
    closed(&moved).await;

    relay.remove(&svc).await.unwrap();
    assert!(store
        .collection::<relay::Balancer>()
        .list(None)
        .unwrap()
        .iter()
        .all(|lb| lb.meta.labels.get("loom.pistack.io/service") != Some(&svc.meta.id.to_string())));
}

fn spawn_udp(socket: tokio::net::UdpSocket, body: &'static [u8]) {
    tokio::spawn(async move {
        let mut buf = [0u8; 64];
        while let Ok((_, peer)) = socket.recv_from(&mut buf).await {
            let _ = socket.send_to(body, peer).await;
        }
    });
}

async fn udp_ping(addr: &str) -> Vec<u8> {
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut buf = [0u8; 64];
    for _ in 0..10 {
        sock.send_to(b"ping", addr).await.unwrap();
        if let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(300), sock.recv(&mut buf)).await
        {
            return buf[..n].to_vec();
        }
    }
    panic!("{addr} gave no udp answer");
}

/// A port in the node-port range free for both tcp and udp on loopback.
fn free_node_port_both(after: u16) -> u16 {
    (after..=32767)
        .find(|p| {
            std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok()
                && std::net::UdpSocket::bind(("127.0.0.1", *p)).is_ok()
        })
        .expect("a free node port")
}

#[tokio::test]
async fn udp_and_tcp_on_one_port_share_a_node_port() {
    let store = Store::in_memory().unwrap();
    let relay =
        std::sync::Arc::new(RelayRealizer::new().with_node_address("127.0.0.1".parse().unwrap()));
    relay.bind_store(store.clone());

    // One backend port number answering on both tcp and udp.
    let (tcp, udp) = loop {
        let tcp = listen().await;
        let port = tcp.local_addr().unwrap().port();
        if let Ok(udp) = tokio::net::UdpSocket::bind(("127.0.0.1", port)).await {
            break (tcp, udp);
        }
    };
    let backend = tcp.local_addr().unwrap();
    spawn_tcp(tcp, b"tcp!");
    spawn_udp(udp, b"udp!");

    let mut svc = service(
        "dns",
        ServiceKind::NodePort,
        53,
        backend.port(),
        Protocol::Tcp,
        vec![endpoint(backend)],
    );
    let mut udp_port = svc.spec.ports[0].clone();
    udp_port.name = "dns-udp".into();
    udp_port.protocol = Protocol::Udp;
    svc.spec.ports.push(udp_port);
    let node_port = free_node_port_both(31500);
    svc.status.node_ports.insert(53, node_port);

    relay.realize(&svc, &[endpoint(backend)]).await.unwrap();
    let at = format!("127.0.0.1:{node_port}");
    assert_eq!(ping(&at).await, b"tcp!");
    assert_eq!(udp_ping(&at).await, b"udp!");

    // The cluster-side udp listener answers too.
    let lbs = store.collection::<relay::Balancer>().list(None).unwrap();
    let cluster_udp = lbs
        .iter()
        .find(|lb| lb.meta.name == format!("svc-{}-53-udp", svc.meta.id))
        .unwrap();
    let bound = cluster_udp.status.provider_ref.clone().unwrap();
    assert_eq!(udp_ping(&bound).await, b"udp!");

    // Again is a no-op.
    relay.realize(&svc, &[endpoint(backend)]).await.unwrap();
    assert_eq!(udp_ping(&at).await, b"udp!");

    relay.remove(&svc).await.unwrap();
    closed(&at).await;
    assert!(store
        .collection::<relay::Balancer>()
        .list(None)
        .unwrap()
        .iter()
        .all(|lb| lb.meta.labels.get("loom.pistack.io/service") != Some(&svc.meta.id.to_string())));
}
