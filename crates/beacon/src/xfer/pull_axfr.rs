// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::server::ZoneData;
use futures::StreamExt;
use hickory_proto::rr::{Name, RecordType};
use hickory_server::net::client::{Client as DnsClient, ClientHandle};
use hickory_server::net::runtime::TokioRuntimeProvider;
use hickory_server::net::tcp::TcpClientStream;
use std::net::SocketAddr;

use super::xfer_timeout::XFER_TIMEOUT;

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
