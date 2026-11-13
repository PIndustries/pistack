// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Control-plane bootstrap: open the store, build the module context,
//! mount every module on the gateway, serve.

use crate::gateway;
use anyhow::Context;
use pistack_rpc::{CommandQueue, QueueConfig};
use pistack_store::Store;
use pistack_types::{Leadership, Module, ModuleContext};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub struct ServerOpts {
    pub data_dir: PathBuf,
    pub init: bool,
    pub join: Option<String>,
    /// The fingerprint of the cloud's root CA, for a first join over https.
    pub join_ca: Option<String>,
    /// Accept a first join over plain http.
    pub insecure_join: bool,
    pub listen: String,
    pub public_url: Option<String>,
    pub insecure_http: bool,
    /// Optional HTTPS listener. `None` keeps today's single plain listener.
    pub tls_listen: Option<String>,
    /// What agents are told to call instead of `https://<tls_listen>`.
    pub agent_url: Option<String>,
    /// Dev mode: a local mock host so machines "run" with no agent, and
    /// unauthenticated requests act as admin. Never on in production.
    pub dev: bool,
}

/// Everything the binary assembles before serving.
pub struct Assembly {
    pub modules: Vec<Arc<dyn Module>>,
    pub forge: Arc<forge::Forge>,
    pub queue: CommandQueue,
    /// Image bytes, for the agent download under `/rpc`.
    pub images: Arc<dyn mold::ImageStore>,
}

/// Every module in the binary, in mount order. Adding a module = one line.
/// `listen` is the server `--listen` address; beacon uses it as the cluster
/// hint when `PISTACK_DNS_LISTEN` is unset. `images` is where mold keeps
/// image bytes; slab and the agent download read the same store.
pub fn assemble(
    dev: bool,
    silo_cfg: silo::SiloConfig,
    images: Arc<dyn mold::ImageStore>,
    listen: &str,
    data_dir: &std::path::Path,
) -> Assembly {
    let gate_m = Arc::new(gate::Gate::with_password_file(
        data_dir.join(gate::ADMIN_PASSWORD_FILE),
    ));
    let registry = Arc::new(forge::DriverRegistry::new());
    let queue = CommandQueue::new(QueueConfig {
        ovn: ovn_host_config(
            std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref(),
            std::env::var(OVN_SB_ENV).ok().as_deref(),
            std::env::var(OVN_ENCAP_TYPE_ENV).ok().as_deref(),
        ),
        ..QueueConfig::default()
    });
    // Agents that join through `/rpc` become forge hosts via its registrar;
    // dev mode additionally keeps the in-process mock `local` host.
    let forge_m = Arc::new(
        forge::Forge::with_queue(registry, queue.clone())
            .local_host(dev)
            .dev_admin(dev),
    );

    let modules: Vec<Arc<dyn Module>> = vec![
        gate_m,
        forge_m.clone(),
        Arc::new(loom::Loom::configured(loom::LoomConfig::production(
            dev,
            queue.clone(),
        ))),
        Arc::new(fabric_module()),
        Arc::new(
            slab::Slab::default()
                .with_silo_limits(silo_cfg.max_object_bytes, silo::MIN_PART_BYTES)
                .with_queue(queue.clone())
                .with_images(images.clone()),
        ),
        Arc::new(mold::Mold::new(images.clone())),
        Arc::new(silo::Silo::configured(silo_cfg)),
        Arc::new(shelf::Shelf),
        Arc::new(beacon::Beacon::with_cluster_hint(listen)),
        Arc::new(relay_module().with_queue(queue.clone())),
        Arc::new(keep::Keep::default()),
        Arc::new(blueprint::Blueprint),
        Arc::new(gauge::Gauge),
        Arc::new(bench::Bench),
    ];
    Assembly {
        modules,
        forge: forge_m,
        queue,
        images,
    }
}

/// Southbound database every agent should point its `ovn-controller` at,
/// e.g. `tcp:10.0.0.11:6642`. Read only when the fabric backend is `ovn`.
const OVN_SB_ENV: &str = "PISTACK_OVN_SB";
/// Tunnel type handed to agents with [`OVN_SB_ENV`]: `geneve` (the default)
/// or `vxlan`.
const OVN_ENCAP_TYPE_ENV: &str = "PISTACK_OVN_ENCAP_TYPE";

/// What agents are told about OVN in the hello answer. `None` unless the
/// backend is `ovn` and a southbound address is set: without one, hosts
/// keep whatever the operator configured and the agent only reports on it.
fn ovn_host_config(
    backend: Option<&str>,
    sb: Option<&str>,
    encap: Option<&str>,
) -> Option<pistack_rpc::OvnHostConfig> {
    if backend != Some("ovn") {
        return None;
    }
    let Some(sb) = sb.map(str::trim).filter(|v| !v.is_empty()) else {
        tracing::warn!(
            "fabric: {OVN_SB_ENV} is not set, so agents are not told where the OVN southbound database is and each host must be set up by hand"
        );
        return None;
    };
    let encap = encap.map(str::trim).filter(|v| !v.is_empty());
    Some(pistack_rpc::OvnHostConfig {
        sb_remote: sb.to_owned(),
        encap_type: encap.unwrap_or("geneve").to_owned(),
    })
}

/// `PISTACK_FABRIC_BACKEND=hostnat` is the single-host NAT datapath.
/// `PISTACK_FABRIC_BACKEND=overlay` is the VXLAN segment datapath. The agent
/// applies that plan. Anything else keeps the mock backend so tests and a
/// laptop server never touch interfaces. A server started without the
/// variable uses the cluster's saved choice (see `fabricchoice`), which
/// `run` puts back into the variable before modules are assembled.
/// `PISTACK_RELAY_BACKEND=forward` mounts the in-process forwarder and HTTP
/// proxy, and `mock` keeps the mock. Unset follows the fabric: `hostnat`,
/// `overlay` or `ovn` gets the forwarder, anything else the mock. `run` refuses any
/// other value before it gets here.
fn relay_module() -> relay::Relay {
    let backend = relay::RelayBackend::from_env().unwrap_or_else(|e| {
        tracing::error!(error = %e, "relay: falling back to the mock backend");
        relay::RelayBackend::Mock
    });
    tracing::info!(backend = backend.name(), "relay: backend");
    relay::Relay::for_backend(backend)
}

fn fabric_module() -> fabric::Fabric {
    match std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref() {
        Some("hostnat") => fabric::Fabric::hostnat(),
        Some("overlay") => fabric::Fabric::overlay(),
        Some("ovn") => fabric::Fabric::ovn(),
        _ => fabric::Fabric::default(),
    }
}

/// Where host agents are told to reach this server: `--agent-url`, else the
/// TLS listener's own address. `None` without a TLS listener. Agents show a
/// client certificate, so the address is https or it is refused.
fn agent_api(tls_listen: Option<&str>, agent_url: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(listen) = tls_listen.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    let Some(url) = agent_url.map(str::trim).filter(|u| !u.is_empty()) else {
        return Ok(Some(format!("https://{listen}")));
    };
    let parsed =
        reqwest::Url::parse(url).with_context(|| format!("--agent-url '{url}' is not a URL"))?;
    if parsed.scheme() != "https" {
        anyhow::bail!("--agent-url '{url}' must be https: agents present a client certificate");
    }
    Ok(Some(url.trim_end_matches('/').to_owned()))
}

/// The host of a URL, without the brackets of an IPv6 address.
fn host_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(host.trim_matches(|c| c == '[' || c == ']').to_owned())
}

/// After a join over https: the store that came down holds the root CA
/// whose fingerprint the operator gave. Nothing to check without one.
fn joined_the_pinned_cloud(store: &Store, pin: Option<&str>) -> anyhow::Result<()> {
    let Some(pin) = pin.map(str::trim).filter(|pin| !pin.is_empty()) else {
        return Ok(());
    };
    let pin = keep::tls::parse_fingerprint(pin)
        .map_err(|err| anyhow::anyhow!("{}: {err}", crate::cluster::JOIN_CA_ENV))?;
    let root = keep::ca::root_fingerprint(store)?.context("the joined store has no cloud CA")?;
    if keep::tls::parse_fingerprint(&root).ok() != Some(pin) {
        anyhow::bail!(
            "the joined store's CA is not the one {} names",
            crate::cluster::JOIN_CA_ENV
        );
    }
    Ok(())
}

/// How a server came to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// It was told to.
    Shutdown,
    /// It replaced its own store and has to start again on the new file.
    Restart,
}

pub async fn run(opts: ServerOpts) -> anyhow::Result<Stopped> {
    std::fs::create_dir_all(&opts.data_dir)
        .with_context(|| format!("creating data dir {}", opts.data_dir.display()))?;
    // Modules that keep files default under PISTACK_DATA_DIR.
    if std::env::var_os("PISTACK_DATA_DIR").is_none() {
        std::env::set_var("PISTACK_DATA_DIR", &opts.data_dir);
    }
    let store_path = opts.data_dir.join("state.redb");
    let public_url = opts.public_url.clone().unwrap_or_else(|| {
        let scheme = if opts.insecure_http { "http" } else { "https" };
        format!("{scheme}://{}", opts.listen.replace("0.0.0.0", "127.0.0.1"))
    });
    let agent_api = agent_api(opts.tls_listen.as_deref(), opts.agent_url.as_deref())?;
    let tls_mode =
        keep::peer::tls_mode_value(std::env::var(keep::peer::CLUSTER_TLS_ENV).ok().as_deref())
            .map_err(anyhow::Error::msg)?;
    if tls_mode == keep::peer::TlsMode::Required && agent_api.is_none() {
        anyhow::bail!(
            "{}=required needs --tls-listen: the other servers could not call this one",
            keep::peer::CLUSTER_TLS_ENV
        );
    }
    let boot = crate::cluster::prepare_store(&crate::cluster::BootRequest {
        data_dir: &opts.data_dir,
        init: opts.init,
        join: opts.join.as_deref(),
        listen: &opts.listen,
        public_url: &public_url,
        ca_pin: opts.join_ca.as_deref(),
        insecure_join: opts.insecure_join,
    })
    .await?;
    // Keep never sweeps. Joined has already downloaded a snapshot. Init has
    // no file yet; opening the store creates it.
    let (events, _) = broadcast::channel(4096);
    let store = match Store::open(&store_path, events.clone()) {
        Ok(store) => store,
        Err(err) => {
            if matches!(boot, crate::cluster::BootKind::Joined) {
                let _ = std::fs::remove_file(&store_path);
            }
            return Err(err).context("opening state store");
        }
    };
    crate::cluster::set_peer_store(&store);
    crate::cluster::warn_if_open();
    let peers = keep::peer::PeerNet::new(tls_mode, store.clone());
    if matches!(boot, crate::cluster::BootKind::Joined) {
        // The download was checked against this fingerprint on the way; the
        // store it brought must be of the same cloud.
        if let Err(err) = joined_the_pinned_cloud(&store, opts.join_ca.as_deref()) {
            drop(peers);
            drop(store);
            let _ = std::fs::remove_file(&store_path);
            return Err(err);
        }
    }

    // One stable node name for this machine, shared with an agent that uses
    // the same data dir. Kept in <data_dir>/node.name across restarts.
    let node =
        pistack_types::node::resolve_node_name(&opts.data_dir).context("resolving node name")?;
    if let Some(old) = &node.replaced {
        tracing::warn!(node = %node.name, previous = %old, "PISTACK_NODE_NAME renamed this node");
    }

    let shutdown = CancellationToken::new();
    // Leadership starts false until establish_local / adopt decides; modules
    // that subscribe will stay idle until set below.
    let leadership = Leadership::new(false);
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: events.clone(),
        shutdown: shutdown.clone(),
        public_url: public_url.clone(),
        node_name: node.name.clone(),
        leadership: leadership.clone(),
    };

    if store.get_meta::<String>("cluster_id")?.is_none() {
        if matches!(boot, crate::cluster::BootKind::Joined) {
            drop(store);
            let _ = std::fs::remove_file(&store_path);
            anyhow::bail!("joined store has no cluster id");
        }
        tracing::info!(dir = %opts.data_dir.display(), "initialising new cloud");
        store.put_meta("cluster_id", &uuid::Uuid::now_v7().to_string())?;
        store.put_meta("created_at", &chrono::Utc::now())?;
    }
    let node_id = crate::cluster::load_or_create_node_id(&opts.data_dir)?;
    let allow_seed = !matches!(boot, crate::cluster::BootKind::Joined);
    let established = match crate::cluster::establish_local(
        &store,
        &node_id,
        &public_url,
        agent_api.as_deref(),
        allow_seed,
    ) {
        Ok(crate::cluster::LocalRole::Ready(ready)) => ready,
        Ok(crate::cluster::LocalRole::NeedJoin) => {
            let hint = crate::cluster::leader_hint(&store, &node_id)?
                .or_else(|| crate::cluster::remembered_leader(&opts.data_dir))
                .context(
                    "store belongs to a cluster and no leader address is known; pass --join URL",
                )?;
            let adopted = crate::cluster::adopt(
                &store,
                &peers,
                &hint,
                &node_id,
                &public_url,
                agent_api.as_deref(),
            );
            match adopted.await {
                Ok(ready) => ready,
                Err(err) => {
                    if matches!(boot, crate::cluster::BootKind::Joined) {
                        drop(store);
                        let _ = std::fs::remove_file(&store_path);
                    }
                    return Err(err);
                }
            }
        }
        Err(err) => return Err(err),
    };
    crate::cluster::clear_remembered_leader(&opts.data_dir);
    let cluster_id: String = store
        .get_meta("cluster_id")?
        .unwrap_or_else(|| "unknown".into());
    tracing::info!(
        %cluster_id,
        %node_id,
        node_name = %node.name,
        node_name_source = %node.source,
        leader = established.leader,
        role = ?established.role,
        members = established.members.len(),
        "control plane membership"
    );
    // Before any module reads the fabric setting. Only a leader saves it
    // (its writes are logged and replicate); a follower never writes its
    // replica, which would split it off from the leader.
    let fabric_choice =
        crate::fabricchoice::settle(&store, established.leader).context("fabric backend")?;
    leadership.set(established.leader);
    // A follower promoted later saves its setting then.
    crate::fabricchoice::save_on_promotion(
        store.clone(),
        leadership.clone(),
        shutdown.clone(),
        &fabric_choice,
    );
    let cluster = std::sync::Arc::new(
        crate::cluster::handle_from(
            node_id.clone(),
            public_url.clone(),
            cluster_id,
            store.clone(),
            store_path.clone(),
            &established,
            crate::cluster::advertise_ip(&opts.listen),
            leadership.clone(),
        )
        .with_agent_api(agent_api.clone())
        .with_peers(peers)
        .with_auto_resync(crate::cluster::auto_resync_value(
            std::env::var(crate::cluster::AUTO_RESYNC_ENV)
                .ok()
                .as_deref(),
        )),
    );
    crate::cluster::spawn_background(
        cluster.clone(),
        crate::cluster::advertise_ip(&opts.listen),
        shutdown.clone(),
    );
    if opts.dev {
        tracing::warn!(
            "DEV MODE: unauthenticated requests act as admin; a mock local host runs machines"
        );
    }

    relay::RelayBackend::from_env().map_err(anyhow::Error::msg)?;
    // PISTACK_KEEP_BACKEND: refuse a bad or parked root-key backend before
    // any module opens keep.
    let key_backend = keep::backend::configured().context("keep root-key backend")?;
    tracing::info!(backend = key_backend.name(), "keep: root-key backend");
    let silo_cfg = silo::SiloConfig::load().context("silo config")?;
    tracing::info!(
        max_object_bytes = silo_cfg.max_object_bytes,
        region = %silo_cfg.region,
        "silo: max object size"
    );
    let mold_cfg = mold::MoldConfig::load().context("mold config")?;
    let images =
        mold_cfg.image_store(silo::Silo::configured(silo_cfg.clone()).client(store.clone()));
    tracing::info!(store = images.name(), "mold: image store");
    let asm = assemble(opts.dev, silo_cfg, images, &opts.listen, &opts.data_dir);

    // Start modules first so gate has generated its signing key before the
    // gateway asks for the verifier.
    for m in &asm.modules {
        let name = m.info().name;
        m.start(ctx.clone())
            .await
            .with_context(|| format!("starting module {name}"))?;
        tracing::debug!(module = name, "started");
    }

    let verifier = gate::Verifier::from_store(&store).context("loading token verifier")?;
    let app = gateway::build(&asm, ctx.clone(), verifier, opts.dev, Some(cluster.clone()));

    let tls = opts
        .tls_listen
        .clone()
        .map(|addr| crate::listen::TlsListen {
            addr,
            data_dir: opts.data_dir.clone(),
            store: store.clone(),
            names: agent_api.as_deref().and_then(host_of).into_iter().collect(),
            leadership: leadership.clone(),
            node_id: node_id.clone(),
        });
    let bound = crate::listen::bind_listeners(app, &opts.listen, tls).await?;
    tracing::info!(
        listen = %bound.http,
        public_url = %public_url,
        agent_url = agent_api.as_deref().unwrap_or("none"),
        modules = asm.modules.len(),
        build = pistack_types::BUILD,
        store_schema = store.schema_version()?,
        agent_rpc = asm.forge.queue().is_some(),
        "pistack server up"
    );
    if bound.tls.is_none() && !opts.insecure_http {
        tracing::warn!("TLS not wired yet (keep is next); serving plain HTTP. Pass --insecure-http to silence this.");
    }
    let sd = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
        sd.cancel();
    });
    let served = bound.serve(shutdown.clone());
    tokio::pin!(served);
    tokio::select! {
        done = &mut served => done?,
        // Open requests get a moment to finish and no more: the store
        // they read is no longer the one in the data directory.
        _ = restart_grace(&cluster, &shutdown) => {}
    }
    if cluster.wants_restart() {
        return Ok(Stopped::Restart);
    }
    Ok(Stopped::Shutdown)
}

/// Ends five seconds after the cluster loop stopped the server to start it
/// again. Never ends on an ordinary shutdown.
async fn restart_grace(cluster: &crate::cluster::ClusterHandle, shutdown: &CancellationToken) {
    shutdown.cancelled().await;
    if !cluster.wants_restart() {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
}

pub fn backup(data_dir: PathBuf, to: PathBuf) -> anyhow::Result<()> {
    let src = data_dir.join("state.redb");
    let info = pistack_store::backup_file(&src, &to)
        .with_context(|| format!("backing up {} to {}", src.display(), to.display()))?;
    // The path alone goes to stdout, for scripts.
    eprintln!("store schema version {}", info.schema_version);
    println!("{}", to.display());
    Ok(())
}

pub fn stats(data_dir: PathBuf) -> anyhow::Result<()> {
    let path = data_dir.join("state.redb");
    let file_bytes = std::fs::metadata(&path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    let (events, _) = broadcast::channel(1);
    let store = Store::open(&path, events)?;
    println!("file\t{file_bytes} bytes\t{}", path.display());
    println!(
        "schema\tversion {}\tthis pistack reads up to {}",
        store.schema_version()?,
        pistack_store::SCHEMA_VERSION
    );
    for table in store.stats()? {
        println!(
            "{}\t{} rows\t{} key bytes\t{} value bytes",
            table.table, table.rows, table.key_bytes, table.value_bytes
        );
    }
    println!(
        "log\tentries {}..={}",
        store.log_start()?,
        store.commit_index()?
    );
    Ok(())
}

pub fn dump(data_dir: PathBuf) -> anyhow::Result<()> {
    let (events, _) = broadcast::channel(1);
    let store = Store::open(data_dir.join("state.redb"), events)?;
    for (k, v) in store.raw_scan("resources", "")? {
        println!("{k}\t{}", String::from_utf8_lossy(&v));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{agent_api, host_of, joined_the_pinned_cloud, ovn_host_config};

    #[test]
    fn the_certificate_host_is_read_from_the_agent_url() {
        assert_eq!(
            host_of("https://cloud.example:8444").as_deref(),
            Some("cloud.example")
        );
        assert_eq!(
            host_of("https://10.0.0.11:8444").as_deref(),
            Some("10.0.0.11")
        );
        assert_eq!(
            host_of("https://[fd00::11]:8444").as_deref(),
            Some("fd00::11")
        );
        assert_eq!(host_of("cloud.example"), None);
    }

    #[test]
    fn a_joined_store_must_hold_the_ca_that_was_pinned() {
        let store = pistack_store::Store::in_memory().unwrap();
        // No fingerprint given: nothing to compare.
        joined_the_pinned_cloud(&store, None).unwrap();
        joined_the_pinned_cloud(&store, Some(" ")).unwrap();
        let zeros = "0".repeat(64);
        assert!(joined_the_pinned_cloud(&store, Some(&zeros)).is_err());
        keep::Client::open(&store).unwrap();
        let print = keep::ca::root_fingerprint(&store).unwrap().unwrap();
        joined_the_pinned_cloud(&store, Some(&print)).unwrap();
        joined_the_pinned_cloud(&store, Some(&print.to_uppercase())).unwrap();
        assert!(joined_the_pinned_cloud(&store, Some(&zeros)).is_err());
        assert!(joined_the_pinned_cloud(&store, Some("abcd")).is_err());
    }

    #[test]
    fn agents_are_sent_to_the_tls_listener_or_the_url_given_for_it() {
        assert_eq!(agent_api(None, None).unwrap(), None);
        assert_eq!(
            agent_api(Some("10.0.0.10:8444"), None).unwrap().as_deref(),
            Some("https://10.0.0.10:8444")
        );
        assert_eq!(
            agent_api(Some("10.0.0.10:8444"), Some("https://cloud.example:8444/"))
                .unwrap()
                .as_deref(),
            Some("https://cloud.example:8444")
        );
        assert!(agent_api(Some("10.0.0.10:8444"), Some("http://cloud.example:8443")).is_err());
        assert!(agent_api(Some("10.0.0.10:8444"), Some("cloud.example")).is_err());
        // The URL names the TLS listener; without one there is nothing to name.
        assert_eq!(
            agent_api(None, Some("https://cloud.example:8444")).unwrap(),
            None
        );
    }

    #[test]
    fn agents_are_told_about_ovn_only_when_the_backend_is_ovn_and_has_an_address() {
        let got = ovn_host_config(Some("ovn"), Some(" tcp:10.0.0.11:6642 "), None).unwrap();
        assert_eq!(got.sb_remote, "tcp:10.0.0.11:6642");
        assert_eq!(got.encap_type, "geneve");
        let got = ovn_host_config(Some("ovn"), Some("tcp:10.0.0.11:6642"), Some("vxlan")).unwrap();
        assert_eq!(got.encap_type, "vxlan");
        assert_eq!(ovn_host_config(Some("ovn"), None, None), None);
        assert_eq!(
            ovn_host_config(Some("ovn"), Some("  "), Some("geneve")),
            None
        );
        assert_eq!(
            ovn_host_config(Some("overlay"), Some("tcp:10.0.0.11:6642"), None),
            None
        );
        assert_eq!(
            ovn_host_config(None, Some("tcp:10.0.0.11:6642"), None),
            None
        );
    }
}
