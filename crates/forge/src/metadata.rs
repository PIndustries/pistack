// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! EC2-style guest metadata. The process binds loopback only. The host
//! helper DNATs `169.254.169.254:80` on a pistack bridge to this port,
//! and the TCP peer address is the guest's fixed IP.
//!
//! OVN has no such bridge. There the host agent answers the guest from a
//! namespace per network and asks here over `/rpc` ([`agent_routes`]),
//! naming the network the request arrived on and the guest's address. The
//! answer is given only for a guest placed on the asking host.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use fabric::{FloatingIp, Port, Subnet};
use pistack_rpc::{MetadataAnswer, MetadataNetwork, MetadataQuery};
use pistack_store::Store;
use pistack_types::{kind, ApiResult, Id};
use std::collections::{BTreeSet, HashSet};
use tokio_util::sync::CancellationToken;

use crate::types::{Host, Machine};

const DEFAULT_LISTEN: &str = "127.0.0.1:18880";

/// Start the metadata server. A bad address or a taken port is logged and
/// this task returns. It does not stop the rest of forge.
pub async fn serve(store: Store, shutdown: CancellationToken) {
    let Some(addr) = listen_from_env() else {
        return;
    };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%addr, error = %e, "forge: metadata listen failed");
            return;
        }
    };
    tracing::info!(%addr, "forge: metadata listening");
    let app = Router::new().fallback(handle).with_state(store);
    let svc = app.into_make_service_with_connect_info::<SocketAddr>();
    let server = axum::serve(listener, svc)
        .with_graceful_shutdown(async move { shutdown.cancelled().await });
    if let Err(e) = server.await {
        tracing::error!(error = %e, "forge: metadata server stopped");
    }
}

fn listen_from_env() -> Option<SocketAddr> {
    let text = std::env::var("PISTACK_METADATA_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.into());
    match parse_listen(&text) {
        Ok(addr) => Some(addr),
        Err(msg) => {
            tracing::error!(listen = %text, "{msg}");
            None
        }
    }
}

/// Refuse an unspecified address. `0.0.0.0` and `::` would answer on the
/// public NIC, and this service has no credentials of its own.
pub fn parse_listen(text: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = text
        .parse()
        .map_err(|_| format!("metadata listen {text} is not an address"))?;
    if addr.ip().is_unspecified() {
        return Err(format!("metadata listen {addr} must not be 0.0.0.0 or ::"));
    }
    Ok(addr)
}

async fn handle(
    State(store): State<Store>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
) -> Response {
    let (code, body) = answer(&store, peer.ip(), req.uri().path());
    (
        code,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}

struct Guest {
    name: String,
    local: String,
    public: String,
    key: Option<String>,
    user_data: String,
    project: Option<Id>,
    secrets: Vec<String>,
}

/// Routes for host agents, to sit beside the rest of `/rpc` behind the
/// same checks.
///
/// * `GET /metadata/networks/{node}`: the networks with a guest on that
///   host, empty unless the networks are OVN.
/// * `POST /metadata/guest`: one guest's metadata request, passed on by
///   the host it runs on.
pub fn agent_routes(store: Store) -> Router {
    Router::new()
        .route("/metadata/networks/{node}", get(agent_networks))
        .route("/metadata/guest", post(agent_guest))
        .with_state(store)
}

async fn agent_networks(
    State(store): State<Store>,
    Path(node): Path<String>,
    ext: axum::http::Extensions,
) -> ApiResult<Json<Vec<MetadataNetwork>>> {
    pistack_rpc::check_peer_node(&ext, &node)?;
    if !crate::ovn::enabled() {
        return Ok(Json(Vec::new()));
    }
    Ok(Json(host_networks(&store, &node)?))
}

async fn agent_guest(
    State(store): State<Store>,
    ext: axum::http::Extensions,
    Json(q): Json<MetadataQuery>,
) -> ApiResult<Json<MetadataAnswer>> {
    pistack_rpc::check_peer_node(&ext, &q.node_name)?;
    let (status, body) = answer_on(&store, &q.node_name, &q.network, q.address, &q.path);
    Ok(Json(MetadataAnswer {
        status: status.as_u16(),
        body,
    }))
}

/// The networks that have a port of a machine placed on host `node`, with
/// each one's metadata port and IPv4 subnets. A host nobody registered has
/// none.
pub fn host_networks(store: &Store, node: &str) -> ApiResult<Vec<MetadataNetwork>> {
    let Some(host) = host_named(store, node)? else {
        return Ok(Vec::new());
    };
    let here: HashSet<Id> = store
        .collection::<Machine>()
        .list(None)?
        .into_iter()
        .filter(|m| m.status.host.as_ref() == Some(&host))
        .map(|m| m.meta.id.clone())
        .collect();
    let networks: BTreeSet<Id> = store
        .collection::<Port>()
        .list(None)?
        .into_iter()
        .filter(|p| p.spec.device.as_ref().is_some_and(|d| here.contains(d)))
        .map(|p| p.spec.network)
        .collect();
    if networks.is_empty() {
        return Ok(Vec::new());
    }
    let subnets = store.collection::<Subnet>().list(None)?;
    Ok(networks
        .into_iter()
        .map(|net| MetadataNetwork {
            network: net.to_string(),
            port: fabric::backend::ovn::metadata_port_name(&net),
            mac: fabric::backend::ovn::metadata_port_mac(&net),
            cidrs: subnets
                .iter()
                .filter(|s| s.spec.network == net && s.spec.cidr.addr().is_ipv4())
                .map(|s| s.spec.cidr.trunc().to_string())
                .collect(),
        })
        .collect())
}

fn host_named(store: &Store, node: &str) -> ApiResult<Option<Id>> {
    Ok(store
        .collection::<Host>()
        .list(None)?
        .into_iter()
        .find(|h| h.spec.node_name == node)
        .map(|h| h.meta.id.clone()))
}

/// [`answer`] for a request that host `node` passed on from `network`.
/// The guest is the machine that holds `peer` on that network, and it must
/// be placed on that host: a host cannot read for a guest it does not run,
/// and the same address on another network is another guest.
pub fn answer_on(
    store: &Store,
    node: &str,
    network: &str,
    peer: IpAddr,
    path: &str,
) -> (StatusCode, Vec<u8>) {
    let not_found = (StatusCode::NOT_FOUND, Vec::new());
    let Some(network) = Id::parse(kind::NETWORK, network) else {
        return not_found;
    };
    let Ok(Some(host)) = host_named(store, node) else {
        return not_found;
    };
    match lookup(store, peer, Some((&network, &host))) {
        Some(guest) => respond(store, guest, path),
        None => not_found,
    }
}

/// Body for one metadata path. Unknown peers and unknown paths are 404.
/// The request path is not logged. Secret payloads are not logged.
pub fn answer(store: &Store, peer: IpAddr, path: &str) -> (StatusCode, Vec<u8>) {
    match lookup(store, peer, None) {
        Some(guest) => respond(store, guest, path),
        None => (StatusCode::NOT_FOUND, Vec::new()),
    }
}

fn respond(store: &Store, guest: Guest, path: &str) -> (StatusCode, Vec<u8>) {
    let path = path.trim_end_matches('/');
    if let Some(name) = path.strip_prefix("/latest/meta-data/secrets/") {
        return named_secret(store, &guest, name);
    }
    let body = match path {
        "/latest/meta-data" => {
            Some(b"hostname\nlocal-ipv4\npublic-ipv4\npublic-keys\nsecrets".to_vec())
        }
        // A value is the bare value and a listing has no last newline, as
        // the EC2 service answers. `$(curl ...)` hid the difference; a
        // program that reads the body whole did not.
        "/latest/meta-data/hostname" => Some(guest.name.into_bytes()),
        "/latest/meta-data/local-ipv4" => Some(guest.local.into_bytes()),
        "/latest/meta-data/public-ipv4" => Some(guest.public.into_bytes()),
        "/latest/meta-data/public-keys" => guest.key.as_ref().map(|_| b"0".to_vec()),
        "/latest/meta-data/public-keys/0/openssh-key" => {
            guest.key.map(|k| k.trim_end().to_owned().into_bytes())
        }
        "/latest/meta-data/secrets" => Some(secret_names(&guest.secrets)),
        "/latest/user-data" => Some(guest.user_data.into_bytes()),
        _ => None,
    };
    match body {
        Some(body) => (StatusCode::OK, body),
        None => (StatusCode::NOT_FOUND, Vec::new()),
    }
}

fn secret_names(names: &[String]) -> Vec<u8> {
    names.join("\n").into_bytes()
}

/// Payload of a secret named on this machine, in the machine's project.
/// Any other name is 404. A keep store that will not open is 503, and the
/// body is not a payload.
fn named_secret(store: &Store, guest: &Guest, name: &str) -> (StatusCode, Vec<u8>) {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\0')
        || name == "."
        || name == ".."
        || !guest.secrets.iter().any(|s| s == name)
    {
        return (StatusCode::NOT_FOUND, Vec::new());
    }
    let client = match keep::Client::open(store) {
        Ok(c) => c,
        Err(_) => {
            tracing::error!("forge: metadata secret store is unavailable");
            return (StatusCode::SERVICE_UNAVAILABLE, b"unavailable".to_vec());
        }
    };
    let listed = match client.list_secrets(guest.project.as_ref()) {
        Ok(v) => v,
        Err(_) => return (StatusCode::NOT_FOUND, Vec::new()),
    };
    let Some(secret) = listed
        .into_iter()
        .find(|s| s.meta.name == name && s.meta.project == guest.project)
    else {
        return (StatusCode::NOT_FOUND, Vec::new());
    };
    match client.get_secret_payload(&secret.meta.id) {
        Ok(bytes) => (StatusCode::OK, bytes),
        Err(_) => (StatusCode::NOT_FOUND, Vec::new()),
    }
}

/// The machine that holds `peer`. `scope` narrows it to a network and the
/// host the machine must be placed on.
fn lookup(store: &Store, peer: IpAddr, scope: Option<(&Id, &Id)>) -> Option<Guest> {
    let port = store
        .collection::<Port>()
        .list(None)
        .ok()?
        .into_iter()
        .find(|p| {
            p.spec.fixed_ips.contains(&peer)
                && scope.is_none_or(|(network, _)| &p.spec.network == network)
                && p.spec
                    .device
                    .as_ref()
                    .is_some_and(|d| d.kind == kind::MACHINE)
        })?;
    let device: Id = port.spec.device.clone()?;
    let machine = store.collection::<Machine>().get(&device).ok()?;
    if let Some((_, host)) = scope {
        if machine.status.host.as_ref() != Some(host) {
            return None;
        }
    }
    let public = store
        .collection::<FloatingIp>()
        .list(None)
        .ok()
        .into_iter()
        .flatten()
        .find(|f| f.spec.port.as_ref() == Some(&port.meta.id))
        .and_then(|f| f.spec.address)
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| peer.to_string());
    let key = machine
        .spec
        .ssh_keys
        .iter()
        .find(|k| !k.trim().is_empty())
        .cloned();
    Some(Guest {
        name: machine.meta.name.clone(),
        local: peer.to_string(),
        public,
        key,
        user_data: machine.spec.user_data.clone().unwrap_or_default(),
        project: machine.meta.project.clone(),
        secrets: machine.spec.secrets.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{new_machine, MachineSpec};
    use fabric::{FloatingIpSpec, NetworkSpec, Port, PortSpec, SubnetSpec};
    use pistack_types::{kind, Meta};
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
}
