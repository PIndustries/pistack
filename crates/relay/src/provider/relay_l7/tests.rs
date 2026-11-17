// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Proxy tests: the bind rules, the route table, the algorithms seen
//! through HTTP, and the request the member receives.

use super::http::route_key;
use super::provider::RelayL7Provider;
use super::table::{Choice, RouteTarget, Table};

use crate::picker::{Backend, Picker};
use crate::provider::{LbProvider, ListenerView};
use crate::types::*;
use pistack_types::error::ErrorCode;
use pistack_types::{kind, Id, Meta};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn balancer(vip: Option<IpAddr>) -> Balancer {
    Balancer {
        meta: Meta::new(kind::BALANCER, "lb", None),
        spec: BalancerSpec {
            vip_network: "net".into(),
            vip_address: vip,
            vip_port: None,
            floating_ip: None,
            provider: Provider::Relay,
            admin_up: true,
        },
        status: BalancerStatus::default(),
    }
}

fn view(lb: &Balancer, protocol: ListenerProtocol, members: Vec<SocketAddr>) -> ListenerView {
    let weights = vec![1u16; members.len()];
    algorithm_view(lb, protocol, members, weights, Algorithm::default())
}

/// A view whose pool carries `algorithm`, one weight per member.
fn algorithm_view(
    lb: &Balancer,
    protocol: ListenerProtocol,
    members: Vec<SocketAddr>,
    weights: Vec<u16>,
    algorithm: Algorithm,
) -> ListenerView {
    let pool_meta = Meta::new(kinds::POOL, "pool", None);
    let member_rows = members
        .into_iter()
        .zip(weights)
        .map(|(addr, weight)| Member {
            meta: Meta::new(kinds::MEMBER, "m", None),
            spec: MemberSpec {
                pool: pool_meta.id.clone(),
                address: addr.ip(),
                port: addr.port(),
                weight,
                backup: false,
                admin_up: true,
            },
            status: MemberStatus::default(),
        })
        .collect();
    let pool = Pool {
        meta: pool_meta,
        spec: PoolSpec {
            balancer: lb.meta.id.clone(),
            protocol: PoolProtocol::Http,
            algorithm,
            session_persistence: None,
            health_monitor: None,
            admin_up: true,
        },
        status: PoolStatus::default(),
    };
    ListenerView {
        balancer: lb.clone(),
        listener: Listener {
            meta: Meta::new(kinds::LISTENER, "ln", None),
            spec: ListenerSpec {
                balancer: lb.meta.id.clone(),
                protocol,
                port: 80,
                default_pool: Some(pool.meta.id.clone()),
                connection_limit: 0,
                admin_up: true,
                door: None,
                routes: Vec::new(),
            },
            status: ListenerStatus::default(),
        },
        pool: Some((pool, member_rows)),
        tls: None,
        routes: Vec::new(),
    }
}

#[test]
fn serves_http_not_l4() {
    let p = RelayL7Provider::new();
    assert_eq!(p.name(), "http");
    assert!(p.serves(Provider::Relay));
    assert!(!p.serves(Provider::Forward));
}

#[tokio::test]
async fn refuses_public_udp_and_tls() {
    let p = RelayL7Provider::new();
    let err = p
        .ensure_balancer(&balancer(Some(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)))))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    let lb = balancer(None);
    p.ensure_balancer(&lb).await.unwrap();
    let udp = p
        .ensure_listener(&view(&lb, ListenerProtocol::Udp, vec![]))
        .await
        .unwrap_err();
    assert_eq!(udp.code, ErrorCode::Unavailable);
    assert!(udp.message.contains("no up member"));
    for ip in [
        IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
        IpAddr::V4(Ipv4Addr::BROADCAST),
    ] {
        let err = p.ensure_balancer(&balancer(Some(ip))).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest, "{ip}");
    }
    let tls = p
        .ensure_listener(&view(
            &lb,
            ListenerProtocol::Terminated {
                cert: Id::new(kind::CERT),
            },
            vec![],
        ))
        .await
        .unwrap_err();
    assert_eq!(tls.code, ErrorCode::Unavailable);
    assert!(tls.message.contains("no certificate"));
    p.delete_balancer(&lb.meta.id).await.unwrap();
}

#[tokio::test]
async fn proxies_get_and_delete_refuses_new_connects() {
    let member = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let member_addr = member.local_addr().unwrap();
    let member_task = tokio::spawn(async move {
        let (mut sock, _) = member.accept().await.unwrap();
        let mut buf = vec![0u8; 2048];
        let n = sock.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).into_owned();
        assert!(req.starts_with("GET / HTTP/1.1"), "{req}");
        assert!(req.contains("X-Forwarded-For:"), "{req}");
        let body = b"pong";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
        sock.write_all(body).await.unwrap();
    });

    let p = RelayL7Provider::new();
    let lb = balancer(None);
    let realized = p.ensure_balancer(&lb).await.unwrap();
    p.ensure_listener(&view(&lb, ListenerProtocol::Http, vec![member_addr]))
        .await
        .unwrap();

    let mut client = TcpStream::connect(&realized.provider_ref).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: app\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.read_to_end(&mut buf),
    )
    .await
    .expect("timed out")
    .unwrap();
    assert!(buf.windows(4).any(|w| w == b"pong"), "{buf:?}");
    member_task.await.unwrap();

    p.delete_listener(&lb.meta.id, &Id::new(kinds::LISTENER))
        .await
        .unwrap();
    let err = TcpStream::connect(&realized.provider_ref)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
}

fn target(host: Option<&str>, path: &str, exact: bool, port: u16) -> RouteTarget {
    RouteTarget {
        host: host.map(str::to_owned),
        path: path.into(),
        exact,
        members: vec![Backend::new(SocketAddr::from(([127, 0, 0, 1], port)))],
    }
}

fn port_of(choice: Choice) -> u16 {
    match choice {
        Choice::Member(addr) => addr.port(),
        other => panic!("{other:?}"),
    }
}

/// No connections counted: nothing is open, so the picker's own
/// algorithm decides between the members a route holds.
fn no_conns() -> parking_lot::Mutex<HashMap<SocketAddr, u64>> {
    parking_lot::Mutex::new(HashMap::new())
}

#[test]
fn table_prefers_specific_host_then_longest_path() {
    let picker = Picker::new(Algorithm::RoundRobin);
    let conns = no_conns();
    let table = Table {
        default: Vec::new(),
        routes: vec![
            target(None, "/", false, 1),
            target(Some("*.example.com"), "/", false, 2),
            target(Some("app.example.com"), "/", false, 3),
            target(Some("app.example.com"), "/api/", false, 4),
            target(Some("app.example.com"), "/api", true, 5),
        ],
    };
    let at =
        |host: Option<&str>, path: &str| port_of(table.choose(host, path, None, &picker, &conns));
    assert_eq!(at(Some("app.example.com"), "/"), 3);
    assert_eq!(at(Some("app.example.com"), "/api/v1"), 4);
    assert_eq!(at(Some("app.example.com"), "/api"), 5);
    assert_eq!(at(Some("app.example.com"), "/apix"), 3);
    assert_eq!(at(Some("www.example.com"), "/api"), 2);
    assert_eq!(at(Some("a.b.example.com"), "/"), 1);
    assert_eq!(at(None, "/"), 1);
}

#[test]
fn table_falls_back_then_404s() {
    let picker = Picker::new(Algorithm::RoundRobin);
    let conns = no_conns();
    let mut table = Table {
        default: Vec::new(),
        routes: vec![target(Some("app"), "/x", false, 1)],
    };
    assert_eq!(
        table.choose(Some("app"), "/y", None, &picker, &conns),
        Choice::NotFound
    );
    table.default = vec![Backend::new(SocketAddr::from(([127, 0, 0, 1], 9)))];
    assert_eq!(
        port_of(table.choose(Some("app"), "/y", None, &picker, &conns)),
        9
    );
    table.routes[0].members.clear();
    assert_eq!(
        table.choose(Some("app"), "/x/1", None, &picker, &conns),
        Choice::Unavailable
    );
    let empty = Table::default();
    assert_eq!(
        empty.choose(None, "/", None, &picker, &conns),
        Choice::Unavailable
    );
}

#[test]
fn route_key_reads_host_and_path() {
    let key = |raw: &str| route_key(raw.as_bytes());
    assert_eq!(
        key("GET /a/b?q=1 HTTP/1.1\r\nHost: App.Example.com:8080\r\n\r\n"),
        (Some("app.example.com".into()), "/a/b".into())
    );
    assert_eq!(
        key("GET http://Edge.Local./x#f HTTP/1.1\r\nHost: other\r\n\r\n"),
        (Some("edge.local".into()), "/x".into())
    );
    assert_eq!(
        key("GET ?q HTTP/1.1\r\nhost: [::1]:80\r\n\r\n"),
        (Some("::1".into()), "/".into())
    );
    assert_eq!(key("GET / HTTP/1.0\r\n\r\n"), (None, "/".into()));
}

/// A member answering a request with its own name, so a response says
/// who served it.
async fn speaker(body: &'static [u8]) -> SocketAddr {
    let member = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = member.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = member.accept().await.unwrap();
            let mut buf = vec![0u8; 2048];
            if sock.read(&mut buf).await.unwrap_or(0) == 0 {
                continue;
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if sock.write_all(resp.as_bytes()).await.is_err() {
                continue;
            }
            let _ = sock.write_all(body).await;
        }
    });
    addr
}

/// One request through the proxy, answered with the body the member
/// returned.
/// One request through the proxy: the status line and, when the member
/// answers in time, its body.
async fn ask(vip: &str, path: &str) -> (Vec<u8>, Vec<u8>) {
    let mut client = TcpStream::connect(vip).await.unwrap();
    client
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: app\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = Vec::new();
    let answered = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.read_to_end(&mut buf),
    )
    .await
    .is_ok();
    let head = match buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(at) => buf[..at].to_vec(),
        None => buf.clone(),
    };
    assert!(
        head.starts_with(b"HTTP/1.1"),
        "{}",
        String::from_utf8_lossy(&buf)
    );
    let body = match buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(at) if answered => buf[at + 4..].to_vec(),
        _ => Vec::new(),
    };
    (head, body)
}

/// One request answered with the member's body.
async fn served(vip: &str, path: &str) -> Vec<u8> {
    let (head, body) = ask(vip, path).await;
    assert!(
        head.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    body
}

async fn ask_n(vip: &str, n: usize) -> Vec<Vec<u8>> {
    let mut seen = Vec::new();
    for _ in 0..n {
        seen.push(served(vip, "/").await);
    }
    seen
}

fn tally(bodies: &[Vec<u8>]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for body in bodies {
        let name = String::from_utf8_lossy(body).to_string();
        match out.iter_mut().find(|(k, _)| *k == name) {
            Some((_, n)) => *n += 1,
            None => out.push((name, 1)),
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn round_robin_honours_weight_over_http() {
    let addrs = vec![speaker(b"one").await, speaker(b"two").await];
    let p = RelayL7Provider::new();
    let lb = balancer(None);
    let realized = p.ensure_balancer(&lb).await.unwrap();
    let view = algorithm_view(
        &lb,
        ListenerProtocol::Http,
        addrs,
        vec![3, 1],
        Algorithm::RoundRobin,
    );
    p.ensure_listener(&view).await.unwrap();
    assert_eq!(
        tally(&ask_n(&realized.provider_ref, 8).await),
        vec![("one".into(), 6), ("two".into(), 2)],
        "weight 3 against weight 1 takes three in four"
    );
    p.delete_balancer(&lb.meta.id).await.unwrap();
}

#[tokio::test]
async fn least_conn_picks_the_least_loaded_member_over_http() {
    // One member is kept busy so the other has the fewest open
    // connections, which is what the algorithm is asked to find. Its
    // answer is slow, so the count is still held when the next request
    // arrives.
    let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let busy_addr = busy.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = busy.accept().await.unwrap();
            let mut buf = vec![0u8; 2048];
            if sock.read(&mut buf).await.unwrap_or(0) == 0 {
                continue;
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let _ = sock.write_all(b"never").await;
        }
    });
    let idle = speaker(b"idle").await;

    let p = RelayL7Provider::new();
    let lb = balancer(None);
    let realized = p.ensure_balancer(&lb).await.unwrap();
    let view = algorithm_view(
        &lb,
        ListenerProtocol::Http,
        vec![busy_addr, idle],
        vec![1, 1],
        Algorithm::LeastConn,
    );
    p.ensure_listener(&view).await.unwrap();

    let mut hold = TcpStream::connect(&realized.provider_ref).await.unwrap();
    hold.write_all(b"GET / HTTP/1.1\r\nHost: app\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(
        served(&realized.provider_ref, "/").await,
        b"idle",
        "the member already holding one is not picked again"
    );
    drop(hold);
    p.delete_balancer(&lb.meta.id).await.unwrap();
}

#[tokio::test]
async fn source_ip_keeps_one_client_on_one_member_over_http() {
    let addrs = vec![
        speaker(b"one").await,
        speaker(b"two").await,
        speaker(b"thr").await,
    ];
    let p = RelayL7Provider::new();
    let lb = balancer(None);
    let realized = p.ensure_balancer(&lb).await.unwrap();
    let view = algorithm_view(
        &lb,
        ListenerProtocol::Http,
        addrs,
        vec![1, 1, 1],
        Algorithm::SourceIp,
    );
    p.ensure_listener(&view).await.unwrap();
    // Every request here arrives from 127.0.0.1, so all of them must land
    // on the one member that address hashes to. Against three members,
    // plain round robin would answer `one` three times in nine.
    let seen = ask_n(&realized.provider_ref, 9).await;
    assert_eq!(
        tally(&seen).len(),
        1,
        "one client address moved between members: {seen:?}"
    );
    p.delete_balancer(&lb.meta.id).await.unwrap();
}

#[tokio::test]
async fn an_unhealthy_member_is_never_picked_over_http() {
    let addrs = [speaker(b"one").await, speaker(b"two").await];
    for algo in [
        Algorithm::RoundRobin,
        Algorithm::LeastConn,
        Algorithm::SourceIp,
    ] {
        let p = RelayL7Provider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        // The second member answers, but is Offline, so the reconciler
        // never hands it to the proxy at all.
        let view = algorithm_view(&lb, ListenerProtocol::Http, vec![addrs[0]], vec![1], algo);
        p.ensure_listener(&view).await.unwrap();
        for body in ask_n(&realized.provider_ref, 4).await {
            assert_eq!(body, b"one", "{algo}: the offline member took traffic");
        }
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }
}
