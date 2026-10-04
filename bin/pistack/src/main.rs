// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pistack` — the one binary.
//!
//! ```text
//! pistack server                 private address, no store: discover, adopt, or found
//! pistack server --init          found a new cluster even if peers answer
//! pistack server --join URL      adopt that control plane and skip discovery
//! pistack server                 existing store: stay in that cluster and announce
//! pistack agent  --join URL      a host (anvil)
//! pistack store dump             inspect state
//! pistack store backup --to FILE copy the store and open the copy
//! ```

mod cluster;
mod gateway;
mod listen;
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
        /// Found a new cluster even if other clusters answer on the private prefix.
        #[arg(long)]
        init: bool,
        /// Adopt an existing control plane. Skips discovery.
        #[arg(long, conflicts_with = "init")]
        join: Option<String>,
        /// Listen address. A private IPv4 address with no store discovers its /24.
        /// `0.0.0.0` does not discover.
        #[arg(long, env = "PISTACK_LISTEN", default_value = "0.0.0.0:8443")]
        listen: String,
        /// Public URL clients use (defaults to https://<listen>).
        #[arg(long, env = "PISTACK_PUBLIC_URL")]
        public_url: Option<String>,
        /// Serve plain HTTP (dev only). Does not disable `--tls-listen`.
        #[arg(long)]
        insecure_http: bool,
        /// Extra HTTPS listener. Must be a specific address, not 0.0.0.0 or ::.
        /// Plain HTTP on `--listen` stays up. No default.
        #[arg(long, env = "PISTACK_TLS_LISTEN")]
        tls_listen: Option<String>,
        /// Dev mode: mock local host, unauthenticated requests act as admin.
        #[arg(long)]
        dev: bool,
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
    /// Copy `data_dir/state.redb` to `--to` and open the copy.
    Backup {
        /// Destination file. It must not already exist.
        #[arg(long)]
        to: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&cli.log))
        .with_target(true)
        .init();

    match cli.cmd {
        Cmd::Server {
            init,
            join,
            listen,
            public_url,
            insecure_http,
            tls_listen,
            dev,
        } => {
            server::run(server::ServerOpts {
                data_dir: cli.data_dir,
                init,
                join,
                listen,
                public_url,
                insecure_http,
                tls_listen,
                dev,
            })
            .await
        }
        Cmd::Agent { join, token } => {
            anvil::run_agent(anvil::AgentOpts {
                join,
                token,
                data_dir: cli.data_dir,
            })
            .await
        }
        Cmd::Store { cmd } => match cmd {
            StoreCmd::Dump => server::dump(cli.data_dir),
            StoreCmd::Backup { to } => server::backup(cli.data_dir, to),
        },
    }
}
