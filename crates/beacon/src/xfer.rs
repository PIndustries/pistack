// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Secondary zone transfer: pull a full AXFR from a primary over TCP and
//! compile it into a [`crate::server::ZoneData`] for authoritative answers.

use crate::server::ZoneData;
use futures::StreamExt;
use hickory_proto::rr::{Name, RecordType};
use hickory_server::net::client::{Client as DnsClient, ClientHandle};
use hickory_server::net::runtime::TokioRuntimeProvider;
use hickory_server::net::tcp::TcpClientStream;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

/// How long to wait for the TCP connect and the transfer itself.
const XFER_TIMEOUT: Duration = Duration::from_secs(10);

/// Parse a primary address from zone spec form (`ip`, `ip:port`, `[ipv6]:port`).
pub(crate) fn parse_primary(raw: &str) -> anyhow::Result<SocketAddr> {
    let s = raw.trim();
    if s.is_empty() {
        anyhow::bail!("empty primary address");
    }
    if let Ok(addr) = SocketAddr::from_str(s) {
        return Ok(addr);
    }
    // Bare IP → port 53.
    if let Ok(ip) = std::net::IpAddr::from_str(s) {
        return Ok(SocketAddr::new(ip, 53));
    }
    anyhow::bail!("primary {raw} is not an address")
}

/// Pull a full AXFR for `apex` from `primary` and return compiled zone data
/// plus the SOA serial.
pub(crate) async fn pull_axfr(primary: SocketAddr, apex: &str) -> anyhow::Result<(u32, ZoneData)> {
    let apex_name = Name::from_ascii(apex)?;
    let (stream, sender) = TcpClientStream::new(
        primary,
        None,
        Some(XFER_TIMEOUT),
        TokioRuntimeProvider::new(),
    );
    let stream = tokio::time::timeout(XFER_TIMEOUT, stream)
        .await
        .map_err(|_| anyhow::anyhow!("connect to {primary} timed out"))?
        .map_err(|e| anyhow::anyhow!("connect to {primary}: {e}"))?;
    let (mut client, bg) = DnsClient::<TokioRuntimeProvider>::new(stream, sender);
    tokio::spawn(bg);

    let mut xfr = client.zone_transfer(apex_name.clone(), None);
    let mut records = Vec::new();
    let deadline = tokio::time::Instant::now() + XFER_TIMEOUT;
    while let Some(next) = xfr.next().await {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("AXFR from {primary} for {apex} timed out");
        }
        let resp = next.map_err(|e| anyhow::anyhow!("AXFR from {primary}: {e}"))?;
        if resp.metadata.response_code != hickory_proto::op::ResponseCode::NoError {
            anyhow::bail!(
                "AXFR from {primary} for {apex}: {}",
                resp.metadata.response_code
            );
        }
        records.extend(resp.answers.iter().cloned());
    }
    if records.len() < 2 {
        anyhow::bail!("AXFR from {primary} for {apex} returned no SOA bracket");
    }
    if records.first().map(|r| r.record_type()) != Some(RecordType::SOA)
        || records.last().map(|r| r.record_type()) != Some(RecordType::SOA)
    {
        anyhow::bail!("AXFR from {primary} for {apex} is not SOA-bracketed");
    }
    let serial = match &records[0].data {
        hickory_proto::rr::RData::SOA(soa) => soa.serial,
        _ => unreachable!(),
    };
    let data = ZoneData::from_axfr(apex_name, records)?;
    Ok((serial, data))
}

/// Try each primary in order until one AXFR succeeds.
pub(crate) async fn pull_from_primaries(
    primaries: &[String],
    apex: &str,
) -> anyhow::Result<(u32, ZoneData, String)> {
    let mut last_err = anyhow::anyhow!("no primaries configured");
    for raw in primaries {
        let addr = match parse_primary(raw) {
            Ok(a) => a,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        match pull_axfr(addr, apex).await {
            Ok((serial, data)) => return Ok((serial, data, raw.clone())),
            Err(e) => {
                tracing::warn!(primary = %raw, apex, error = %e, "beacon: secondary AXFR failed");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_addresses() {
        assert_eq!(
            parse_primary("10.0.0.1").unwrap(),
            "10.0.0.1:53".parse().unwrap()
        );
        assert_eq!(
            parse_primary("10.0.0.1:5353").unwrap(),
            "10.0.0.1:5353".parse().unwrap()
        );
        assert_eq!(
            parse_primary("[::1]:53").unwrap(),
            "[::1]:53".parse().unwrap()
        );
        assert!(parse_primary("not-an-ip").is_err());
    }
}
