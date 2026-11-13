// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Active health checking.
//!
//! One task probes every member of every pool that has a `health_monitor`,
//! on that monitor's interval, and flips `status.operating_status` with
//! rise/fall hysteresis: `fall` consecutive failures take a member
//! `Online -> Offline`, `rise` consecutive successes bring it back. A flip
//! touches the pool so the reconciler re-programs the listeners using it.
//!
//! Probes: `Tcp` is a connect; `Http` is a `GET path` over a fresh
//! connection whose status line must match `expected_codes`; `Https` is a
//! connect only until a TLS client is wired in (noted in the member
//! message). Every probe is bounded by the monitor's `timeout`.
//!
//! Members administratively down are left `Draining` and not probed; pools
//! without a monitor leave their members `NoMonitor`.

use crate::client::Client;
use crate::types::*;
use pistack_types::module::CancellationToken;
use pistack_types::{Id, Phase};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How often the checker looks for due probes.
const TICK: Duration = Duration::from_secs(1);

/// Result of one probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    Up,
    Down(String),
}

/// Probe one member with the monitor's settings.
pub async fn probe(addr: SocketAddr, hm: &HealthMonitor) -> Probe {
    let timeout = Duration::from_secs(hm.timeout.max(1) as u64);
    match tokio::time::timeout(timeout, probe_inner(addr, hm)).await {
        Ok(p) => p,
        Err(_) => Probe::Down(format!("timeout after {}s", timeout.as_secs())),
    }
}

async fn probe_inner(addr: SocketAddr, hm: &HealthMonitor) -> Probe {
    let mut stream = match tokio::net::TcpStream::connect(addr).await {
        Ok(s) => s,
        Err(e) => return Probe::Down(format!("connect: {e}")),
    };
    if hm.kind != MonitorType::Http {
        return Probe::Up;
    }
    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: pistack-relay/{}\r\nConnection: close\r\n\r\n",
        hm.path,
        addr,
        pistack_types::BUILD
    );
    if let Err(e) = stream.write_all(req.as_bytes()).await {
        return Probe::Down(format!("write: {e}"));
    }
    let mut buf = vec![0u8; 512];
    let mut read = 0;
    while read < buf.len() {
        match stream.read(&mut buf[read..]).await {
            Ok(0) => break,
            Ok(n) => {
                read += n;
                if buf[..read].contains(&b'\n') {
                    break;
                }
            }
            Err(e) => return Probe::Down(format!("read: {e}")),
        }
    }
    let line = String::from_utf8_lossy(&buf[..read]);
    let line = line.lines().next().unwrap_or_default();
    // "HTTP/1.1 200 OK"
    let code = line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok());
    match code {
        Some(c) if hm.expected_codes.contains(&c) => Probe::Up,
        Some(c) => Probe::Down(format!("status {c}")),
        None => Probe::Down(format!("bad status line {line:?}")),
    }
}

/// Apply one probe result to a member's status. Returns true when the
/// operating status flipped.
pub fn apply(status: &mut MemberStatus, hm: &HealthMonitor, result: &Probe) -> bool {
    use OperatingStatus::*;
    let was = status.operating_status;
    match (result, was) {
        (Probe::Up, Online) => status.streak = 0,
        (Probe::Up, _) => {
            status.streak += 1;
            if status.streak >= hm.rise {
                status.operating_status = Online;
                status.streak = 0;
            }
        }
        (Probe::Down(_), Offline) => status.streak = 0,
        (Probe::Down(_), _) => {
            status.streak += 1;
            if status.streak >= hm.fall {
                status.operating_status = Offline;
                status.streak = 0;
            }
        }
    }
    status.message = match result {
        Probe::Up => String::new(),
        Probe::Down(m) => m.clone(),
    };
    if hm.kind == MonitorType::Https && status.message.is_empty() {
        status.message = "https monitor: tcp connect only (no tls probe yet)".into();
    }
    status.last_check = Some(chrono::Utc::now());
    status.operating_status != was
}

/// The checker's memory between ticks: when each member was last probed.
pub struct Checker {
    client: Client,
    last: HashMap<Id, Instant>,
}

impl Checker {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            last: HashMap::new(),
        }
    }

    /// Probe every member that is due (or every member, with `force`).
    /// Returns the number of members whose operating status flipped.
    pub async fn tick(&mut self, force: bool) -> usize {
        let now = Instant::now();
        let mut flipped = 0;
        let pools = self.client.pools().list(None).unwrap_or_default();
        for pool in pools {
            let hm = match &pool.spec.health_monitor {
                Some(hm) if pool.spec.admin_up => hm.clone(),
                _ => continue,
            };
            let interval = Duration::from_secs(hm.interval.max(1) as u64);
            for m in self.client.members_of(&pool.meta.id).unwrap_or_default() {
                if !m.spec.admin_up {
                    continue;
                }
                let due = force
                    || self
                        .last
                        .get(&m.meta.id)
                        .is_none_or(|t| now.duration_since(*t) >= interval);
                if !due {
                    continue;
                }
                self.last.insert(m.meta.id.clone(), now);
                let result = probe(m.addr(), &hm).await;
                let mut next = m.status.clone();
                let flip = apply(&mut next, &hm, &result);
                let changed =
                    flip || next.streak != m.status.streak || next.message != m.status.message;
                if !changed {
                    continue;
                }
                let write = self.client.members().modify(&m.meta.id, |x| {
                    x.status = next.clone();
                    x.status.phase = Phase::Ready;
                    x.status.observed_revision = x.meta.revision + 1;
                    Ok(())
                });
                match write {
                    Ok(_) if flip => {
                        tracing::info!(
                            member = %m.meta.id, addr = %m.addr(), status = ?next.operating_status,
                            "relay: member status changed"
                        );
                        flipped += 1;
                        self.client.touch_pool(&pool.meta.id);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(member = %m.meta.id, error = %e, "relay: health status write failed")
                    }
                }
            }
        }
        // Forget members that are gone.
        let live: std::collections::HashSet<Id> = self
            .client
            .members()
            .list(None)
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.meta.id)
            .collect();
        self.last.retain(|id, _| live.contains(id));
        flipped
    }
}

/// Run the checker until `shutdown` fires.
pub fn start(client: Client, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut checker = Checker::new(client);
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("relay health checker stopping");
                    break;
                }
                _ = tick.tick() => {
                    checker.tick(false).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hm(rise: u32, fall: u32) -> HealthMonitor {
        HealthMonitor {
            rise,
            fall,
            ..HealthMonitor::default()
        }
    }

    #[test]
    fn rise_and_fall_hysteresis() {
        let hm = hm(2, 3);
        let mut s = MemberStatus::default(); // NoMonitor
        assert!(!apply(&mut s, &hm, &Probe::Up));
        assert_eq!(s.streak, 1);
        assert!(apply(&mut s, &hm, &Probe::Up));
        assert_eq!(s.operating_status, OperatingStatus::Online);
        assert!(!apply(&mut s, &hm, &Probe::Down("x".into())));
        assert!(!apply(&mut s, &hm, &Probe::Down("x".into())));
        assert_eq!(s.operating_status, OperatingStatus::Online);
        // a success resets the fall streak
        assert!(!apply(&mut s, &hm, &Probe::Up));
        assert_eq!(s.streak, 0);
        assert!(!apply(&mut s, &hm, &Probe::Down("x".into())));
        assert!(!apply(&mut s, &hm, &Probe::Down("x".into())));
        assert!(apply(&mut s, &hm, &Probe::Down("x".into())));
        assert_eq!(s.operating_status, OperatingStatus::Offline);
        assert_eq!(s.message, "x");
    }

    #[tokio::test]
    async fn tcp_and_http_probes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await;
                    let _ = s
                        .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                        .await;
                });
            }
        });
        assert_eq!(probe(addr, &HealthMonitor::default()).await, Probe::Up);
        let http = HealthMonitor {
            kind: MonitorType::Http,
            ..HealthMonitor::default()
        };
        assert_eq!(probe(addr, &http).await, Probe::Down("status 503".into()));
        let lenient = HealthMonitor {
            expected_codes: vec![200, 503],
            ..http
        };
        assert_eq!(probe(addr, &lenient).await, Probe::Up);

        // nothing listening
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead = closed.local_addr().unwrap();
        drop(closed);
        assert!(matches!(
            probe(dead, &HealthMonitor::default()).await,
            Probe::Down(_)
        ));
    }
}
