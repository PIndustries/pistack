// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::book::outcome::Outcome;
use crate::probe::net::connect_host::connect_host;
use crate::probe::net::http_path::http_path;
use crate::probe::net::http_status::http_status;
use crate::probe::net::tls_config::tls_config;
use tokio::net::TcpStream;

pub async fn http_probe(
    check: &DueCheck,
    path: &str,
    port: u16,
    host: Option<&str>,
    https: bool,
) -> Outcome {
    if port == 0 {
        return Outcome::fail("probe port is unset");
    }
    let Ok(path) = http_path(path) else {
        return Outcome::fail("probe path is invalid");
    };
    let connect = match connect_host(check, host) {
        Ok(host) => host.to_owned(),
        Err(outcome) => return outcome,
    };
    let header_host = host
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(connect.as_str())
        .to_owned();
    let result = tokio::time::timeout(check.timeout, async {
        let stream = TcpStream::connect((connect.as_str(), port))
            .await
            .map_err(|e| format!("connect: {e}"))?;
        if https {
            let tls = tls_config().map_err(|e| format!("tls: {e}"))?;
            let name = rustls::pki_types::ServerName::try_from(connect.clone())
                .map_err(|_| "probe host is invalid".to_owned())?;
            let stream = tokio_rustls::TlsConnector::from(tls)
                .connect(name, stream)
                .await
                .map_err(|e| format!("tls: {e}"))?;
            http_status(stream, &path, &header_host).await
        } else {
            http_status(stream, &path, &header_host).await
        }
    })
    .await;
    match result {
        Ok(Ok(code)) if (200..400).contains(&code) => Outcome::ok(),
        Ok(Ok(code)) => Outcome::fail(format!("http status {code}")),
        Ok(Err(message)) => Outcome::fail(message),
        Err(_) => Outcome::fail("timed out"),
    }
}
