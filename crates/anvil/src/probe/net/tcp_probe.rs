// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::book::outcome::Outcome;
use crate::probe::net::connect_host::connect_host;
use tokio::net::TcpStream;

pub async fn tcp_probe(check: &DueCheck, port: u16) -> Outcome {
    if port == 0 {
        return Outcome::fail("probe port is unset");
    }
    let host = match connect_host(check, None) {
        Ok(host) => host.to_owned(),
        Err(outcome) => return outcome,
    };
    match tokio::time::timeout(check.timeout, TcpStream::connect((host.as_str(), port))).await {
        Ok(Ok(_)) => Outcome::ok(),
        Ok(Err(e)) => Outcome::fail(format!("connect: {e}")),
        Err(_) => Outcome::fail("timed out"),
    }
}
