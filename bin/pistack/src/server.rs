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
    pub listen: String,
    pub public_url: Option<String>,
    pub insecure_http: bool,
    /// Optional HTTPS listener. `None` keeps today's single plain listener.
    pub tls_listen: Option<String>,
    /// Dev mode: a local mock host so machines "run" with no agent, and
    /// unauthenticated requests act as admin. Never on in production.
    pub dev: bool,
}

/// Everything the binary assembles before serving.
pub struct Assembly {
    pub modules: Vec<Arc<dyn Module>>,
    pub gate: Arc<gate::Gate>,
    pub forge: Arc<forge::Forge>,
    pub queue: CommandQueue,
}

/// Every module in the binary, in mount order. Adding a module = one line.
/// `listen` is the server `--listen` address; beacon uses it as the cluster
/// hint when `PISTACK_DNS_LISTEN` is unset.
pub fn assemble(dev: bool, silo_cfg: silo::SiloConfig, listen: &str) -> Assembly {
    let gate_m = Arc::new(gate::Gate::default());
    let registry = Arc::new(forge::DriverRegistry::new());
    let queue = CommandQueue::new(QueueConfig::default());
    // Agents that join through `/rpc` become forge hosts via its registrar;
    // dev mode additionally keeps the in-process mock `local` host.
    let forge_m = Arc::new(
        forge::Forge::with_queue(registry, queue.clone())
            .local_host(dev)
            .dev_admin(dev),
    );

    let images: Arc<dyn mold::ImageStore> =
        Arc::new(mold::LocalDirStore::new(mold::default_image_dir()));
    let modules: Vec<Arc<dyn Module>> = vec![
        gate_m.clone(),
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
        Arc::new(mold::Mold::new(images)),
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
        gate: gate_m,
        forge: forge_m,
        queue,
    }
}

/// `PISTACK_FABRIC_BACKEND=hostnat` is the single-host NAT datapath.
/// `PISTACK_FABRIC_BACKEND=overlay` is the VXLAN segment datapath. The agent
/// applies that plan. Anything else keeps the mock backend so tests and a
/// laptop server never touch interfaces. A server started without the
/// variable uses the cluster's saved choice (see `fabricchoice`), which
/// `run` puts back into the variable before modules are assembled.
/// `PISTACK_RELAY_BACKEND=forward` mounts the in-process forwarder and HTTP
/// proxy, and `mock` keeps the mock. Unset follows the fabric: `hostnat` or
/// `overlay` gets the forwarder, anything else the mock. `run` refuses any
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
        _ => fabric::Fabric::default(),
    }
}

pub async fn run(opts: ServerOpts) -> anyhow::Result<()> {
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
    let boot = crate::cluster::prepare_store(&crate::cluster::BootRequest {
        data_dir: &opts.data_dir,
        init: opts.init,
        join: opts.join.as_deref(),
        listen: &opts.listen,
        public_url: &public_url,
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
    let established =
        match crate::cluster::establish_local(&store, &node_id, &public_url, allow_seed) {
            Ok(crate::cluster::LocalRole::Ready(ready)) => ready,
            Ok(crate::cluster::LocalRole::NeedJoin) => {
                let hint = crate::cluster::leader_hint(&store, &node_id)?
                .or_else(|| crate::cluster::remembered_leader(&opts.data_dir))
                .context(
                    "store belongs to a cluster and no leader address is known; pass --join URL",
                )?;
                match crate::cluster::adopt(&store, &hint, &node_id, &public_url).await {
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
    let cluster = std::sync::Arc::new(crate::cluster::handle_from(
        node_id,
        public_url.clone(),
        cluster_id,
        store.clone(),
        store_path.clone(),
        &established,
        crate::cluster::advertise_ip(&opts.listen),
        leadership.clone(),
    ));
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
    let asm = assemble(opts.dev, silo_cfg, &opts.listen);

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
    let app = gateway::build(&asm, ctx.clone(), verifier, opts.dev, Some(cluster));

    let tls = opts
        .tls_listen
        .clone()
        .map(|addr| crate::listen::TlsListen {
            addr,
            data_dir: opts.data_dir.clone(),
            store: store.clone(),
        });
    let bound = crate::listen::bind_listeners(app, &opts.listen, tls).await?;
    tracing::info!(
        listen = %bound.http,
        public_url = %public_url,
        modules = asm.modules.len(),
        identity = asm.gate.info().version,
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
    bound.serve(shutdown).await?;
    Ok(())
}

pub fn backup(data_dir: PathBuf, to: PathBuf) -> anyhow::Result<()> {
    let src = data_dir.join("state.redb");
    pistack_store::backup_file(&src, &to)
        .with_context(|| format!("backing up {} to {}", src.display(), to.display()))?;
    println!("{}", to.display());
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
