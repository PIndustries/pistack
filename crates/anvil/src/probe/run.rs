// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::book::outcome::Outcome;
use crate::probe::exec::command_probe_reply::CommandProbeReply;
use crate::probe::exec::validate_exec_argv::validate_exec_argv;
use crate::probe::net::http_probe::http_probe;
use crate::probe::net::tcp_probe::tcp_probe;
use crate::probe::outcome_of_reply;
use crate::probe::spec::probe_action::ProbeAction;
use std::future::Future;
use std::time::Duration;

/// Run one check. `exec` calls `exec_probe` with the probe argv and timeout.
/// A probe with no handler is unsupported and does not count as a failure.
pub async fn run<F, Fut>(check: &DueCheck, exec_probe: F) -> Outcome
where
    F: FnOnce(&[String], Duration) -> Fut,
    Fut: Future<Output = CommandProbeReply>,
{
    match &check.action {
        ProbeAction::Exec { command } => {
            if let Err(message) = validate_exec_argv(command) {
                return Outcome::fail(message);
            }
            outcome_of_reply(exec_probe(command, check.timeout).await)
        }
        ProbeAction::Missing => Outcome::unsupported("probe has no handler"),
        ProbeAction::Tcp { port } => tcp_probe(check, *port).await,
        ProbeAction::Http {
            path,
            port,
            host,
            https,
        } => http_probe(check, path, *port, host.as_deref(), *https).await,
    }
}
