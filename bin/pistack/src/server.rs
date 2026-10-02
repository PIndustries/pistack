// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Control-plane bootstrap: open the store, build the module context,
//! mount every module on the gateway, serve.

use crate::gateway;
use anyhow::Context;
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
}

/// Every module in the binary, in mount order. Adding a module = one line.
pub fn all_modules() -> Vec<Arc<dyn Module>> {
    vec![
        Arc::new(gate::Gate::default()),
        Arc::new(forge::Forge::default()),
        Arc::new(loom::Loom::default()),
        Arc::new(fabric::Fabric::default()),
        Arc::new(slab::Slab::default()),
        Arc::new(mold::Mold::default()),
        Arc::new(silo::Silo::default()),
        Arc::new(shelf::Shelf::default()),
        Arc::new(beacon::Beacon::default()),
        Arc::new(relay::Relay::default()),
        Arc::new(keep::Keep::default()),
        Arc::new(blueprint::Blueprint::default()),
        Arc::new(gauge::Gauge::default()),
        Arc::new(bench::Bench::default()),
    ]
}

pub async fn run(opts: ServerOpts) -> anyhow::Result<()> {
    std::fs::create_dir_all(&opts.data_dir)
        .with_context(|| format!("creating data dir {}", opts.data_dir.display()))?;
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

    let modules = all_modules();
    let app = gateway::build(&modules, ctx.clone());

    for m in &modules {
        let name = m.info().name;
        m.start(ctx.clone()).await.with_context(|| format!("starting module {name}"))?;
        tracing::debug!(module = name, "started");
    }

    tracing::info!(listen = %opts.listen, public_url = %public_url, modules = modules.len(), "pistack server up");
    let listener = tokio::net::TcpListener::bind(&opts.listen).await?;
    if !opts.insecure_http {
        tracing::warn!("TLS not wired yet; serving plain HTTP. Pass --insecure-http to silence this.");
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
