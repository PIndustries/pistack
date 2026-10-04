// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! EC2-style guest metadata. The process binds loopback only. The host
//! helper DNATs `169.254.169.254:80` on a pistack bridge to this port,
//! and the TCP peer address is the guest's fixed IP.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use fabric::{FloatingIp, Port};
use pistack_store::Store;
use pistack_types::{kind, Id};
use tokio_util::sync::CancellationToken;

use crate::types::Machine;

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

/// Body for one metadata path. Unknown peers and unknown paths are 404.
/// The request path is not logged. Secret payloads are not logged.
pub fn answer(store: &Store, peer: IpAddr, path: &str) -> (StatusCode, Vec<u8>) {
    let Some(guest) = lookup(store, peer) else {
        return (StatusCode::NOT_FOUND, Vec::new());
    };
    let path = path.trim_end_matches('/');
    if let Some(name) = path.strip_prefix("/latest/meta-data/secrets/") {
        return named_secret(store, &guest, name);
    }
    let body = match path {
        "/latest/meta-data" => {
            Some(b"hostname\nlocal-ipv4\npublic-ipv4\npublic-keys\nsecrets\n".to_vec())
        }
        "/latest/meta-data/hostname" => Some(format!("{}\n", guest.name).into_bytes()),
        "/latest/meta-data/local-ipv4" => Some(format!("{}\n", guest.local).into_bytes()),
        "/latest/meta-data/public-ipv4" => Some(format!("{}\n", guest.public).into_bytes()),
        "/latest/meta-data/public-keys" => guest.key.as_ref().map(|_| b"0\n".to_vec()),
        "/latest/meta-data/public-keys/0/openssh-key" => {
            guest.key.as_ref().map(|k| format!("{k}\n").into_bytes())
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
    if names.is_empty() {
        return Vec::new();
    }
    let mut body = names.join("\n");
    body.push('\n');
    body.into_bytes()
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

fn lookup(store: &Store, peer: IpAddr) -> Option<Guest> {
    let port = store
        .collection::<Port>()
        .list(None)
        .ok()?
        .into_iter()
        .find(|p| {
            p.spec.fixed_ips.contains(&peer)
                && p.spec
                    .device
                    .as_ref()
                    .is_some_and(|d| d.kind == kind::MACHINE)
        })?;
    let device: Id = port.spec.device.clone()?;
    let machine = store.collection::<Machine>().get(&device).ok()?;
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
        assert_eq!(body, b"10.90.0.10\n");
        let (_, host) = answer(&store, peer, "/latest/meta-data/hostname");
        assert_eq!(host, b"box\n");
        let (_, pubip) = answer(&store, peer, "/latest/meta-data/public-ipv4");
        assert_eq!(pubip, b"10.90.0.10\n");
        let (_, key) = answer(&store, peer, "/latest/meta-data/public-keys/0/openssh-key");
        assert_eq!(key, b"ssh-ed25519 AAAAbox\n");
        let (_, data) = answer(&store, peer, "/latest/user-data");
        assert_eq!(data, b"#cloud-config\n");

        let mut ext = NetworkSpec::default();
        ext.external = true;
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
        assert_eq!(pubip, b"10.66.66.84\n");
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
        assert_eq!(names, b"db-password\n");
        let (code, body) = answer(&store, peer, "/latest/meta-data/secrets/db-password");
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body, b"s3cr3t");
        let (code, body) = answer(&store, peer, "/latest/meta-data/secrets/other");
        assert_eq!(code, StatusCode::NOT_FOUND);
        assert!(body.is_empty());
        let (code, _) = answer(&store, peer, "/latest/meta-data/secrets/missing");
        assert_eq!(code, StatusCode::NOT_FOUND);
    }
}
