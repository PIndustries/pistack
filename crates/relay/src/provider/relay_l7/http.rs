// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Reading and rewriting the request itself: the head off the wire, the
//! route key out of it, and the forwarded request written back out.

use std::net::SocketAddr;
use tokio::io::AsyncReadExt;

/// The request's host and path. The host comes from an absolute request
/// target when there is one, otherwise from the `Host` header. The path
/// drops any query or fragment.
pub(super) fn route_key(head: &[u8]) -> (Option<String>, String) {
    let Ok(text) = std::str::from_utf8(head) else {
        return (None, "/".into());
    };
    let mut lines = text.split("\r\n");
    let target = lines
        .next()
        .and_then(|start| start.split_whitespace().nth(1))
        .unwrap_or("/");
    let mut host = None;
    let mut path = target;
    for scheme in ["http://", "https://"] {
        if let Some(rest) = target.strip_prefix(scheme) {
            let (authority, tail) = match rest.find('/') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, "/"),
            };
            host = Some(normalize_host(authority));
            path = tail;
        }
    }
    if host.is_none() {
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("host") {
                    host = Some(normalize_host(value));
                    break;
                }
            }
        }
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    (host.filter(|h| !h.is_empty()), path.to_owned())
}

/// Lower-case, without a port or a trailing dot.
pub(super) fn normalize_host(raw: &str) -> String {
    let raw = raw.trim();
    let host = if let Some(rest) = raw.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        raw.rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(raw, |(host, _)| host)
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

pub(super) async fn read_headers<R>(stream: &mut R) -> Option<(Vec<u8>, Vec<u8>)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if buf.len() > 65_536 {
            return None;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        let n = match tokio::time::timeout(left, stream.read(&mut tmp)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return None,
            Ok(Ok(n)) => n,
        };
        buf.extend_from_slice(&tmp[..n]);
        if let Some(end) = find_header_end(&buf) {
            let rest = buf.split_off(end);
            return Some((buf, rest));
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

pub(super) fn header_has(head: &[u8], name: &str) -> bool {
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    text.lines().skip(1).any(|line| {
        line.split(':')
            .next()
            .is_some_and(|h| h.trim().eq_ignore_ascii_case(name))
    })
}

pub(super) fn content_length(head: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(head).ok()?;
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

pub(super) fn rewrite_request(head: &[u8], peer: SocketAddr) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(head).ok()?;
    let (start, headers) = text.split_once("\r\n")?;
    let mut parts = start.split_whitespace();
    let method = parts.next()?;
    let _path = parts.next()?;
    let version = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !matches!(
        method,
        "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "PATCH"
    ) {
        return None;
    }
    if !version.starts_with("HTTP/1.") {
        return None;
    }
    let mut out = String::new();
    out.push_str(start);
    out.push_str("\r\n");
    let mut forwarded = false;
    for line in headers.split("\r\n") {
        if line.is_empty() {
            break;
        }
        let name = line.split(':').next().unwrap_or("").trim();
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
        ) {
            continue;
        }
        if name.eq_ignore_ascii_case("x-forwarded-for") {
            forwarded = true;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    if !forwarded {
        out.push_str(&format!("X-Forwarded-For: {peer}\r\n"));
    }
    out.push_str("Connection: close\r\n\r\n");
    Some(out.into_bytes())
}
