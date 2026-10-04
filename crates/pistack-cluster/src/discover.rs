// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Private-subnet discovery. WireGuard does not carry a broadcast, so a
//! new node unicasts to each address in its own prefix. The prefix must be
//! /24 or longer. `0.0.0.0` and public ranges are refused.

use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

pub const DISCOVER_PORT: u16 = 18445;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announce {
    pub cluster_id: String,
    pub node_id: String,
    pub api: String,
    pub raft: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DiscoverError {
    #[error("advertise address {0} is not a private unicast")]
    Address(String),
    #[error("prefix /{0} is too wide to scan")]
    Prefix(u8),
    #[error("discovery socket: {0}")]
    Io(String),
}

/// Refuse unspecified, multicast, broadcast, and public addresses.
pub fn check_advertise(ip: IpAddr) -> Result<(), DiscoverError> {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_private() && !v4.is_unspecified() && !v4.is_broadcast() && !v4.is_multicast() {
                Ok(())
            } else {
                Err(DiscoverError::Address(v4.to_string()))
            }
        }
        IpAddr::V6(v6) => Err(DiscoverError::Address(v6.to_string())),
    }
}

/// Hosts in `ip/prefix`, skipping the network, the broadcast, and `ip`.
/// Prefixes shorter than /24 are refused. /32 returns an empty list.
pub fn scan_targets(ip: Ipv4Addr, prefix: u8) -> Result<Vec<Ipv4Addr>, DiscoverError> {
    if prefix < 24 || prefix > 32 {
        return Err(DiscoverError::Prefix(prefix));
    }
    if prefix == 32 {
        return Ok(Vec::new());
    }
    let width = 32 - prefix;
    let size = 1u32 << width;
    let base = u32::from(ip) & !(size - 1);
    let mut out = Vec::with_capacity(size as usize);
    for offset in 1..size - 1 {
        let host = Ipv4Addr::from(base + offset);
        if host != ip {
            out.push(host);
        }
    }
    Ok(out)
}

/// Bind `ip:port`, send one announce to `dest`, and return announces heard
/// on this socket within `wait`, skipping `self_id`.
pub async fn collect_announcements(
    ip: IpAddr,
    port: u16,
    dest: SocketAddr,
    mine: &Announce,
    wait: Duration,
    self_id: &str,
) -> Result<Vec<Announce>, DiscoverError> {
    let sock = UdpSocket::bind(SocketAddr::new(ip, port))
        .await
        .map_err(|e| DiscoverError::Io(e.to_string()))?;
    let payload = serde_json::to_vec(mine).map_err(|e| DiscoverError::Io(e.to_string()))?;
    // Immediate announce, then again at the halfway mark so a peer that binds
    // a moment later still hears it.
    send_datagram(&sock, &payload, dest).await?;

    let start = tokio::time::Instant::now();
    let deadline = start + wait;
    let resend_at = start + wait / 2;
    let mut resent = false;
    let mut heard = Vec::new();
    let mut buf = [0u8; 8192];

    while tokio::time::Instant::now() < deadline {
        let now = tokio::time::Instant::now();
        if !resent && now >= resend_at {
            send_datagram(&sock, &payload, dest).await?;
            resent = true;
            continue;
        }
        let wake = if resent { deadline } else { resend_at };
        let pause = wake.saturating_duration_since(now);
        if pause.is_zero() {
            continue;
        }
        match tokio::time::timeout(pause, sock.recv_from(&mut buf)).await {
            Err(_) => {}
            Ok(Ok((n, _))) => {
                let Ok(ann) = serde_json::from_slice::<Announce>(&buf[..n]) else {
                    continue;
                };
                if ann.node_id != self_id && !heard.contains(&ann) {
                    heard.push(ann);
                }
            }
            Ok(Err(e)) if port_unreachable(&e) => {}
            Ok(Err(e)) => return Err(DiscoverError::Io(e.to_string())),
        }
    }
    Ok(heard)
}

/// Bind `bind` once, send `mine` to every target, resend at half of `wait`,
/// and return announces whose node_id is not mine.node_id. Keep announces
/// with an empty cluster_id (a peer that is still forming). Skip bad JSON.
pub async fn sweep(
    bind: SocketAddr,
    targets: &[SocketAddr],
    mine: &Announce,
    wait: Duration,
) -> Result<Vec<Announce>, DiscoverError> {
    let sock = UdpSocket::bind(bind)
        .await
        .map_err(|err| DiscoverError::Io(err.to_string()))?;
    let payload = serde_json::to_vec(mine).map_err(|err| DiscoverError::Io(err.to_string()))?;
    for dest in targets {
        send_datagram(&sock, &payload, *dest).await?;
    }

    let start = tokio::time::Instant::now();
    let deadline = start + wait;
    let resend_at = start + wait / 2;
    let mut resent = false;
    let mut heard = Vec::new();
    let mut buf = [0u8; 8192];

    while tokio::time::Instant::now() < deadline {
        let now = tokio::time::Instant::now();
        if !resent && now >= resend_at {
            for dest in targets {
                send_datagram(&sock, &payload, *dest).await?;
            }
            resent = true;
            continue;
        }
        let wake = if resent { deadline } else { resend_at };
        let pause = wake.saturating_duration_since(now);
        if pause.is_zero() {
            continue;
        }
        match tokio::time::timeout(pause, sock.recv_from(&mut buf)).await {
            Err(_) => {}
            Ok(Ok((len, _))) => {
                let Ok(ann) = serde_json::from_slice::<Announce>(&buf[..len]) else {
                    continue;
                };
                if ann.node_id != mine.node_id && !heard.contains(&ann) {
                    heard.push(ann);
                }
            }
            Ok(Err(err)) if port_unreachable(&err) => {}
            Ok(Err(err)) => return Err(DiscoverError::Io(err.to_string())),
        }
    }
    Ok(heard)
}

/// ICMP port-unreachable stays queued on the socket and the next send drops
/// its datagram. Retry once after consuming that error.
async fn send_datagram(
    sock: &UdpSocket,
    payload: &[u8],
    dest: SocketAddr,
) -> Result<(), DiscoverError> {
    match sock.send_to(payload, dest).await {
        Ok(_) => Ok(()),
        Err(e) if port_unreachable(&e) => match sock.send_to(payload, dest).await {
            Ok(_) => Ok(()),
            Err(e) if port_unreachable(&e) => Ok(()),
            Err(e) => Err(DiscoverError::Io(e.to_string())),
        },
        Err(e) => Err(DiscoverError::Io(e.to_string())),
    }
}

fn port_unreachable(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertise_stays_on_a_private_address() {
        assert!(check_advertise("10.66.66.1".parse().unwrap()).is_ok());
        assert!(check_advertise("192.168.1.9".parse().unwrap()).is_ok());
        assert!(check_advertise(IpAddr::V4(Ipv4Addr::UNSPECIFIED)).is_err());
        assert!(check_advertise("8.8.8.8".parse().unwrap()).is_err());
        assert!(check_advertise("255.255.255.255".parse().unwrap()).is_err());
    }

    #[test]
    fn scan_is_the_private_prefix_without_self() {
        let got = scan_targets("10.66.66.1".parse().unwrap(), 24).unwrap();
        assert_eq!(got.len(), 253);
        assert!(!got.contains(&"10.66.66.0".parse().unwrap()));
        assert!(!got.contains(&"10.66.66.1".parse().unwrap()));
        assert!(!got.contains(&"10.66.66.255".parse().unwrap()));
        assert!(got.contains(&"10.66.66.2".parse().unwrap()));
        assert!(scan_targets("10.66.66.1".parse().unwrap(), 16).is_err());
        assert!(scan_targets("10.66.66.1".parse().unwrap(), 32)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn two_sockets_hear_each_other() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let left = Announce {
            cluster_id: "c1".into(),
            node_id: "a".into(),
            api: "http://127.0.0.1:1".into(),
            raft: "127.0.0.1:2".into(),
        };
        let right = Announce {
            cluster_id: "c1".into(),
            node_id: "b".into(),
            api: "http://127.0.0.1:3".into(),
            raft: "127.0.0.1:4".into(),
        };
        let left_task = collect_announcements(
            ip,
            18451,
            "127.0.0.1:18452".parse().unwrap(),
            &left,
            Duration::from_millis(400),
            "a",
        );
        let right_task = collect_announcements(
            ip,
            18452,
            "127.0.0.1:18451".parse().unwrap(),
            &right,
            Duration::from_millis(400),
            "b",
        );
        let (heard_left, heard_right) = tokio::join!(left_task, right_task);
        let heard_left = heard_left.unwrap();
        let heard_right = heard_right.unwrap();
        assert_eq!(heard_left, vec![right]);
        assert_eq!(heard_right, vec![left]);
    }

    #[tokio::test]
    async fn sweep_hears_one_responder() {
        let responder: SocketAddr = "127.0.0.1:18461".parse().unwrap();
        let bind: SocketAddr = "127.0.0.1:18462".parse().unwrap();
        let peer = Announce {
            cluster_id: String::new(),
            node_id: "peer".into(),
            api: "http://127.0.0.1:9".into(),
            raft: "127.0.0.1:10".into(),
        };
        let mine = Announce {
            cluster_id: String::new(),
            node_id: "me".into(),
            api: "http://127.0.0.1:11".into(),
            raft: "127.0.0.1:12".into(),
        };
        let sock = UdpSocket::bind(responder).await.unwrap();
        let reply = peer.clone();
        let task = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            let (_len, from) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf))
                .await
                .expect("responder timed out")
                .unwrap();
            let payload = serde_json::to_vec(&reply).unwrap();
            sock.send_to(&payload, from).await.unwrap();
        });
        let heard = sweep(bind, &[responder], &mine, Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(heard, vec![peer]);
        task.await.unwrap();
    }
}
