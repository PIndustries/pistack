// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Answer `169.254.169.254` for guests on OVN networks.
//!
//! OVN gives a guest no path to the host, so the bridge DNAT the other
//! datapaths use does not exist here. Instead, for every network that has a
//! guest on this host, the agent keeps:
//!
//! * a network namespace named after the network's metadata port,
//!   `pimeta-<network uuid>`;
//! * a veth pair, one end on `br-int` with `iface-id` set to that port (a
//!   `localport` in OVN, so traffic to it never leaves the host), the other
//!   end `meta0` inside the namespace holding `169.254.169.254`;
//! * a listener on `169.254.169.254:80` inside the namespace.
//!
//! Who is asking is decided by the datapath alone: the namespace a
//! connection arrived in names the network, and the TCP peer address is
//! the guest's fixed IP. Nothing in the request is trusted, and only its
//! path is passed on. The agent asks the server over its own channel, and
//! the server answers only for a guest it placed on this host.
//!
//! The server says which networks those are ([`MetadataNetwork`]). A
//! network that is no longer listed loses its namespace, port and listener.
//! Nothing here listens on a host address: the only socket is inside the
//! namespace.
//!
//! The agent must be root: it creates namespaces and opens a socket inside
//! each. A non-root agent serves no metadata on OVN and says so once.

use async_trait::async_trait;
use pistack_rpc::proto::MetadataNetwork;
use pistack_rpc::AgentClient;
use pistack_types::{ApiError, ApiResult};
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use crate::ovnhost::Runner;

const METADATA_ADDR: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
const INTEGRATION_BRIDGE: &str = "br-int";
/// The namespace, and the OVN port the host end is bound to.
const PORT_PREFIX: &str = "pimeta-";
/// The interface inside the namespace.
const INNER: &str = "meta0";
/// Guests of one network served at once. More are refused, not queued.
const MAX_CONNECTIONS: usize = 64;
const HEAD_LIMIT: usize = 8 * 1024;
const PATH_LIMIT: usize = 512;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Where the answers come from. The agent's server connection in real use.
#[async_trait]
pub(crate) trait Upstream: Send + Sync {
    /// Status and body for `path`, asked by the guest holding `address` on
    /// `network`.
    async fn fetch(&self, network: &str, address: IpAddr, path: &str) -> ApiResult<(u16, Vec<u8>)>;
}

#[async_trait]
impl Upstream for AgentClient {
    async fn fetch(&self, network: &str, address: IpAddr, path: &str) -> ApiResult<(u16, Vec<u8>)> {
        self.metadata(network, address, path)
            .await
            .map_err(|e| ApiError::unavailable(e.to_string()))
    }
}

/// Opens the listener inside a namespace. Tests bind loopback instead.
#[async_trait]
pub(crate) trait NsListen: Send + Sync {
    async fn listen(&self, netns: &str) -> ApiResult<TcpListener>;
}

struct SystemListen;

#[async_trait]
impl NsListen for SystemListen {
    async fn listen(&self, netns: &str) -> ApiResult<TcpListener> {
        let path = format!("/run/netns/{netns}");
        let (tx, rx) = tokio::sync::oneshot::channel();
        // A thread of its own: entering the namespace changes the thread,
        // and this one ends as soon as the socket is bound. The socket
        // stays in the namespace it was made in.
        std::thread::Builder::new()
            .name("pimeta-bind".into())
            .spawn(move || {
                let _ = tx.send(bind_in_netns(&path));
            })
            .map_err(|e| ApiError::unavailable(format!("metadata listener thread: {e}")))?;
        let bound = rx
            .await
            .map_err(|_| ApiError::unavailable("metadata listener thread died"))?
            .map_err(|e| ApiError::unavailable(format!("metadata listener in {netns}: {e}")))?;
        TcpListener::from_std(bound)
            .map_err(|e| ApiError::unavailable(format!("metadata listener in {netns}: {e}")))
    }
}

fn bind_in_netns(path: &str) -> std::io::Result<std::net::TcpListener> {
    let ns = std::fs::File::open(path)?;
    enter_netns(&ns)?;
    let listener = std::net::TcpListener::bind((METADATA_ADDR, 80))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

#[cfg(target_os = "linux")]
fn enter_netns(ns: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: the descriptor is open for the whole call, and setns with
    // CLONE_NEWNET moves only the calling thread.
    if unsafe { libc::setns(ns.as_raw_fd(), libc::CLONE_NEWNET) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn enter_netns(_ns: &std::fs::File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "network namespaces need Linux",
    ))
}

/// One network's metadata port on this host, checked and named.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    network: String,
    /// The OVN port name. Also the namespace name.
    port: String,
    /// Host end of the veth, on `br-int`.
    veth: String,
    mac: String,
    cidrs: Vec<String>,
}

impl Target {
    /// Everything here ends up as a command argument, so nothing is taken
    /// that is not exactly the expected shape.
    fn parse(net: &MetadataNetwork) -> ApiResult<Self> {
        let bad = |what: &str| ApiError::bad_request(format!("metadata network: bad {what}"));
        let veth = veth_name(&net.port).ok_or_else(|| bad("port name"))?;
        if net.network.is_empty()
            || net.network.len() > 64
            || !net
                .network
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(bad("network id"));
        }
        let mac_ok = net.mac.len() == 17
            && net.mac.split(':').count() == 6
            && net
                .mac
                .split(':')
                .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
        if !mac_ok {
            return Err(bad("mac"));
        }
        let mut cidrs = Vec::new();
        for cidr in &net.cidrs {
            let ok = cidr.split_once('/').is_some_and(|(ip, len)| {
                ip.parse::<Ipv4Addr>().is_ok() && len.parse::<u8>().is_ok_and(|n| n <= 32)
            });
            if !ok {
                return Err(bad("subnet"));
            }
            cidrs.push(cidr.clone());
        }
        cidrs.sort();
        cidrs.dedup();
        Ok(Self {
            network: net.network.clone(),
            port: net.port.clone(),
            veth,
            mac: net.mac.to_ascii_lowercase(),
            cidrs,
        })
    }
}

/// `pm` and the last twelve hex digits of the uuid in `pimeta-<uuid>`: the
/// random end, and fourteen characters, inside the kernel's limit of
/// fifteen. `None` when `port` is not such a name.
fn veth_name(port: &str) -> Option<String> {
    let uuid = port.strip_prefix(PORT_PREFIX)?;
    let shape_ok = uuid.len() == 36
        && uuid.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
        });
    shape_ok.then(|| format!("pm{}", &uuid[24..]))
}

struct Served {
    target: Target,
    proxy: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct State {
    served: BTreeMap<String, Served>,
    /// False until the namespaces a previous agent left were looked at.
    looked: bool,
    /// The last problem logged, so a standing one is said once.
    said: Option<String>,
}

/// The metadata namespaces of this host.
pub(crate) struct MetaNets {
    runner: Arc<dyn Runner>,
    listen: Arc<dyn NsListen>,
    /// `ovs-vsctl` as an argv prefix. `None` finds it on each use.
    vsctl: Option<Vec<String>>,
    ip: String,
    root: bool,
    state: tokio::sync::Mutex<State>,
    /// Held from asking the server to applying its answer, so an older
    /// list is never applied after a newer one.
    asking: tokio::sync::Mutex<()>,
}

impl MetaNets {
    /// The real tools on this host.
    pub(crate) fn system() -> Self {
        Self {
            runner: Arc::new(crate::ovnhost::SystemRunner),
            listen: Arc::new(SystemListen),
            vsctl: None,
            ip: crate::facts::which("ip")
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "ip".to_owned()),
            root: crate::ovnhost::agent_is_root(),
            state: Default::default(),
            asking: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with(runner: Arc<dyn Runner>, listen: Arc<dyn NsListen>, root: bool) -> Self {
        Self {
            runner,
            listen,
            vsctl: Some(vec!["ovs-vsctl".to_owned()]),
            ip: "ip".to_owned(),
            root,
            state: Default::default(),
            asking: Default::default(),
        }
    }

    /// Ask the server which networks have a guest here and make this host
    /// match. A failure is logged when it starts and when it changes; the
    /// next heartbeat tries again.
    pub(crate) async fn refresh(&self, client: &AgentClient) {
        // The heartbeat and a machine start both come here. Without this
        // the heartbeat's older list could undo what the start just made.
        let _asking = self.asking.lock().await;
        let outcome = match client.metadata_networks().await {
            Ok(want) => self.sync(&want, Arc::new(client.clone())).await,
            Err(e) => Err(ApiError::unavailable(format!(
                "the server did not list this host's networks: {e}"
            ))),
        };
        let problem = outcome.err().map(|e| e.message);
        let mut state = self.state.lock().await;
        if state.said != problem {
            match &problem {
                Some(p) => {
                    tracing::warn!(reason = %p, "anvil: metadata for OVN guests is not set up")
                }
                None => tracing::info!("anvil: metadata for OVN guests is set up"),
            }
            state.said = problem;
        }
    }

    /// Serve exactly `want`. Networks already served as listed cost one
    /// `ip netns list`; with nothing listed and nothing served, not even
    /// that. One network failing does not stop the others; the first error
    /// is returned.
    pub(crate) async fn sync(
        &self,
        want: &[MetadataNetwork],
        upstream: Arc<dyn Upstream>,
    ) -> ApiResult<()> {
        let mut state = self.state.lock().await;
        if want.is_empty() && state.served.is_empty() && state.looked {
            return Ok(());
        }
        if !self.root {
            // Nothing was ever made by this agent, so with nothing wanted
            // there is nothing to say.
            if want.is_empty() {
                return Ok(());
            }
            return Err(ApiError::unavailable(
                "the agent is not root, so it cannot make the metadata namespaces. Guests on OVN networks get no metadata from this host",
            ));
        }
        let mut first_error = None;
        let mut targets = BTreeMap::new();
        for net in want {
            match Target::parse(net) {
                Ok(t) => {
                    targets.insert(t.port.clone(), t);
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        let existing = self.namespaces().await?;
        state.looked = true;

        // Gone from the list, or left by an agent that ran before.
        let stale: Vec<String> = existing
            .iter()
            .chain(state.served.keys())
            .filter(|ns| !targets.contains_key(*ns))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for ns in stale {
            if let Some(served) = state.served.remove(&ns) {
                served.proxy.abort();
            }
            match self.remove(&ns, existing.contains(&ns)).await {
                Ok(()) => tracing::info!(netns = %ns, "anvil: metadata namespace removed"),
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }

        for (ns, target) in targets {
            let settled = state.served.get(&ns).is_some_and(|s| {
                s.target == target && !s.proxy.is_finished() && existing.contains(&ns)
            });
            if settled {
                continue;
            }
            let before = state.served.remove(&ns);
            if let Some(old) = &before {
                old.proxy.abort();
            }
            let old_cidrs = before.map(|s| s.target.cidrs).unwrap_or_default();
            let made = async {
                self.ensure(&target, existing.contains(&ns), &old_cidrs)
                    .await?;
                self.listen.listen(&ns).await
            }
            .await;
            match made {
                Ok(listener) => {
                    tracing::info!(netns = %ns, network = %target.network, "anvil: metadata namespace ready");
                    let proxy =
                        tokio::spawn(serve(listener, target.network.clone(), upstream.clone()));
                    state.served.insert(ns, Served { target, proxy });
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn ip(&self, args: &[&str]) -> ApiResult<String> {
        let mut argv = vec![self.ip.clone()];
        argv.extend(args.iter().map(|a| (*a).to_owned()));
        self.runner.run(&argv).await
    }

    async fn vsctl(&self, args: &[&str]) -> ApiResult<String> {
        let mut argv = match &self.vsctl {
            Some(v) => v.clone(),
            None => crate::ovnhost::Tools::from_system()?.vsctl,
        };
        argv.extend(args.iter().map(|a| (*a).to_owned()));
        self.runner.run(&argv).await
    }

    /// The metadata namespaces that exist. `ip netns list` prints a name
    /// per line, followed by ` (id: N)` once the namespace has a peer.
    async fn namespaces(&self) -> ApiResult<BTreeSet<String>> {
        let out = self.ip(&["netns", "list"]).await?;
        Ok(out
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| veth_name(name).is_some())
            .map(str::to_owned)
            .collect())
    }

    /// Namespace, veth, address, routes and the port on `br-int`. Every
    /// step can be repeated.
    async fn ensure(&self, t: &Target, exists: bool, old_cidrs: &[String]) -> ApiResult<()> {
        let ns = t.port.as_str();
        if !exists {
            self.ip(&["netns", "add", ns]).await?;
        }
        if self.ip(&["-n", ns, "link", "show", INNER]).await.is_err() {
            // A host end without its peer is left from a namespace that
            // was deleted by hand.
            let _ = self.ip(&["link", "del", &t.veth]).await;
            self.ip(&[
                "link", "add", &t.veth, "type", "veth", "peer", "name", INNER, "netns", ns,
            ])
            .await?;
        }
        self.ip(&["-n", ns, "link", "set", INNER, "address", &t.mac])
            .await?;
        self.ip(&["-n", ns, "link", "set", "lo", "up"]).await?;
        self.ip(&["-n", ns, "link", "set", INNER, "up"]).await?;
        let address = format!("{METADATA_ADDR}/32");
        self.ip(&["-n", ns, "addr", "replace", &address, "dev", INNER])
            .await?;
        for gone in old_cidrs.iter().filter(|c| !t.cidrs.contains(c)) {
            let _ = self
                .ip(&["-n", ns, "route", "del", gone, "dev", INNER])
                .await;
        }
        // The way back to the guests: they are on this link.
        for cidr in &t.cidrs {
            self.ip(&["-n", ns, "route", "replace", cidr, "dev", INNER])
                .await?;
        }
        self.ip(&["link", "set", &t.veth, "up"]).await?;
        let iface_id = format!("external_ids:iface-id={}", t.port);
        self.vsctl(&[
            "--may-exist",
            "add-port",
            INTEGRATION_BRIDGE,
            &t.veth,
            "--",
            "set",
            "Interface",
            &t.veth,
            &iface_id,
        ])
        .await?;
        Ok(())
    }

    /// Take the port off the bridge and delete the namespace, which takes
    /// both ends of the veth with it.
    async fn remove(&self, ns: &str, exists: bool) -> ApiResult<()> {
        let Some(veth) = veth_name(ns) else {
            return Ok(());
        };
        self.vsctl(&["--if-exists", "del-port", INTEGRATION_BRIDGE, &veth])
            .await?;
        if exists {
            self.ip(&["netns", "del", ns]).await?;
        }
        Ok(())
    }
}

/// Accept guests of one network until the task is aborted.
async fn serve(listener: TcpListener, network: String, upstream: Arc<dyn Upstream>) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (socket, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                tracing::debug!(error = %e, "anvil: metadata accept failed");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let network = network.clone();
        let upstream = upstream.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let _ = tokio::time::timeout(
                REQUEST_TIMEOUT,
                answer(socket, peer.ip(), &network, upstream.as_ref()),
            )
            .await;
        });
    }
}

/// One request, one answer, then close. Only the method and the path are
/// read from what the guest sent.
async fn answer(mut socket: TcpStream, peer: IpAddr, network: &str, upstream: &dyn Upstream) {
    let head = match tokio::time::timeout(READ_TIMEOUT, read_head(&mut socket)).await {
        Ok(Some(head)) => head,
        _ => return,
    };
    let (status, body, head_only) = match request_line(&head) {
        None => (400, Vec::new(), false),
        Some((method, _)) if method != "GET" && method != "HEAD" => (405, Vec::new(), false),
        Some((method, path)) => match upstream.fetch(network, peer, path).await {
            Ok((status, body)) => (status, body, method == "HEAD"),
            Err(e) => {
                // The path may name a secret, so it is not logged.
                tracing::debug!(%network, %peer, error = %e.message, "anvil: metadata upstream failed");
                (
                    503,
                    b"metadata is not available\n".to_vec(),
                    method == "HEAD",
                )
            }
        },
    };
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "",
    };
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    if !head_only {
        out.extend_from_slice(&body);
    }
    let _ = socket.write_all(&out).await;
    let _ = socket.shutdown().await;
}

/// The request up to the blank line. `None` when the guest closed first or
/// sent more than [`HEAD_LIMIT`] without one.
async fn read_head(socket: &mut TcpStream) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        head.extend_from_slice(&chunk[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(head);
        }
        if head.len() > HEAD_LIMIT {
            return None;
        }
    }
}

/// Method and path of `GET /path?query HTTP/1.1`. The query is dropped.
fn request_line(head: &[u8]) -> Option<(&str, &str)> {
    let line = head.split(|b| *b == b'\r').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return None;
    }
    let path = target.split('?').next()?;
    let ok = path.starts_with('/')
        && path.len() <= PATH_LIMIT
        && path.chars().all(|c| c.is_ascii_graphic());
    ok.then_some((method, path))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Mutex;

    pub(crate) const NET_A: &str = "net-01a10318-a507-74fd-a86e-5acbda974722";
    pub(crate) const PORT_A: &str = "pimeta-01a10318-a507-74fd-a86e-5acbda974722";
    const VETH_A: &str = "pm5acbda974722";
    const NET_B: &str = "net-01a10318-a507-74fd-a86e-0000000000bb";
    const PORT_B: &str = "pimeta-01a10318-a507-74fd-a86e-0000000000bb";
    const VETH_B: &str = "pm0000000000bb";

    pub(crate) fn net(network: &str, port: &str, cidrs: &[&str]) -> MetadataNetwork {
        MetadataNetwork {
            network: network.into(),
            port: port.into(),
            mac: "06:5a:cb:da:97:47".into(),
            cidrs: cidrs.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    /// A host's namespaces, links and `br-int` ports, kept in memory, that
    /// answers the `ip` and `ovs-vsctl` commands this module sends the way
    /// the real tools print.
    #[derive(Default)]
    pub(crate) struct FakeHost {
        state: Mutex<HostState>,
    }

    #[derive(Default)]
    struct HostState {
        /// Namespace to the links inside it.
        namespaces: BTreeMap<String, BTreeSet<String>>,
        host_links: BTreeSet<String>,
        /// `br-int` port to its `iface-id`.
        ports: BTreeMap<String, String>,
        routes: BTreeMap<String, BTreeSet<String>>,
        addrs: BTreeMap<String, BTreeSet<String>>,
        log: Vec<String>,
    }

    impl FakeHost {
        fn take_log(&self) -> Vec<String> {
            std::mem::take(&mut self.state.lock().unwrap().log)
        }
        pub(crate) fn namespaces(&self) -> Vec<String> {
            self.state
                .lock()
                .unwrap()
                .namespaces
                .keys()
                .cloned()
                .collect()
        }
        fn port(&self, veth: &str) -> Option<String> {
            self.state.lock().unwrap().ports.get(veth).cloned()
        }
        fn routes(&self, ns: &str) -> Vec<String> {
            let st = self.state.lock().unwrap();
            st.routes.get(ns).into_iter().flatten().cloned().collect()
        }
    }

    #[async_trait]
    impl Runner for FakeHost {
        async fn run(&self, argv: &[String]) -> ApiResult<String> {
            let mut st = self.state.lock().unwrap();
            st.log.push(argv.join(" "));
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            let missing = |what: &str| Err(ApiError::unavailable(format!("{what}: not found")));
            match args.as_slice() {
                ["ip", "netns", "list"] => Ok(st
                    .namespaces
                    .iter()
                    .map(|(name, links)| {
                        if links.is_empty() {
                            format!("{name}\n")
                        } else {
                            format!("{name} (id: 3)\n")
                        }
                    })
                    .collect::<String>()
                    + "qrouter-someone-elses\n"),
                ["ip", "netns", "add", ns] => {
                    st.namespaces.insert((*ns).into(), BTreeSet::new());
                    Ok(String::new())
                }
                ["ip", "netns", "del", ns] => {
                    let Some(links) = st.namespaces.remove(*ns) else {
                        return missing("namespace");
                    };
                    // The peer of a veth goes with it.
                    if !links.is_empty() {
                        let veth = veth_name(ns).unwrap();
                        st.host_links.remove(&veth);
                        st.ports.remove(&veth);
                    }
                    st.routes.remove(*ns);
                    st.addrs.remove(*ns);
                    Ok(String::new())
                }
                ["ip", "link", "add", veth, "type", "veth", "peer", "name", inner, "netns", ns] => {
                    if !st.host_links.insert((*veth).into()) {
                        return Err(ApiError::unavailable("RTNETLINK answers: File exists"));
                    }
                    match st.namespaces.get_mut(*ns) {
                        Some(links) => links.insert((*inner).into()),
                        None => return missing("namespace"),
                    };
                    Ok(String::new())
                }
                ["ip", "link", "del", veth] => {
                    if st.host_links.remove(*veth) {
                        Ok(String::new())
                    } else {
                        missing("link")
                    }
                }
                ["ip", "link", "set", veth, "up"] => {
                    if st.host_links.contains(*veth) {
                        Ok(String::new())
                    } else {
                        missing("link")
                    }
                }
                ["ip", "-n", ns, "link", "show", link] => {
                    if st.namespaces.get(*ns).is_some_and(|l| l.contains(*link)) {
                        Ok(format!("2: {link}@if9: <BROADCAST,MULTICAST,UP>"))
                    } else {
                        missing("link")
                    }
                }
                ["ip", "-n", ns, "link", "set", ..] if st.namespaces.contains_key(*ns) => {
                    Ok(String::new())
                }
                ["ip", "-n", ns, "addr", "replace", addr, "dev", _] => {
                    st.addrs
                        .entry((*ns).into())
                        .or_default()
                        .insert((*addr).into());
                    Ok(String::new())
                }
                ["ip", "-n", ns, "route", "replace", cidr, "dev", _] => {
                    st.routes
                        .entry((*ns).into())
                        .or_default()
                        .insert((*cidr).into());
                    Ok(String::new())
                }
                ["ip", "-n", ns, "route", "del", cidr, "dev", _] => {
                    st.routes.entry((*ns).into()).or_default().remove(*cidr);
                    Ok(String::new())
                }
                ["ovs-vsctl", "--may-exist", "add-port", "br-int", veth, "--", "set", "Interface", again, id] =>
                {
                    assert_eq!(veth, again);
                    let id = id.strip_prefix("external_ids:iface-id=").unwrap();
                    st.ports.insert((*veth).into(), id.into());
                    Ok(String::new())
                }
                ["ovs-vsctl", "--if-exists", "del-port", "br-int", veth] => {
                    st.ports.remove(*veth);
                    Ok(String::new())
                }
                other => panic!("unexpected command {other:?}"),
            }
        }
    }

    /// Listens on loopback in place of the namespace, and says where.
    #[derive(Default)]
    pub(crate) struct FakeListen {
        bound: Mutex<BTreeMap<String, SocketAddr>>,
    }

    impl FakeListen {
        pub(crate) fn addr(&self, ns: &str) -> SocketAddr {
            self.bound.lock().unwrap()[ns]
        }
        pub(crate) fn bound(&self, ns: &str) -> bool {
            self.bound.lock().unwrap().contains_key(ns)
        }
    }

    #[async_trait]
    impl NsListen for FakeListen {
        async fn listen(&self, netns: &str) -> ApiResult<TcpListener> {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            self.bound
                .lock()
                .unwrap()
                .insert(netns.into(), listener.local_addr().unwrap());
            Ok(listener)
        }
    }

    /// Records what the proxy asked and answers like the server would.
    #[derive(Default)]
    struct FakeServer {
        asked: Mutex<Vec<(String, IpAddr, String)>>,
        down: Mutex<bool>,
    }

    #[async_trait]
    impl Upstream for FakeServer {
        async fn fetch(
            &self,
            network: &str,
            address: IpAddr,
            path: &str,
        ) -> ApiResult<(u16, Vec<u8>)> {
            self.asked
                .lock()
                .unwrap()
                .push((network.into(), address, path.into()));
            if *self.down.lock().unwrap() {
                return Err(ApiError::unavailable("no leader"));
            }
            Ok(match path {
                "/latest/meta-data/hostname" => (200, format!("guest-on-{network}\n").into_bytes()),
                _ => (404, Vec::new()),
            })
        }
    }

    struct Rig {
        host: Arc<FakeHost>,
        listen: Arc<FakeListen>,
        server: Arc<FakeServer>,
        nets: MetaNets,
    }

    fn rig() -> Rig {
        let host = Arc::new(FakeHost::default());
        let listen = Arc::new(FakeListen::default());
        Rig {
            nets: MetaNets::with(host.clone(), listen.clone(), true),
            host,
            listen,
            server: Arc::new(FakeServer::default()),
        }
    }

    impl Rig {
        async fn sync(&self, want: &[MetadataNetwork]) -> ApiResult<()> {
            self.nets.sync(want, self.server.clone()).await
        }
    }

    /// Send `request` as a guest would and return the whole reply.
    async fn raw(addr: SocketAddr, request: &str) -> String {
        let mut socket = TcpStream::connect(addr).await.unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut reply = String::new();
        socket.read_to_string(&mut reply).await.unwrap();
        reply
    }

    pub(crate) async fn get(addr: SocketAddr, path: &str) -> String {
        raw(
            addr,
            &format!("GET {path} HTTP/1.1\r\nHost: 169.254.169.254\r\n\r\n"),
        )
        .await
    }

    #[tokio::test]
    async fn each_network_gets_a_namespace_a_port_and_a_listener() {
        let r = rig();
        r.sync(&[
            net(NET_A, PORT_A, &["10.1.0.0/24", "10.1.1.0/24"]),
            net(NET_B, PORT_B, &["10.1.0.0/24"]),
        ])
        .await
        .unwrap();

        assert_eq!(r.host.namespaces(), [PORT_B, PORT_A]);
        // The host end is bound to the network's localport by name.
        assert_eq!(r.host.port(VETH_A).as_deref(), Some(PORT_A));
        assert_eq!(r.host.port(VETH_B).as_deref(), Some(PORT_B));
        assert_eq!(r.host.routes(PORT_A), ["10.1.0.0/24", "10.1.1.0/24"]);
        let log = r.host.take_log();
        let for_a: Vec<&str> = log
            .iter()
            .map(String::as_str)
            .filter(|c| c.contains(PORT_A) || c.contains(VETH_A))
            .collect();
        assert_eq!(
            for_a,
            [
                format!("ip netns add {PORT_A}"),
                format!("ip -n {PORT_A} link show meta0"),
                format!("ip link del {VETH_A}"),
                format!("ip link add {VETH_A} type veth peer name meta0 netns {PORT_A}"),
                format!("ip -n {PORT_A} link set meta0 address 06:5a:cb:da:97:47"),
                format!("ip -n {PORT_A} link set lo up"),
                format!("ip -n {PORT_A} link set meta0 up"),
                format!("ip -n {PORT_A} addr replace 169.254.169.254/32 dev meta0"),
                format!("ip -n {PORT_A} route replace 10.1.0.0/24 dev meta0"),
                format!("ip -n {PORT_A} route replace 10.1.1.0/24 dev meta0"),
                format!("ip link set {VETH_A} up"),
                format!(
                    "ovs-vsctl --may-exist add-port br-int {VETH_A} -- set Interface {VETH_A} external_ids:iface-id={PORT_A}"
                ),
            ]
        );

        // Nothing changed: one look, no writes.
        r.sync(&[
            net(NET_B, PORT_B, &["10.1.0.0/24"]),
            net(NET_A, PORT_A, &["10.1.1.0/24", "10.1.0.0/24"]),
        ])
        .await
        .unwrap();
        assert_eq!(r.host.take_log(), ["ip netns list"]);
    }

    #[tokio::test]
    async fn the_network_and_the_address_come_from_the_connection_not_the_request() {
        let r = rig();
        // Both networks use the same subnet, as two projects may.
        r.sync(&[
            net(NET_A, PORT_A, &["10.1.0.0/24"]),
            net(NET_B, PORT_B, &["10.1.0.0/24"]),
        ])
        .await
        .unwrap();

        let reply = get(r.listen.addr(PORT_A), "/latest/meta-data/hostname").await;
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(
            reply.ends_with(&format!("\r\n\r\nguest-on-{NET_A}\n")),
            "{reply}"
        );
        let reply = get(r.listen.addr(PORT_B), "/latest/meta-data/hostname").await;
        assert!(
            reply.ends_with(&format!("\r\n\r\nguest-on-{NET_B}\n")),
            "{reply}"
        );

        // A guest that claims to be someone else in every header it can set.
        let forged = format!(
            "GET /latest/meta-data/hostname?network={NET_B}&address=10.1.0.9 HTTP/1.1\r\n\
             Host: 169.254.169.254\r\n\
             X-Forwarded-For: 10.1.0.9\r\n\
             X-Pistack-Network: {NET_B}\r\n\
             X-Instance-Id: someone-else\r\n\r\n"
        );
        let reply = raw(r.listen.addr(PORT_A), &forged).await;
        assert!(
            reply.ends_with(&format!("\r\n\r\nguest-on-{NET_A}\n")),
            "{reply}"
        );

        let asked = r.server.asked.lock().unwrap().clone();
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(
            asked,
            [
                (
                    NET_A.to_owned(),
                    loopback,
                    "/latest/meta-data/hostname".to_owned()
                ),
                (
                    NET_B.to_owned(),
                    loopback,
                    "/latest/meta-data/hostname".to_owned()
                ),
                (
                    NET_A.to_owned(),
                    loopback,
                    "/latest/meta-data/hostname".to_owned()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn other_methods_bad_requests_and_a_server_that_is_away() {
        let r = rig();
        r.sync(&[net(NET_A, PORT_A, &["10.1.0.0/24"])])
            .await
            .unwrap();
        let addr = r.listen.addr(PORT_A);

        let reply = get(addr, "/latest/meta-data/nothing").await;
        assert!(reply.starts_with("HTTP/1.1 404 Not Found\r\n"), "{reply}");
        let reply = raw(
            addr,
            "POST /latest/user-data HTTP/1.1\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 405 "), "{reply}");
        let reply = raw(addr, "GET latest HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 400 "), "{reply}");
        let reply = raw(addr, "HEAD /latest/meta-data/hostname HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(reply.ends_with("Connection: close\r\n\r\n"), "{reply}");
        // Only the two well-formed ones reached the server.
        assert_eq!(r.server.asked.lock().unwrap().len(), 2);

        *r.server.down.lock().unwrap() = true;
        let reply = get(addr, "/latest/meta-data/hostname").await;
        assert!(reply.starts_with("HTTP/1.1 503 "), "{reply}");
        assert!(!reply.contains("no leader"), "{reply}");
    }

    #[tokio::test]
    async fn the_last_guest_leaving_takes_the_namespace_port_and_listener() {
        let r = rig();
        let both = [
            net(NET_A, PORT_A, &["10.1.0.0/24"]),
            net(NET_B, PORT_B, &["10.2.0.0/24"]),
        ];
        r.sync(&both).await.unwrap();
        let addr_a = r.listen.addr(PORT_A);
        r.host.take_log();

        r.sync(&both[1..]).await.unwrap();
        assert_eq!(r.host.namespaces(), [PORT_B]);
        assert_eq!(r.host.port(VETH_A), None);
        assert_eq!(r.host.port(VETH_B).as_deref(), Some(PORT_B));
        assert_eq!(
            r.host.take_log(),
            [
                "ip netns list".to_owned(),
                format!("ovs-vsctl --if-exists del-port br-int {VETH_A}"),
                format!("ip netns del {PORT_A}"),
            ]
        );
        let mut closed = false;
        for _ in 0..50 {
            if TcpStream::connect(addr_a).await.is_err() {
                closed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(closed, "the listener of the removed network is closed");
        let reply = get(r.listen.addr(PORT_B), "/latest/meta-data/hostname").await;
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");

        r.sync(&[]).await.unwrap();
        assert!(r.host.namespaces().is_empty());
        r.host.take_log();
        // Nothing served and nothing wanted: the host is not even asked.
        r.sync(&[]).await.unwrap();
        assert!(r.host.take_log().is_empty());
    }

    #[tokio::test]
    async fn what_a_previous_agent_left_is_adopted_or_removed() {
        let r = rig();
        // Namespaces survive the agent. Their listeners do not.
        r.sync(&[
            net(NET_A, PORT_A, &["10.1.0.0/24"]),
            net(NET_B, PORT_B, &["10.2.0.0/24"]),
        ])
        .await
        .unwrap();
        let restarted = MetaNets::with(r.host.clone(), r.listen.clone(), true);
        r.host.take_log();

        restarted
            .sync(&[net(NET_A, PORT_A, &["10.1.0.0/24"])], r.server.clone())
            .await
            .unwrap();
        assert_eq!(r.host.namespaces(), [PORT_A]);
        let log = r.host.take_log();
        assert!(!log.iter().any(|c| c.contains("netns add")), "{log:?}");
        assert!(!log.iter().any(|c| c.contains("link add")), "{log:?}");
        assert!(log.contains(&format!("ip netns del {PORT_B}")), "{log:?}");
        let reply = get(r.listen.addr(PORT_A), "/latest/meta-data/hostname").await;
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");

        // With nothing wanted, the first look still clears what is there.
        let again = MetaNets::with(r.host.clone(), r.listen.clone(), true);
        again.sync(&[], r.server.clone()).await.unwrap();
        assert!(r.host.namespaces().is_empty());
    }

    #[tokio::test]
    async fn a_subnet_that_goes_loses_its_route() {
        let r = rig();
        r.sync(&[net(NET_A, PORT_A, &["10.1.0.0/24", "10.1.1.0/24"])])
            .await
            .unwrap();
        r.sync(&[net(NET_A, PORT_A, &["10.1.1.0/24", "10.1.2.0/24"])])
            .await
            .unwrap();
        assert_eq!(r.host.routes(PORT_A), ["10.1.1.0/24", "10.1.2.0/24"]);
        let reply = get(r.listen.addr(PORT_A), "/latest/meta-data/hostname").await;
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    }

    #[tokio::test]
    async fn names_that_are_not_ours_never_reach_a_command() {
        let r = rig();
        let mut odd_port = net(NET_A, "pimeta-; rm -rf /", &["10.1.0.0/24"]);
        let e = r.sync(std::slice::from_ref(&odd_port)).await.unwrap_err();
        assert!(e.message.contains("port name"), "{e}");
        odd_port.port = "br-int".into();
        assert!(r.sync(std::slice::from_ref(&odd_port)).await.is_err());
        let mut odd_mac = net(NET_A, PORT_A, &["10.1.0.0/24"]);
        odd_mac.mac = "up; reboot".into();
        assert!(r.sync(&[odd_mac]).await.is_err());
        let odd_cidr = net(NET_A, PORT_A, &["default"]);
        assert!(r.sync(&[odd_cidr]).await.is_err());
        let v6 = net(NET_A, PORT_A, &["fd00::/64"]);
        assert!(r.sync(&[v6]).await.is_err());
        assert!(r.host.namespaces().is_empty());
        assert!(r.host.take_log().iter().all(|c| c == "ip netns list"));

        // A good network beside a bad one is still served.
        let e = r
            .sync(&[
                net(NET_A, "pimeta-nope", &[]),
                net(NET_B, PORT_B, &["10.2.0.0/24"]),
            ])
            .await
            .unwrap_err();
        assert!(e.message.contains("port name"), "{e}");
        assert_eq!(r.host.namespaces(), [PORT_B]);
    }

    #[tokio::test]
    async fn an_agent_that_is_not_root_touches_nothing_and_says_why() {
        let host = Arc::new(FakeHost::default());
        let nets = MetaNets::with(host.clone(), Arc::new(FakeListen::default()), false);
        let e = nets
            .sync(
                &[net(NET_A, PORT_A, &["10.1.0.0/24"])],
                Arc::new(FakeServer::default()),
            )
            .await
            .unwrap_err();
        assert!(e.message.contains("not root"), "{e}");
        // With no OVN guest on the host it has nothing to complain about.
        nets.sync(&[], Arc::new(FakeServer::default()))
            .await
            .unwrap();
        assert!(host.take_log().is_empty());
    }

    #[test]
    fn request_lines() {
        assert_eq!(
            request_line(b"GET /latest/user-data HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some(("GET", "/latest/user-data"))
        );
        assert_eq!(
            request_line(b"GET /a?b=c HTTP/1.0\r\n\r\n"),
            Some(("GET", "/a"))
        );
        assert_eq!(request_line(b"GET /a b HTTP/1.1\r\n\r\n"), None);
        assert_eq!(request_line(b"GET http://x/a HTTP/1.1\r\n\r\n"), None);
        assert_eq!(request_line(b"GET /a\r\n\r\n"), None);
        let long = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(PATH_LIMIT));
        assert_eq!(request_line(long.as_bytes()), None);
        assert_eq!(veth_name(PORT_A).as_deref(), Some(VETH_A));
        assert_eq!(
            veth_name("pimeta-01A10318-a507-74fd-a86e-5acbda974722"),
            None
        );
        assert_eq!(
            veth_name("qrouter-01a10318-a507-74fd-a86e-5acbda974722"),
            None
        );
    }
}
