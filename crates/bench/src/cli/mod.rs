// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `pi` CLI as a library: `run(args, io)` parses, talks to the API and
//! writes to the handles in `io`, so tests drive it end to end without a
//! process.

pub mod args;
pub mod client;
pub mod commands;
pub mod config;
pub mod kinds;
pub mod output;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use std::io::{BufRead, Write};
use std::path::PathBuf;

pub use client::{CliError, CliResult, Client};
pub use config::{Config, Profile};
pub use output::{Format, Output};

/// Where the CLI reads and writes.
pub struct Io<'a> {
    pub stdin: &'a mut dyn BufRead,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    /// Is a terminal attached? Governs password prompts and confirmations.
    pub tty: bool,
}

impl Io<'_> {
    /// Read one line from stdin (without the newline).
    pub fn read_line(&mut self) -> std::io::Result<String> {
        let mut s = String::new();
        self.stdin.read_line(&mut s)?;
        Ok(s.trim_end_matches(['\r', '\n']).to_owned())
    }
}

/// Everything a command needs: parsed globals, config location, output format.
pub struct Ctx {
    pub config_path: PathBuf,
    pub config: Config,
    pub profile_name: String,
    pub url: Option<String>,
    pub token: Option<String>,
    pub project: Option<String>,
    pub insecure: bool,
    pub fmt: Format,
}

impl Ctx {
    fn from_cli(cli: &args::Cli) -> Result<Ctx> {
        let config_path = cli.config.clone().unwrap_or_else(Config::default_path);
        let config = Config::load(&config_path)?;
        let profile_name = cli
            .profile
            .clone()
            .unwrap_or_else(|| config.current_name().to_owned());
        let profile = config.get(&profile_name);
        Ok(Ctx {
            url: cli
                .url
                .clone()
                .or_else(|| profile.map(|p| p.url.clone()))
                .filter(|u| !u.is_empty()),
            token: cli
                .token
                .clone()
                .or_else(|| profile.and_then(|p| p.token.clone())),
            project: cli
                .project
                .clone()
                .or_else(|| profile.and_then(|p| p.project.clone())),
            insecure: cli.insecure || profile.is_some_and(|p| p.insecure),
            fmt: Format {
                output: cli.output,
                no_headers: cli.no_headers,
            },
            config_path,
            config,
            profile_name,
        })
    }

    /// An authenticated client for the selected profile.
    pub fn client(&self) -> CliResult<Client> {
        let url = self.url.as_deref().ok_or_else(|| {
            CliError::usage(
                "no server configured: run `pi login URL --name USER`, or pass --url / PI_URL",
            )
        })?;
        Client::new(url, self.token.clone(), self.insecure)
    }

    /// A client that may be unauthenticated (login, version, modules).
    pub fn client_with(&self, url: &str) -> CliResult<Client> {
        Client::new(url, self.token.clone(), self.insecure)
    }
}

/// Run `pi` with `args[0]` being the program name. Returns the exit code;
/// errors that reach the caller are internal failures, not user errors.
pub async fn run(args: Vec<String>, io: &mut Io<'_>) -> Result<i32> {
    let cli = match args::Cli::try_parse_from(&args) {
        Ok(c) => c,
        Err(e) => {
            // Help and version go to stdout and exit 0; usage errors to stderr, exit 2.
            let text = e.render().to_string();
            if e.use_stderr() {
                write!(io.stderr, "{text}")?;
            } else {
                write!(io.stdout, "{text}")?;
            }
            return Ok(e.exit_code());
        }
    };

    if let args::Cmd::Completion { shell } = &cli.cmd {
        // Render into memory first: clap_complete panics on a closed pipe.
        let mut cmd = args::Cli::command();
        let mut buf = Vec::new();
        clap_complete::generate(clap_complete::Shell::from(*shell), &mut cmd, "pi", &mut buf);
        io.stdout.write_all(&buf)?;
        return Ok(0);
    }

    let mut ctx = Ctx::from_cli(&cli)?;
    match commands::dispatch(&mut ctx, cli.cmd, io).await {
        Ok(code) => Ok(code),
        Err(e) => {
            writeln!(io.stderr, "error: {e}")?;
            if e.is_unauthorized() {
                let url = ctx.url.as_deref().unwrap_or("URL");
                writeln!(
                    io.stderr,
                    "hint: your token is missing or expired; run `pi login {url} --name USER`"
                )?;
            }
            if let CliError::Api { details, .. } = &e {
                if !details.is_empty() {
                    writeln!(
                        io.stderr,
                        "details: {}",
                        serde_json::to_string(details).unwrap_or_default()
                    )?;
                }
            }
            Ok(1)
        }
    }
}
