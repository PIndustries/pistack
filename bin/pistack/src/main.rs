// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pistack` — the one binary.
//!
//! ```text
//! pistack server                 private address, no store: discover, adopt, or found
//! pistack server --init          found a new cluster even if peers answer
//! pistack server --join URL      adopt that control plane and skip discovery
//!                                (https with PISTACK_JOIN_CA_SHA256, or
//!                                plain http with --insecure-join)
//! pistack server                 existing store: stay in that cluster and announce
//! pistack agent  --join URL      a host (anvil)
//! pistack store dump             inspect state
//! pistack store backup --to FILE copy the store and check the copy
//! pistack store restore --from FILE   put a backup in an empty data directory
//! pistack keep status            where the root key is kept; can the configured backend open it
//! pistack keep migrate-to-vault  move the software root key into Vault Transit
//! pistack keep migrate-to-software --allow-plaintext-key   the reverse
//! pistack keep rewrap            re-encrypt the root key under the latest Transit key version
//! ```
//!
//! The `keep` commands run on a stopped node's store; see `keepcmd.rs`.

mod cluster;
mod fabricchoice;
mod gateway;
mod keepcmd;
mod listen;
mod metrics;
mod restore;
mod server;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pistack", version = pistack_types::BUILD, about = "PiStack private cloud")]
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
        /// SHA-256 of the cloud's root CA certificate, as `pi join-token
        /// create` prints it. Needed for the first join over https.
        #[arg(long = "join-ca-sha256", env = "PISTACK_JOIN_CA_SHA256")]
        join_ca: Option<String>,
        /// Let the first join go over plain http: the join token and the
        /// whole store cross the network in the clear.
        #[arg(long, env = "PISTACK_INSECURE_JOIN")]
        insecure_join: bool,
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
        /// URL host agents use to reach `--tls-listen` (defaults to
        /// https://<tls-listen>). Must be https.
        #[arg(long, env = "PISTACK_AGENT_URL", requires = "tls_listen")]
        agent_url: Option<String>,
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
    /// Root-key operations on the local store. Stop the server first.
    Keep {
        #[command(subcommand)]
        cmd: KeepCmd,
    },
}

#[derive(Subcommand)]
enum KeepCmd {
    /// Show where the root key is kept and whether the configured backend opens it.
    Status,
    /// Wrap the software root key with Vault Transit, verify, then drop the
    /// plaintext copy in the same transaction. Needs PISTACK_KEEP_BACKEND=vault
    /// and the PISTACK_KEEP_VAULT_* settings.
    MigrateToVault,
    /// Unwrap the root key from Vault and store it in the clear, dropping the
    /// wrapped copy in the same transaction.
    MigrateToSoftware {
        /// Confirm that the key will be readable by anyone with the store file.
        #[arg(long)]
        allow_plaintext_key: bool,
    },
    /// After rotating the Transit key, re-encrypt the stored root key under
    /// its latest version. Do this before raising min_decryption_version.
    Rewrap,
    /// Seal every secret and private key again under a new root key
    /// (software backend). Safe to interrupt: run it again to finish.
    RotateRootKey {
        /// File holding the new key. If it does not exist a new key is
        /// written to it. A cluster needs it: every server must be given
        /// the same key. Delete the file when all servers are done.
        #[arg(long, value_name = "PATH")]
        key_file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum StoreCmd {
    /// Dump every resource as JSON lines.
    Dump,
    /// Copy `data_dir/state.redb` to `--to` and check the copy.
    Backup {
        /// Destination file. It must not already exist.
        #[arg(long)]
        to: PathBuf,
    },
    /// Check a backup and put it in the data directory as `state.redb`. The
    /// server that then starts there is the only member of the cluster.
    /// Stop the server first.
    Restore {
        /// A file written by `pistack store backup`. It is not changed.
        #[arg(long)]
        from: PathBuf,
        /// Restore into a data directory that is not empty. The store that
        /// is there is renamed, not deleted.
        #[arg(long)]
        force: bool,
    },
    /// Rows and bytes per table, and how much of the store is replication log.
    /// Stop the server first: the store file opens once.
    Stats,
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
            join_ca,
            insecure_join,
            listen,
            public_url,
            insecure_http,
            tls_listen,
            agent_url,
            dev,
        } => {
            let stopped = server::run(server::ServerOpts {
                data_dir: cli.data_dir,
                init,
                join,
                join_ca,
                insecure_join,
                listen,
                public_url,
                insecure_http,
                tls_listen,
                agent_url,
                dev,
            })
            .await?;
            match stopped {
                server::Stopped::Shutdown => Ok(()),
                server::Stopped::Restart => Err(start_again()),
            }
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
            StoreCmd::Restore { from, force } => restore::run(&cli.data_dir, &from, force),
            StoreCmd::Stats => server::stats(cli.data_dir),
        },
        Cmd::Keep { cmd } => match cmd {
            KeepCmd::Status => keepcmd::status(&cli.data_dir),
            KeepCmd::MigrateToVault => keepcmd::migrate_to_vault(&cli.data_dir),
            KeepCmd::MigrateToSoftware {
                allow_plaintext_key,
            } => keepcmd::migrate_to_software(&cli.data_dir, allow_plaintext_key),
            KeepCmd::Rewrap => keepcmd::rewrap(&cli.data_dir),
            KeepCmd::RotateRootKey { key_file } => {
                keepcmd::rotate_root_key(&cli.data_dir, key_file.as_deref())
            }
        },
    }
}

/// Replace this process with a new one given the same arguments. It returns
/// only when that failed, and the error ends the process with a failure
/// status so a service manager starts it instead.
fn start_again() -> anyhow::Error {
    use std::os::unix::process::CommandExt;
    tracing::info!("starting again on the new store");
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            return anyhow::anyhow!(
                "the store was replaced and the server must be started again; it could not find its own binary: {err}"
            )
        }
    };
    let err = std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .exec();
    anyhow::anyhow!(
        "the store was replaced and the server must be started again; running {} failed: {err}",
        exe.display()
    )
}
