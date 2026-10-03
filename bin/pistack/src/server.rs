// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Control-plane bootstrap: open the store, build the module context,
//! mount every module on the gateway, serve.

use crate::gateway;
use anyhow::Context;
use pistack_rpc::{CommandQueue, QueueConfig};
use pistack_store::Store;
use pistack_types::{Module, ModuleContext};
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
pub fn assemble(dev: bool) -> Assembly {
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

    let modules: Vec<Arc<dyn Module>> = vec![
        gate_m.clone(),
        forge_m.clone(),
        Arc::new(loom::Loom),
        Arc::new(fabric::Fabric::default()),
        Arc::new(slab::Slab::default()),
        Arc::new(mold::Mold::default()),
        Arc::new(silo::Silo),
        Arc::new(shelf::Shelf),
        Arc::new(beacon::Beacon::default()),
        Arc::new(relay::Relay::default()),
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

pub async fn run(opts: ServerOpts) -> anyhow::Result<()> {
    std::fs::create_dir_all(&opts.data_dir)
        .with_context(|| format!("creating data dir {}", opts.data_dir.display()))?;
    // Modules that keep files default under PISTACK_DATA_DIR.
    if std::env::var_os("PISTACK_DATA_DIR").is_none() {
        std::env::set_var("PISTACK_DATA_DIR", &opts.data_dir);
    }
    let store_path = opts.data_dir.join("state.redb");
    let fresh = !store_path.exists();
    if fresh && !opts.init && opts.join.is_none() {
        anyhow::bail!(
            "no state in {}: run `pistack server --init` for a new cloud or `--join URL`",
            opts.data_dir.display()
        );
    }
    if opts.join.is_some() {
        anyhow::bail!("--join is not implemented yet (single control-plane node for now)");
    }

    let (events, _) = broadcast::channel(4096);
    let store = Store::open(&store_path, events.clone()).context("opening state store")?;

    let public_url = opts.public_url.clone().unwrap_or_else(|| {
        let scheme = if opts.insecure_http { "http" } else { "https" };
        format!("{scheme}://{}", opts.listen.replace("0.0.0.0", "127.0.0.1"))
    });

    let shutdown = CancellationToken::new();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: events.clone(),
        shutdown: shutdown.clone(),
        public_url: public_url.clone(),
        node_name: hostname(),
    };

    if fresh {
        tracing::info!(dir = %opts.data_dir.display(), "initialising new cloud");
        store.put_meta("cluster_id", &uuid::Uuid::now_v7().to_string())?;
        store.put_meta("created_at", &chrono::Utc::now())?;
    }
    if opts.dev {
        tracing::warn!(
            "DEV MODE: unauthenticated requests act as admin; a mock local host runs machines"
        );
    }

    let asm = assemble(opts.dev);

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
    let app = gateway::build(&asm, ctx.clone(), verifier, opts.dev);

    tracing::info!(
        listen = %opts.listen,
        public_url = %public_url,
        modules = asm.modules.len(),
        identity = asm.gate.info().version,
        agent_rpc = asm.forge.queue().is_some(),
        "pistack server up"
    );
    let listener = tokio::net::TcpListener::bind(&opts.listen).await?;
    if !opts.insecure_http {
        tracing::warn!("TLS not wired yet (keep is next); serving plain HTTP. Pass --insecure-http to silence this.");
    }
    let sd = shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
            sd.cancel();
        })
        .await?;
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

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "localhost".into())
}
