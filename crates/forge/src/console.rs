// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Browser console for a machine.
//!
//! The portal asks for a ticket with the caller's bearer token. The
//! WebSocket that follows carries only that ticket: a browser cannot set
//! `Authorization` on a WebSocket. The ticket lives in this process for five
//! minutes, is bound to the machine id, and is consumed when the upgrade
//! starts. The TCP connection is always `127.0.0.1` and the display number
//! comes from `virsh vncdisplay`. Nothing here publishes the VNC port.

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use pistack_types::ApiError;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

const TICKET_TTL: Duration = Duration::from_secs(5 * 60);
const VIRSH_TIMEOUT: Duration = Duration::from_secs(5);

struct Ticket {
    machine_id: String,
    expires: Instant,
}

static TICKETS: LazyLock<Mutex<HashMap<String, Ticket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 32 hex characters. Callers must not write this value to a log.
pub fn mint_ticket(machine_id: &str) -> String {
    let ticket = Uuid::now_v7().simple().to_string();
    let mut map = TICKETS.lock();
    let now = Instant::now();
    map.retain(|_, ticket| ticket.expires > now);
    map.insert(
        ticket.clone(),
        Ticket {
            machine_id: machine_id.to_owned(),
            expires: now + TICKET_TTL,
        },
    );
    ticket
}

/// True when the ticket is live and was minted for `machine_id`.
/// Does not consume it. A failed `virsh` can try the same ticket again.
pub fn check_ticket(ticket: &str, machine_id: &str) -> Result<(), ApiError> {
    let map = TICKETS.lock();
    match map.get(ticket) {
        Some(found) if found.expires > Instant::now() && found.machine_id == machine_id => Ok(()),
        _ => Err(ApiError::unauthorized("console ticket rejected")),
    }
}

/// Drop the ticket. A second upgrade with the same value is rejected.
pub fn take_ticket(ticket: &str, machine_id: &str) -> Result<(), ApiError> {
    let mut map = TICKETS.lock();
    match map.remove(ticket) {
        Some(found) if found.expires > Instant::now() && found.machine_id == machine_id => Ok(()),
        Some(found) => {
            map.insert(ticket.to_owned(), found);
            Err(ApiError::unauthorized("console ticket rejected"))
        }
        None => Err(ApiError::unauthorized("console ticket rejected")),
    }
}

/// `ticket` query value. Missing and empty are the same rejection.
pub fn query_ticket(raw: Option<&str>) -> Result<String, ApiError> {
    let query = raw.unwrap_or("");
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "ticket" {
            let ticket = percent_decode(value);
            if ticket.is_empty() {
                break;
            }
            return Ok(ticket);
        }
    }
    Err(ApiError::unauthorized("console ticket rejected"))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// `pistack-{id}`. Same character rule as anvil, without a forge dependency
/// on the host agent.
pub fn domain_name(id: &str) -> Result<String, ApiError> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(ApiError::bad_request(
            "machine id cannot be used as a domain name",
        ));
    }
    Ok(format!("pistack-{id}"))
}

/// Display number from `virsh vncdisplay`. `:1` is TCP 5901.
/// Only a loopback host is accepted. `0.0.0.0` and every other address are rejected.
pub fn vnc_port(display: &str) -> Result<u16, String> {
    let line = display
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.is_empty() {
        return Err("empty vnc display".into());
    }
    let (host, number) = split_display(line)?;
    if !is_loopback(host) {
        return Err("vnc is not on localhost".into());
    }
    let number: u32 = number
        .parse()
        .map_err(|_| "bad vnc display number".to_owned())?;
    let port = 5900u32
        .checked_add(number)
        .ok_or_else(|| "vnc port overflow".to_owned())?;
    u16::try_from(port).map_err(|_| "vnc port overflow".to_owned())
}

fn split_display(line: &str) -> Result<(&str, &str), String> {
    if let Some(number) = line.strip_prefix("[::1]:") {
        return Ok(("::1", number));
    }
    match line.rsplit_once(':') {
        Some((host, number)) if !number.is_empty() => Ok((host, number)),
        _ => Err("bad vnc display".into()),
    }
}

fn is_loopback(host: &str) -> bool {
    host.is_empty()
        || host == "127.0.0.1"
        || host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host == "[::1]"
}

/// Run `virsh vncdisplay` and return the loopback TCP port.
pub async fn display_port(machine_id: &str) -> Result<u16, ApiError> {
    let domain = domain_name(machine_id)?;
    let child = tokio::process::Command::new("virsh")
        .args(["-c", "qemu:///system", "vncdisplay", &domain])
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ApiError::unavailable("virsh is not available"))?;
    let finished = tokio::time::timeout(VIRSH_TIMEOUT, child.wait_with_output()).await;
    let output = match finished {
        Ok(Ok(output)) => output,
        Ok(Err(_)) => return Err(ApiError::unavailable("virsh failed")),
        Err(_) => return Err(ApiError::unavailable("virsh timed out")),
    };
    if !output.status.success() {
        return Err(ApiError::unavailable("vnc display is not available"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    vnc_port(&text).map_err(|_| ApiError::unavailable("vnc display is not on localhost"))
}

/// Copy bytes between the browser socket and the loopback VNC port.
pub async fn splice(socket: WebSocket, port: u16) {
    let stream = match TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(%error, port, "vnc connect failed");
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);

    let from_vnc = tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let to_browser = tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            if sink.send(Message::Binary(chunk.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(message) = stream.next().await {
        let bytes = match message {
            Ok(Message::Binary(bytes)) => bytes,
            Ok(Message::Text(text)) => text.as_bytes().to_vec().into(),
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Err(_) => break,
        };
        if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
            break;
        }
    }

    from_vnc.abort();
    to_browser.abort();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_expiry_and_wrong_machine() {
        let ticket = Uuid::now_v7().simple().to_string();
        assert_eq!(ticket.len(), 32);
        assert!(ticket.chars().all(|c| c.is_ascii_hexdigit()));
        {
            let mut map = TICKETS.lock();
            map.insert(
                ticket.clone(),
                Ticket {
                    machine_id: "mach-a".into(),
                    expires: Instant::now() + TICKET_TTL,
                },
            );
        }
        assert!(check_ticket(&ticket, "mach-b").is_err());
        assert!(check_ticket(&ticket, "mach-a").is_ok());

        let expired = format!("{ticket}ee");
        {
            let mut map = TICKETS.lock();
            map.insert(
                expired.clone(),
                Ticket {
                    machine_id: "mach-a".into(),
                    expires: Instant::now() - Duration::from_secs(5),
                },
            );
        }
        assert!(check_ticket(&expired, "mach-a").is_err());
        assert!(take_ticket(&expired, "mach-a").is_err());
        assert!(check_ticket(&ticket, "mach-a").is_ok());
        assert!(take_ticket(&ticket, "mach-a").is_ok());
        assert!(take_ticket(&ticket, "mach-a").is_err());
        assert!(query_ticket(None).is_err());
        assert!(query_ticket(Some("")).is_err());
        assert!(query_ticket(Some("ticket=")).is_err());
        assert_eq!(query_ticket(Some("other=1&ticket=abc")).unwrap(), "abc");
    }

    #[test]
    fn vnc_port_accepts_only_loopback() {
        assert_eq!(vnc_port(":1").unwrap(), 5901);
        assert_eq!(vnc_port("127.0.0.1:2").unwrap(), 5902);
        assert_eq!(vnc_port("127.0.0.1:0\n").unwrap(), 5900);
        assert_eq!(vnc_port("localhost:3").unwrap(), 5903);
        assert_eq!(vnc_port("::1:4").unwrap(), 5904);
        assert_eq!(vnc_port("[::1]:5").unwrap(), 5905);
        assert!(vnc_port("10.66.66.1:1").is_err());
        assert!(vnc_port("0.0.0.0:0").is_err());
        assert!(vnc_port("").is_err());
        assert!(vnc_port("   ").is_err());
        assert!(vnc_port(":").is_err());
        assert!(domain_name("mach-01a1007f-5708-7534-8fdf-a9bec59c383b").is_ok());
        assert!(domain_name("mach-1;rm").is_err());
        assert!(domain_name("../etc").is_err());
        assert!(domain_name("").is_err());
    }
}
