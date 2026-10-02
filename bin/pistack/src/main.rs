// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pistack` — the one binary.
//!
//! ```text
//! pistack server --init          first control-plane node: create store, CA, admin
//! pistack server --join URL      additional control-plane node
//! pistack agent  --join URL      a host (anvil)
//! pistack store dump             inspect state
//! ```

mod gateway;
mod server;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pistack", version, about = "PiStack private cloud")]
struct Cli {
    /// Data directory for the state store and keys.
    #[arg(long, env = "PISTACK_DATA_DIR", default_value = "/var/lib/pistack")]
    data_dir: PathBuf,
    /// Log filter, e.g. `info,gate=debug`.
    #[arg(long, env = "PISTACK_LOG", default_value = "info")]
    log: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a control-plane node (API gateway + all modules).
    Server {
        /// Initialise a brand-new cloud in `data_dir`.
        #[arg(long)]
        init: bool,
        /// Join an existing control plane.
        #[arg(long, conflicts_with = "init")]
        join: Option<String>,
        /// Listen address for the API.
        #[arg(long, env = "PISTACK_LISTEN", default_value = "0.0.0.0:8443")]
        listen: String,
        /// Public URL clients use (defaults to https://<listen>).
        #[arg(long, env = "PISTACK_PUBLIC_URL")]
        public_url: Option<String>,
        /// Serve plain HTTP (dev only).
        #[arg(long)]
        insecure_http: bool,
    },
    /// Run a host agent (anvil).
    Agent {
        #[arg(long)]
        join: String,
        #[arg(long, env = "PISTACK_JOIN_TOKEN")]
        token: Option<String>,
    },
    /// Inspect the local state store.
    Store {
        #[command(subcommand)]
        cmd: StoreCmd,
    },
}

#[derive(Subcommand)]
enum StoreCmd {
    /// Dump every resource as JSON lines.
    Dump,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&cli.log))
        .with_target(true)
        .init();

    match cli.cmd {
        Cmd::Server { init, join, listen, public_url, insecure_http } => {
            server::run(server::ServerOpts {
                data_dir: cli.data_dir,
                init,
                join,
                listen,
                public_url,
                insecure_http,
            })
            .await
        }
        Cmd::Agent { join, token } => anvil::run_agent(anvil::AgentOpts { join, token, data_dir: cli.data_dir }).await,
        Cmd::Store { cmd: StoreCmd::Dump } => server::dump(cli.data_dir),
    }
}
